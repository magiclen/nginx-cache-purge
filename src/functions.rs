use std::{
    io,
    path::{Path, PathBuf},
    sync::Arc,
};

use anyhow::{anyhow, Context};
use async_recursion::async_recursion;
use md5::{Digest, Md5};
use scanner_rust::{generic_array::typenum::U384, ScannerAscii};
use tokio::sync::Mutex;

use crate::AppResult;

#[inline]
async fn remove_file<P: AsRef<Path>>(path: P) -> io::Result<()> {
    let path = path.as_ref();

    if cfg!(debug_assertions) {
        println!("Remove file: {path:?}");

        Ok(())
    } else {
        tokio::fs::remove_file(path).await
    }
}

#[inline]
async fn remove_dir_all<P: AsRef<Path>>(path: P) -> io::Result<()> {
    let path = path.as_ref();

    if cfg!(debug_assertions) {
        println!("Remove dir all: {path:?}");

        Ok(())
    } else {
        tokio::fs::remove_dir_all(path).await
    }
}

#[inline]
async fn remove_dir<P: AsRef<Path>>(path: P) -> io::Result<()> {
    let path = path.as_ref();

    if cfg!(debug_assertions) {
        println!("Remove dir: {path:?}");
    } else {
        match tokio::fs::remove_dir(path).await {
            Ok(_) => (),
            Err(error) => {
                // check if the error is caused by directory is not empty
                // TODO we should just use `io::ErrorKind::DirectoryNotEmpty` in the future
                return if error.kind().to_string() == "directory not empty" {
                    Err(io::Error::new(io::ErrorKind::Other, error))
                } else {
                    Err(error)
                };
            },
        }
    }

    Ok(())
}

async fn remove_empty_ancestors<P: AsRef<Path>>(
    path: P,
    relative_degree: usize,
) -> anyhow::Result<()> {
    if let Some(mut path) = path.as_ref().parent() {
        for _ in 1..=relative_degree {
            match remove_dir(path).await {
                Ok(_) => (),
                Err(error)
                    if matches!(error.kind(), io::ErrorKind::NotFound | io::ErrorKind::Other) =>
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
pub async fn remove_all_files_in_directory<P: AsRef<Path>>(path: P) -> anyhow::Result<bool> {
    let mut result = false;

    let path = path.as_ref();

    for dir_entry in path.read_dir().with_context(|| anyhow!("{path:?}"))? {
        let dir_entry = dir_entry.with_context(|| anyhow!("{path:?}"))?;

        let file_type = match dir_entry.file_type() {
            Ok(file_type) => file_type,
            Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
            Err(error) => return Err(error).with_context(|| anyhow!("{dir_entry:?}")),
        };

        let path = dir_entry.path();

        if file_type.is_dir() {
            match remove_dir_all(&path).await {
                Ok(_) => result = true,
                Err(error) if error.kind() == io::ErrorKind::NotFound => {
                    result = true;

                    continue;
                },
                Err(error) => return Err(error).with_context(|| anyhow!("{path:?}")),
            }
        } else {
            match remove_file(&path).await {
                Ok(_) => result = true,
                Err(error) if error.kind() == io::ErrorKind::NotFound => {
                    result = true;

                    continue;
                },
                Err(error) => return Err(error).with_context(|| anyhow!("{path:?}")),
            }
        }
    }

    Ok(result)
}

/// Purge a cache with a specific key.
pub async fn remove_one_cache<P: AsRef<Path>, L: AsRef<str>, K: AsRef<str>, EK: AsRef<str>>(
    cache_path: P,
    levels: L,
    key: K,
    exclude_keys: Vec<EK>,
) -> anyhow::Result<AppResult> {
    let levels = parse_levels(levels)?;
    let number_of_levels = levels.len();

    let key = key.as_ref();

    for exclude_key in exclude_keys {
        let exclude_key = exclude_key.as_ref();

        if exclude_key.is_empty() && key.is_empty() {
            return Ok(AppResult::CacheIgnored);
        }

        let keys = parse_key(&exclude_key);

        if hit_key(key, &keys) {
            return Ok(AppResult::CacheIgnored);
        }
    }

    let file_path = create_cache_file_path(cache_path, levels, key);

    match remove_file(&file_path).await {
        Ok(_) => {
            remove_empty_ancestors(file_path, number_of_levels).await?;

            Ok(AppResult::Ok)
        },
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            Ok(AppResult::AlreadyPurged(file_path))
        },
        Err(error) => Err(error).with_context(|| anyhow!("{file_path:?}")),
    }
}

/// Purge multiple caches via wildcard.
pub async fn remove_caches_via_wildcard<
    P: AsRef<Path>,
    L: AsRef<str>,
    K: AsRef<str>,
    EK: AsRef<str>,
>(
    cache_path: P,
    levels: L,
    key: K,
    exclude_keys: Vec<EK>,
) -> anyhow::Result<AppResult> {
    #[async_recursion]
    async fn iterate(
        number_of_levels: usize,
        keys: Arc<Vec<Vec<u8>>>,
        exclude_key_keys: Arc<Vec<Vec<Vec<u8>>>>,
        exclude_paths: Arc<Mutex<Vec<PathBuf>>>,
        path: PathBuf,
        level: usize,
    ) -> anyhow::Result<bool> {
        let mut result = false;

        let mut tasks = Vec::new();

        for dir_entry in path.read_dir().with_context(|| anyhow!("{path:?}"))? {
            let dir_entry = dir_entry.with_context(|| anyhow!("{path:?}"))?;

            let file_type = match dir_entry.file_type() {
                Ok(file_type) => file_type,
                Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
                Err(error) => return Err(error).with_context(|| anyhow!("{dir_entry:?}")),
            };

            if number_of_levels == level {
                if file_type.is_file() {
                    let file_path = dir_entry.path();

                    {
                        let mut exclude_paths = exclude_paths.lock().await;

                        let exclude_paths_len = exclude_paths.len();

                        let mut i = 0;

                        let file_path = file_path.as_path();

                        while i < exclude_paths_len {
                            let exclude_path = &exclude_paths[i];

                            if exclude_path == file_path {
                                break;
                            }

                            i += 1;
                        }

                        if i != exclude_paths_len {
                            exclude_paths.remove(i);

                            continue;
                        }
                    }

                    tasks.push(tokio::spawn(match_key_and_remove_one_cache(
                        number_of_levels,
                        keys.clone(),
                        exclude_key_keys.clone(),
                        file_path,
                    )));
                }
            } else if file_type.is_dir() {
                tasks.push(tokio::spawn(iterate(
                    number_of_levels,
                    keys.clone(),
                    exclude_key_keys.clone(),
                    exclude_paths.clone(),
                    dir_entry.path(),
                    level + 1,
                )));
            }
        }

        for task in tasks {
            result = task.await.unwrap()? || result;
        }

        Ok(result)
    }

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

    let mut exclude_key_keys: Vec<Vec<Vec<u8>>> = Vec::new();
    let mut exclude_paths: Vec<PathBuf> = Vec::new();

    for exclude_key in exclude_keys {
        let exclude_key = exclude_key.as_ref();

        if exclude_key.contains('*') {
            let keys: Vec<Vec<u8>> =
                parse_key(&exclude_key).into_iter().map(|v| v.to_vec()).collect();

            if keys.len() == 1 && keys[0].is_empty() {
                return Ok(AppResult::AlreadyPurgedWildcard);
            }

            exclude_key_keys.push(keys);
        } else {
            let file_path = create_cache_file_path(cache_path.as_path(), &levels, exclude_key);

            exclude_paths.push(file_path);
        }
    }

    let keys = parse_key(&key);

    if keys.len() == 1
        && keys[0].is_empty()
        && exclude_key_keys.is_empty()
        && exclude_paths.is_empty()
    {
        return remove_all_files_in_directory(cache_path).await.map(|modified| {
            if modified {
                AppResult::Ok
            } else {
                AppResult::AlreadyPurgedWildcard
            }
        });
    }

    let keys = keys.into_iter().map(|v| v.to_vec()).collect::<Vec<Vec<u8>>>();

    iterate(
        number_of_levels,
        Arc::new(keys),
        Arc::new(exclude_key_keys),
        Arc::new(Mutex::new(exclude_paths)),
        cache_path,
        0,
    )
    .await
    .map(|modified| if modified { AppResult::Ok } else { AppResult::AlreadyPurgedWildcard })
}

fn hit_key<RK: AsRef<[u8]>, K: AsRef<[u8]>>(read_key: RK, keys: &[K]) -> bool {
    let read_key = read_key.as_ref();

    let mut p = 0;
    let mut i = 0;
    let read_key_len = read_key.len();
    let keys_len = keys.len();

    loop {
        let key = keys[i].as_ref();
        let key_len = key.len();

        if key_len == 0 {
            i += 1;

            if i == keys_len {
                break true;
            }

            let key = keys[i].as_ref();
            let key_len = key.len();
            debug_assert!(!key.is_empty());

            match read_key[p..].windows(key_len).position(|window| window == key).map(|i| i + p) {
                Some(index) => {
                    i += 1;

                    if i == keys_len {
                        break true;
                    }

                    p = index + key_len;
                },
                None => {
                    break false;
                },
            }
        } else if read_key_len - p < key_len {
            break false;
        } else {
            let e = p + key_len;

            if &read_key[p..e] == key {
                i += 1;

                if i == keys_len {
                    break true;
                }

                p = e;
            } else {
                break false;
            }
        }
    }
}

async fn match_key_and_remove_one_cache<P: AsRef<Path>>(
    number_of_levels: usize,
    keys: Arc<Vec<Vec<u8>>>,
    exclude_key_keys: Arc<Vec<Vec<Vec<u8>>>>,
    file_path: P,
) -> anyhow::Result<bool> {
    let file_path = file_path.as_ref();

    let mut sc: ScannerAscii<_, U384> =
        ScannerAscii::scan_path2(file_path).with_context(|| anyhow!("{file_path:?}"))?;

    // skip the header
    sc.drop_next_line().with_context(|| anyhow!("{file_path:?}"))?;

    // skip the label
    sc.drop_next_bytes("KEY: ".len()).with_context(|| anyhow!("{file_path:?}"))?;

    let read_key = sc
        .next_line_raw()
        .with_context(|| anyhow!("{file_path:?}"))?
        .ok_or(anyhow!("The content of {file_path:?} is incorrect."))?;

    // drop sc
    drop(sc);

    for exclude_key_key in exclude_key_keys.as_ref() {
        if hit_key(read_key.as_slice(), exclude_key_key) {
            return Ok(false);
        }
    }

    if hit_key(read_key, keys.as_ref()) {
        match remove_file(file_path).await {
            Ok(_) => (),
            Err(error) if error.kind() == io::ErrorKind::NotFound => (),
            Err(error) => return Err(error).with_context(|| anyhow!("{file_path:?}")),
        }

        remove_empty_ancestors(file_path, number_of_levels).await?;

        Ok(true)
    } else {
        Ok(false)
    }
}

fn parse_levels<L: AsRef<str>>(levels: L) -> anyhow::Result<Vec<usize>> {
    let levels: Vec<&str> = levels.as_ref().split(':').collect();

    if levels.len() > 3 {
        Err(anyhow!("The number of hierarchy levels cannot be bigger than 3."))
    } else {
        let number_of_levels = levels.len();

        let mut levels_usize = Vec::with_capacity(number_of_levels);

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
}

fn parse_key<K: AsRef<str>>(key: &K) -> Vec<&[u8]> {
    let key = key.as_ref().as_bytes();

    debug_assert!(!key.is_empty());

    let mut v = Vec::new();

    let mut p = 0;
    let key_len = key.len();

    loop {
        match key[p..].iter().cloned().position(|u| u == b'*').map(|i| i + p) {
            Some(i) => {
                if i == p {
                    // don't allow duplicated empty string to be added into Vec (when key is like foo**bar)
                    if v.is_empty() {
                        v.push([].as_slice());
                    }
                } else {
                    v.push(&key[p..i]);
                    v.push(&[]);
                }

                p = i + 1;

                if p >= key_len {
                    break;
                }
            },
            None => {
                v.push(&key[p..]);

                break;
            },
        }
    }

    v
}

fn create_cache_file_path<P: AsRef<Path>, L: AsRef<[usize]>, K: AsRef<str>>(
    cache_path: P,
    levels: L,
    key: K,
) -> PathBuf {
    let mut hasher = Md5::new();
    hasher.update(key.as_ref());

    let key_md5_value = u128::from_be_bytes(hasher.finalize().into());
    let hashed_key = format!("{:032x}", key_md5_value);

    let mut file_path = cache_path.as_ref().to_path_buf();
    let mut p = 32; // md5's hex string length

    for level in levels.as_ref() {
        file_path.push(&hashed_key[(p - level)..p]);

        p -= level;
    }

    file_path.push(hashed_key);

    file_path
}
