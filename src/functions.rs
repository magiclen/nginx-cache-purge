use std::{
    collections::HashSet,
    fs::{self, File},
    io::{self, Read},
    mem,
    ops::Range,
    path::{Path, PathBuf},
    sync::{
        Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
        mpsc::{self, SyncSender},
    },
    thread,
};

use anyhow::{Context, anyhow};
use md5::{Digest, Md5};

use crate::AppResult;

/// nginx writes this marker right after the binary cache header, then the key and a line feed.
const KEY_MARKER: &[u8] = b"\nKEY: ";

/// nginx keeps the offset of the HTTP headers in a `u_short`, so the `KEY` line always fits in 64 KiB.
const KEY_SEARCH_LIMIT: usize = 64 * 1024;

/// Big enough to hold the binary header (336 bytes on x86_64) plus a typical key in a single read.
const KEY_SEARCH_CHUNK: usize = 4096;

/// The number of cache file paths handed to a worker thread at once.
const BATCH_SIZE: usize = 64;

/// When set, nothing is actually removed from the file system.
static DRY_RUN: AtomicBool = AtomicBool::new(false);

#[inline]
pub fn set_dry_run(dry_run: bool) {
    DRY_RUN.store(dry_run, Ordering::Relaxed);
}

#[inline]
fn is_dry_run() -> bool {
    DRY_RUN.load(Ordering::Relaxed)
}

#[inline]
fn worker_count() -> usize {
    thread::available_parallelism().map(|n| n.get()).unwrap_or(1).clamp(1, 16)
}

#[inline]
fn remove_file<P: AsRef<Path>>(path: P) -> io::Result<()> {
    let path = path.as_ref();

    if is_dry_run() {
        // a missing file still has to be reported as such, otherwise a dry run would not end with the same result as a real run
        fs::symlink_metadata(path)?;

        println!("Remove file: {path:?}");

        Ok(())
    } else {
        fs::remove_file(path)
    }
}

#[inline]
fn remove_dir_all<P: AsRef<Path>>(path: P) -> io::Result<()> {
    let path = path.as_ref();

    if is_dry_run() {
        fs::symlink_metadata(path)?;

        println!("Remove dir all: {path:?}");

        Ok(())
    } else {
        fs::remove_dir_all(path)
    }
}

#[inline]
fn remove_dir<P: AsRef<Path>>(path: P) -> io::Result<()> {
    let path = path.as_ref();

    if is_dry_run() {
        // a dry run keeps every file, so report the directory as it is instead of pretending it became empty
        if path.read_dir()?.next().is_some() {
            return Err(io::ErrorKind::DirectoryNotEmpty.into());
        }

        println!("Remove dir: {path:?}");

        Ok(())
    } else {
        fs::remove_dir(path)
    }
}

fn remove_empty_ancestors<P: AsRef<Path>>(path: P, relative_degree: usize) -> anyhow::Result<()> {
    if let Some(mut path) = path.as_ref().parent() {
        for _ in 1..=relative_degree {
            match remove_dir(path) {
                Ok(_) => (),
                Err(error)
                    if matches!(
                        error.kind(),
                        io::ErrorKind::NotFound | io::ErrorKind::DirectoryNotEmpty
                    ) =>
                {
                    return Ok(());
                },
                Err(error) => return Err(error).with_context(|| anyhow!("{path:?}")),
            }

            match path.parent() {
                Some(parent) => {
                    path = parent;
                },
                None => break,
            }
        }
    }

    Ok(())
}

/// Do something like `rm -rf /path/to/*`. The `/path/to` directory will not be deleted. This function may be dangerous.
pub fn remove_all_files_in_directory<P: AsRef<Path>>(path: P) -> anyhow::Result<bool> {
    let path = path.as_ref();

    let mut entries: Vec<(PathBuf, bool)> = Vec::new();

    for dir_entry in path.read_dir().with_context(|| anyhow!("{path:?}"))? {
        let dir_entry = dir_entry.with_context(|| anyhow!("{path:?}"))?;

        let file_type = match dir_entry.file_type() {
            Ok(file_type) => file_type,
            Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
            Err(error) => return Err(error).with_context(|| anyhow!("{dir_entry:?}")),
        };

        entries.push((dir_entry.path(), file_type.is_dir()));
    }

    if entries.is_empty() {
        return Ok(false);
    }

    let next = AtomicUsize::new(0);
    let removed = AtomicBool::new(false);
    let first_error: Mutex<Option<anyhow::Error>> = Mutex::new(None);

    thread::scope(|scope| {
        for _ in 0..worker_count().min(entries.len()) {
            scope.spawn(|| {
                while let Some((path, is_dir)) = entries.get(next.fetch_add(1, Ordering::Relaxed)) {
                    let result = if *is_dir { remove_dir_all(path) } else { remove_file(path) };

                    match result {
                        Ok(_) => removed.store(true, Ordering::Relaxed),
                        Err(error) if error.kind() == io::ErrorKind::NotFound => {
                            removed.store(true, Ordering::Relaxed);
                        },
                        Err(error) => {
                            first_error.lock().unwrap().get_or_insert_with(|| {
                                anyhow::Error::new(error).context(anyhow!("{path:?}"))
                            });

                            break;
                        },
                    }
                }
            });
        }
    });

    match first_error.into_inner().unwrap() {
        Some(error) => Err(error),
        None => Ok(removed.load(Ordering::Relaxed)),
    }
}

/// Purge a cache with a specific key.
pub fn remove_one_cache<P: AsRef<Path>>(
    cache_path: P,
    levels: &str,
    key: &str,
    exclude_keys: &[&str],
) -> anyhow::Result<AppResult> {
    let levels = parse_levels(levels)?;
    let number_of_levels = levels.len();

    for exclude_key in exclude_keys {
        if hit_key(key, &parse_key(exclude_key)) {
            return Ok(AppResult::CacheIgnored);
        }
    }

    let file_path = create_cache_file_path(cache_path, &levels, key);

    match remove_file(&file_path) {
        Ok(_) => {
            remove_empty_ancestors(&file_path, number_of_levels)?;

            Ok(AppResult::Ok)
        },
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            Ok(AppResult::AlreadyPurged(file_path))
        },
        Err(error) => Err(error).with_context(|| anyhow!("{file_path:?}")),
    }
}

/// Purge multiple caches via wildcard.
pub fn remove_caches_via_wildcard<P: AsRef<Path>>(
    cache_path: P,
    levels: &str,
    key: &str,
    exclude_keys: &[&str],
) -> anyhow::Result<AppResult> {
    let cache_path = cache_path.as_ref();

    let cache_path = match cache_path.canonicalize() {
        Ok(path) => {
            if !path.is_dir() {
                return Err(anyhow!("{cache_path:?} is not a directory."));
            }

            path
        },
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return Ok(AppResult::AlreadyPurgedWildcard);
        },
        Err(error) => return Err(error).with_context(|| anyhow!("{cache_path:?}")),
    };

    let levels = parse_levels(levels)?;
    let number_of_levels = levels.len();

    let mut exclude_key_segments: Vec<Vec<&[u8]>> = Vec::new();
    let mut exclude_paths: HashSet<PathBuf> = HashSet::new();

    for exclude_key in exclude_keys {
        if exclude_key.contains('*') {
            let segments = parse_key(exclude_key);

            if is_match_all(&segments) {
                return Ok(AppResult::AlreadyPurgedWildcard);
            }

            exclude_key_segments.push(segments);
        } else {
            exclude_paths.insert(create_cache_file_path(
                cache_path.as_path(),
                &levels,
                exclude_key,
            ));
        }
    }

    let segments = parse_key(key);

    if is_match_all(&segments) && exclude_key_segments.is_empty() && exclude_paths.is_empty() {
        return remove_all_files_in_directory(cache_path).map(|modified| {
            if modified { AppResult::Ok } else { AppResult::AlreadyPurgedWildcard }
        });
    }

    let workers = worker_count();

    let (sender, receiver) = mpsc::sync_channel::<Vec<PathBuf>>(workers * 2);
    let receiver = Mutex::new(receiver);

    let removed = AtomicBool::new(false);
    let aborted = AtomicBool::new(false);
    let first_error: Mutex<Option<anyhow::Error>> = Mutex::new(None);

    // the directories are collected in post-order, so a child always comes before its parent
    let mut directories: Vec<PathBuf> = Vec::new();

    let walk_result = thread::scope(|scope| {
        for _ in 0..workers {
            scope.spawn(|| {
                let mut buffer = Vec::with_capacity(KEY_SEARCH_CHUNK);

                loop {
                    let batch = {
                        let receiver = receiver.lock().unwrap();

                        match receiver.recv() {
                            Ok(batch) => batch,
                            Err(_) => break,
                        }
                    };

                    // keep draining the channel after a failure, otherwise the walking thread would block forever
                    if aborted.load(Ordering::Relaxed) {
                        continue;
                    }

                    for file_path in batch {
                        match match_key_and_remove_one_cache(
                            &segments,
                            &exclude_key_segments,
                            &file_path,
                            &mut buffer,
                        ) {
                            Ok(true) => removed.store(true, Ordering::Relaxed),
                            Ok(false) => (),
                            Err(error) => {
                                aborted.store(true, Ordering::Relaxed);
                                first_error.lock().unwrap().get_or_insert(error);

                                break;
                            },
                        }
                    }
                }
            });
        }

        let mut batch = Vec::with_capacity(BATCH_SIZE);

        let result = collect_cache_files(
            cache_path.as_path(),
            number_of_levels,
            0,
            &exclude_paths,
            &aborted,
            &sender,
            &mut directories,
            &mut batch,
        );

        if result.is_ok() && !batch.is_empty() {
            let _ = sender.send(batch);
        }

        // closing the channel is what tells the workers to stop
        drop(sender);

        result
    });

    walk_result?;

    if let Some(error) = first_error.into_inner().unwrap() {
        return Err(error);
    }

    for directory in directories {
        match remove_dir(&directory) {
            Ok(_) => (),
            Err(error)
                if matches!(
                    error.kind(),
                    io::ErrorKind::NotFound | io::ErrorKind::DirectoryNotEmpty
                ) => {},
            Err(error) => return Err(error).with_context(|| anyhow!("{directory:?}")),
        }
    }

    Ok(if removed.load(Ordering::Relaxed) {
        AppResult::Ok
    } else {
        AppResult::AlreadyPurgedWildcard
    })
}

#[allow(clippy::too_many_arguments)]
fn collect_cache_files(
    path: &Path,
    number_of_levels: usize,
    level: usize,
    exclude_paths: &HashSet<PathBuf>,
    aborted: &AtomicBool,
    sender: &SyncSender<Vec<PathBuf>>,
    directories: &mut Vec<PathBuf>,
    batch: &mut Vec<PathBuf>,
) -> anyhow::Result<()> {
    for dir_entry in path.read_dir().with_context(|| anyhow!("{path:?}"))? {
        if aborted.load(Ordering::Relaxed) {
            return Ok(());
        }

        let dir_entry = dir_entry.with_context(|| anyhow!("{path:?}"))?;

        let file_type = match dir_entry.file_type() {
            Ok(file_type) => file_type,
            Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
            Err(error) => return Err(error).with_context(|| anyhow!("{dir_entry:?}")),
        };

        if number_of_levels == level {
            if file_type.is_file() {
                let file_path = dir_entry.path();

                if exclude_paths.contains(&file_path) {
                    continue;
                }

                batch.push(file_path);

                if batch.len() == BATCH_SIZE {
                    let batch = mem::replace(batch, Vec::with_capacity(BATCH_SIZE));

                    if sender.send(batch).is_err() {
                        return Ok(());
                    }
                }
            }
        } else if file_type.is_dir() {
            let dir_path = dir_entry.path();

            collect_cache_files(
                dir_path.as_path(),
                number_of_levels,
                level + 1,
                exclude_paths,
                aborted,
                sender,
                directories,
                batch,
            )?;

            directories.push(dir_path);
        }
    }

    Ok(())
}

fn match_key_and_remove_one_cache(
    segments: &[&[u8]],
    exclude_key_segments: &[Vec<&[u8]>],
    file_path: &Path,
    buffer: &mut Vec<u8>,
) -> anyhow::Result<bool> {
    let key = match read_cache_key(file_path, buffer) {
        Ok(Some(key)) => key,
        // the file is not an nginx cache file, so leave it alone
        Ok(None) => return Ok(false),
        // the file may be removed by nginx while we are walking the cache directory
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(error).with_context(|| anyhow!("{file_path:?}")),
    };

    let read_key = &buffer[key];

    for exclude_key_segments in exclude_key_segments {
        if hit_key(read_key, exclude_key_segments) {
            return Ok(false);
        }
    }

    if !hit_key(read_key, segments) {
        return Ok(false);
    }

    match remove_file(file_path) {
        Ok(_) => (),
        Err(error) if error.kind() == io::ErrorKind::NotFound => (),
        Err(error) => return Err(error).with_context(|| anyhow!("{file_path:?}")),
    }

    Ok(true)
}

/// Read an nginx cache file into `buffer` and return where its key is.
///
/// The binary header in front of the key holds timestamps and a checksum, which may contain `\n` or `\r`, so the `\nKEY: ` marker has to be searched for instead of assuming a fixed offset.
fn read_cache_key(file_path: &Path, buffer: &mut Vec<u8>) -> io::Result<Option<Range<usize>>> {
    let mut file = File::open(file_path)?;

    buffer.clear();

    let mut marker_end: Option<usize> = None;
    let mut searched: usize = 0;

    loop {
        let filled = buffer.len();

        if filled >= KEY_SEARCH_LIMIT {
            return Ok(None);
        }

        buffer.resize(filled + KEY_SEARCH_CHUNK, 0);

        let size = read_as_much_as_possible(&mut file, &mut buffer[filled..])?;

        buffer.truncate(filled + size);

        if size == 0 {
            return Ok(None);
        }

        if marker_end.is_none() {
            // the marker may straddle two reads, so step back before the bytes that were already searched
            let from = searched.saturating_sub(KEY_MARKER.len() - 1);

            if let Some(index) =
                buffer[from..].windows(KEY_MARKER.len()).position(|window| window == KEY_MARKER)
            {
                marker_end = Some(from + index + KEY_MARKER.len());
            }
        }

        if let Some(marker_end) = marker_end {
            let from = searched.max(marker_end);

            if let Some(index) = buffer[from..].iter().position(|e| *e == b'\n') {
                return Ok(Some(marker_end..(from + index)));
            }
        }

        searched = buffer.len();
    }
}

fn read_as_much_as_possible<R: Read>(reader: &mut R, buffer: &mut [u8]) -> io::Result<usize> {
    let mut filled = 0;

    while filled < buffer.len() {
        match reader.read(&mut buffer[filled..]) {
            Ok(0) => break,
            Ok(size) => filled += size,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => (),
            Err(error) => return Err(error),
        }
    }

    Ok(filled)
}

fn hit_key<RK: AsRef<[u8]>, K: AsRef<[u8]>>(read_key: RK, segments: &[K]) -> bool {
    let read_key = read_key.as_ref();

    let mut p = 0;
    let mut floating = false;

    for (i, segment) in segments.iter().enumerate() {
        let segment = segment.as_ref();

        // an empty segment is a `*`, which lets the next segment match at any position
        if segment.is_empty() {
            floating = true;

            continue;
        }

        if floating {
            let index = if i == segments.len() - 1 {
                // the pattern ends here, so this segment has to sit at the very end
                if read_key.len() < p + segment.len() || !read_key.ends_with(segment) {
                    return false;
                }

                read_key.len() - segment.len()
            } else {
                match read_key[p..]
                    .windows(segment.len())
                    .position(|window| window == segment)
                    .map(|index| index + p)
                {
                    Some(index) => index,
                    None => return false,
                }
            };

            p = index + segment.len();
            floating = false;
        } else {
            if read_key.len() - p < segment.len() || &read_key[p..(p + segment.len())] != segment {
                return false;
            }

            p += segment.len();
        }
    }

    // without a trailing `*` the whole key has to be consumed
    floating || p == read_key.len()
}

fn parse_levels(levels: &str) -> anyhow::Result<Vec<usize>> {
    // nginx allows `levels` to be omitted, which puts every cache file directly in the cache directory
    if levels.is_empty() {
        return Ok(Vec::new());
    }

    let levels: Vec<&str> = levels.split(':').collect();

    if levels.len() > 3 {
        return Err(anyhow!("The number of hierarchy levels cannot be bigger than 3."));
    }

    let mut levels_usize = Vec::with_capacity(levels.len());

    for level in levels {
        let level_usize = level
            .parse()
            .with_context(|| anyhow!("The value of levels should be a positive integer."))?;

        if !(1..=2).contains(&level_usize) {
            return Err(anyhow!("The value of levels should be 1 or 2."));
        }

        levels_usize.push(level_usize);
    }

    Ok(levels_usize)
}

/// Split a key pattern into segments, where an empty segment stands for a `*`.
fn parse_key(key: &str) -> Vec<&[u8]> {
    let key = key.as_bytes();

    let mut segments: Vec<&[u8]> = Vec::new();

    let mut p = 0;

    while let Some(i) = key[p..].iter().position(|u| *u == b'*').map(|i| i + p) {
        if i > p {
            segments.push(&key[p..i]);
        }

        // consecutive `*`s mean the same as a single one
        if segments.last().is_none_or(|segment| !segment.is_empty()) {
            segments.push(&[]);
        }

        p = i + 1;
    }

    if p < key.len() {
        segments.push(&key[p..]);
    }

    segments
}

#[inline]
fn is_match_all(segments: &[&[u8]]) -> bool {
    segments.len() == 1 && segments[0].is_empty()
}

fn create_cache_file_path<P: AsRef<Path>>(cache_path: P, levels: &[usize], key: &str) -> PathBuf {
    let mut hasher = Md5::new();
    hasher.update(key);

    let key_md5_value = u128::from_be_bytes(hasher.finalize().into());
    let hashed_key = format!("{key_md5_value:032x}");

    let mut file_path = cache_path.as_ref().to_path_buf();
    let mut p = 32; // md5's hex string length

    for level in levels {
        file_path.push(&hashed_key[(p - level)..p]);

        p -= level;
    }

    file_path.push(hashed_key);

    file_path
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The size of `ngx_http_file_cache_header_t` on x86_64.
    const NGINX_HEADER_SIZE: usize = 336;

    /// Build a file that looks like the one nginx writes, with a binary header that contains `\n` and `\r` on purpose.
    ///
    /// The real header holds timestamps and a checksum, so about one file in ten really does contain such a byte.
    fn write_cache_file(file_path: &Path, key: &str, header_size: usize) {
        let mut content = vec![0u8; header_size];

        content[8] = b'\n';
        content[12] = b'\r';
        content[header_size - 1] = b'\n';

        content.extend_from_slice(KEY_MARKER);
        content.extend_from_slice(key.as_bytes());
        content.push(b'\n');
        content.extend_from_slice(b"HTTP/1.1 200 OK\r\n\r\nhello");

        if let Some(parent) = file_path.parent() {
            fs::create_dir_all(parent).unwrap();
        }

        fs::write(file_path, content).unwrap();
    }

    fn read_key_of(file_path: &Path) -> Option<String> {
        let mut buffer = Vec::new();

        read_cache_key(file_path, &mut buffer)
            .unwrap()
            .map(|key| String::from_utf8(buffer[key].to_vec()).unwrap())
    }

    #[test]
    fn parse_levels_works() {
        assert_eq!(vec![1, 2], parse_levels("1:2").unwrap());
        assert_eq!(vec![1, 1, 1], parse_levels("1:1:1").unwrap());
        assert_eq!(vec![2], parse_levels("2").unwrap());
        // nginx allows `levels` to be omitted
        assert_eq!(Vec::<usize>::new(), parse_levels("").unwrap());

        assert!(parse_levels("3").is_err());
        assert!(parse_levels("1:2:1:2").is_err());
        assert!(parse_levels("a").is_err());
    }

    #[test]
    fn parse_key_works() {
        assert_eq!(vec![b"http/blog".as_slice()], parse_key("http/blog"));
        assert_eq!(vec![b"http/blog".as_slice(), b""], parse_key("http/blog*"));
        assert_eq!(vec![b"".as_slice(), b"/help", b""], parse_key("*/help*"));
        assert_eq!(vec![b"".as_slice()], parse_key("*"));
        // consecutive `*`s mean the same as a single one
        assert_eq!(vec![b"".as_slice()], parse_key("**"));
        assert_eq!(vec![b"a".as_slice(), b"", b"b"], parse_key("a**b"));
        assert_eq!(Vec::<&[u8]>::new(), parse_key(""));
    }

    #[test]
    fn hit_key_works() {
        assert!(hit_key("http/blog/a", &parse_key("http/blog*")));
        assert!(hit_key("http/blog", &parse_key("http/blog*")));
        assert!(!hit_key("http/blo", &parse_key("http/blog*")));

        assert!(hit_key("http/a/help/b", &parse_key("*/help*")));
        assert!(!hit_key("http/a/hel", &parse_key("*/help*")));

        assert!(hit_key("anything", &parse_key("*")));
        assert!(hit_key("", &parse_key("*")));

        assert!(hit_key("a/b/c", &parse_key("a*c")));
        assert!(hit_key("ac", &parse_key("a*c")));
        assert!(!hit_key("a/b/d", &parse_key("a*c")));

        // a pattern that does not end with `*` has to match the whole key
        assert!(hit_key("http/blog", &parse_key("http/blog")));
        assert!(!hit_key("http/blog/a", &parse_key("http/blog")));
        assert!(hit_key("http/a/help", &parse_key("*/help")));
        assert!(!hit_key("http/a/help/b", &parse_key("*/help")));

        assert!(hit_key("", &parse_key("")));
        assert!(!hit_key("a", &parse_key("")));
    }

    #[test]
    fn create_cache_file_path_works() {
        // the md5 of "https/example.org/a" is f09ad58f5b488fc9b23ac05982c3bbcd
        const KEY: &str = "https/example.org/a";

        assert_eq!(
            PathBuf::from("/cache/d/bc/f09ad58f5b488fc9b23ac05982c3bbcd"),
            create_cache_file_path("/cache", &[1, 2], KEY)
        );

        assert_eq!(
            PathBuf::from("/cache/d/c/b/f09ad58f5b488fc9b23ac05982c3bbcd"),
            create_cache_file_path("/cache", &[1, 1, 1], KEY)
        );

        assert_eq!(
            PathBuf::from("/cache/f09ad58f5b488fc9b23ac05982c3bbcd"),
            create_cache_file_path("/cache", &[], KEY)
        );
    }

    #[test]
    fn read_cache_key_skips_a_header_containing_line_breaks() {
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("cache");

        write_cache_file(&file_path, "https/example.org/a?b=1&c=2", NGINX_HEADER_SIZE);

        assert_eq!(Some("https/example.org/a?b=1&c=2".to_string()), read_key_of(&file_path));
    }

    #[test]
    fn read_cache_key_handles_a_key_that_does_not_fit_in_one_read() {
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("cache");

        let key = format!("https/example.org/{}", "a".repeat(KEY_SEARCH_CHUNK * 2));

        write_cache_file(&file_path, key.as_str(), NGINX_HEADER_SIZE);

        assert_eq!(Some(key), read_key_of(&file_path));
    }

    #[test]
    fn read_cache_key_handles_a_marker_across_two_reads() {
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("cache");

        // put the marker right on the boundary of the first read
        write_cache_file(&file_path, "https/example.org/a", KEY_SEARCH_CHUNK - 2);

        assert_eq!(Some("https/example.org/a".to_string()), read_key_of(&file_path));
    }

    #[test]
    fn read_cache_key_ignores_a_file_without_a_marker() {
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("cache");

        fs::write(&file_path, b"not an nginx cache file").unwrap();

        assert_eq!(None, read_key_of(&file_path));
    }

    #[test]
    fn remove_one_cache_works() {
        let dir = tempfile::tempdir().unwrap();
        let key = "https/example.org/a";
        let file_path = create_cache_file_path(dir.path(), &[1, 2], key);

        write_cache_file(&file_path, key, NGINX_HEADER_SIZE);

        assert_eq!(AppResult::Ok, remove_one_cache(dir.path(), "1:2", key, &[]).unwrap());
        assert!(!file_path.exists());
        // the levels directories should be gone as well
        assert!(!file_path.parent().unwrap().exists());

        assert_eq!(
            AppResult::AlreadyPurged(file_path),
            remove_one_cache(dir.path(), "1:2", key, &[]).unwrap()
        );
    }

    #[test]
    fn remove_one_cache_honors_exclude_keys() {
        let dir = tempfile::tempdir().unwrap();
        let key = "https/example.org/a";
        let file_path = create_cache_file_path(dir.path(), &[1, 2], key);

        write_cache_file(&file_path, key, NGINX_HEADER_SIZE);

        assert_eq!(
            AppResult::CacheIgnored,
            remove_one_cache(dir.path(), "1:2", key, &["https/example.org/*"]).unwrap()
        );
        assert!(file_path.exists());

        // an exclude key without a `*` has to match the whole key
        assert_eq!(
            AppResult::Ok,
            remove_one_cache(dir.path(), "1:2", key, &["https/example.org"]).unwrap()
        );
    }

    #[test]
    fn remove_caches_via_wildcard_works() {
        let dir = tempfile::tempdir().unwrap();
        let levels = [1, 2];

        let keys = ["https/example.org/a", "https/example.org/b", "https/other.org/c"];

        for key in keys {
            write_cache_file(
                &create_cache_file_path(dir.path(), &levels, key),
                key,
                NGINX_HEADER_SIZE,
            );
        }

        assert_eq!(
            AppResult::Ok,
            remove_caches_via_wildcard(dir.path(), "1:2", "https/example.org/*", &[]).unwrap()
        );

        assert!(!create_cache_file_path(dir.path(), &levels, keys[0]).exists());
        assert!(!create_cache_file_path(dir.path(), &levels, keys[1]).exists());
        assert!(create_cache_file_path(dir.path(), &levels, keys[2]).exists());
    }

    #[test]
    fn remove_caches_via_wildcard_honors_exclude_keys() {
        let dir = tempfile::tempdir().unwrap();
        let levels = [1, 2];

        let keys = ["https/example.org/a", "https/example.org/image/b", "https/example.org/c"];

        for key in keys {
            write_cache_file(
                &create_cache_file_path(dir.path(), &levels, key),
                key,
                NGINX_HEADER_SIZE,
            );
        }

        assert_eq!(
            AppResult::Ok,
            remove_caches_via_wildcard(dir.path(), "1:2", "*", &[
                "https/example.org/image/*",
                "https/example.org/c"
            ])
            .unwrap()
        );

        assert!(!create_cache_file_path(dir.path(), &levels, keys[0]).exists());
        assert!(create_cache_file_path(dir.path(), &levels, keys[1]).exists());
        assert!(create_cache_file_path(dir.path(), &levels, keys[2]).exists());
    }

    #[test]
    fn remove_caches_via_wildcard_cleans_up_empty_directories() {
        let dir = tempfile::tempdir().unwrap();
        let levels = [1, 2];

        for i in 0..64 {
            let key = format!("https/example.org/{i}");

            write_cache_file(
                &create_cache_file_path(dir.path(), &levels, key.as_str()),
                key.as_str(),
                NGINX_HEADER_SIZE,
            );
        }

        assert_eq!(
            AppResult::Ok,
            remove_caches_via_wildcard(dir.path(), "1:2", "https/*", &[]).unwrap()
        );

        assert_eq!(0, dir.path().read_dir().unwrap().count());
    }

    #[test]
    fn remove_caches_via_wildcard_works_without_levels() {
        let dir = tempfile::tempdir().unwrap();

        let keys = ["https/example.org/a", "https/other.org/b"];

        for key in keys {
            write_cache_file(&create_cache_file_path(dir.path(), &[], key), key, NGINX_HEADER_SIZE);
        }

        assert_eq!(
            AppResult::Ok,
            remove_caches_via_wildcard(dir.path(), "", "https/example.org/*", &[]).unwrap()
        );

        assert!(!create_cache_file_path(dir.path(), &[], keys[0]).exists());
        assert!(create_cache_file_path(dir.path(), &[], keys[1]).exists());
    }

    #[test]
    fn remove_caches_via_wildcard_reports_nothing_to_purge() {
        let dir = tempfile::tempdir().unwrap();
        let key = "https/other.org/a";

        write_cache_file(&create_cache_file_path(dir.path(), &[1, 2], key), key, NGINX_HEADER_SIZE);

        assert_eq!(
            AppResult::AlreadyPurgedWildcard,
            remove_caches_via_wildcard(dir.path(), "1:2", "https/example.org/*", &[]).unwrap()
        );
    }

    #[test]
    fn remove_all_files_in_directory_works() {
        let dir = tempfile::tempdir().unwrap();
        let levels = [1, 2];

        for i in 0..16 {
            let key = format!("https/example.org/{i}");

            write_cache_file(
                &create_cache_file_path(dir.path(), &levels, key.as_str()),
                key.as_str(),
                NGINX_HEADER_SIZE,
            );
        }

        assert_eq!(AppResult::Ok, remove_caches_via_wildcard(dir.path(), "1:2", "*", &[]).unwrap());
        assert_eq!(0, dir.path().read_dir().unwrap().count());

        assert_eq!(
            AppResult::AlreadyPurgedWildcard,
            remove_caches_via_wildcard(dir.path(), "1:2", "*", &[]).unwrap()
        );
    }
}
