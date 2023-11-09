use std::{io, path::Path};

use anyhow::{anyhow, Context};
use md5::{Digest, Md5};
use scanner_rust::{generic_array::typenum::U384, ScannerAscii};

use crate::AppResult;

#[inline]
fn remove_file<P: AsRef<Path>>(path: P) -> io::Result<()> {
    let path = path.as_ref();

    if cfg!(debug_assertions) {
        println!("Remove file: {path:?}");

        Ok(())
    } else {
        std::fs::remove_file(path)
    }
}

#[inline]
fn remove_dir_all<P: AsRef<Path>>(path: P) -> io::Result<()> {
    let path = path.as_ref();

    if cfg!(debug_assertions) {
        println!("Remove dir all: {path:?}");

        Ok(())
    } else {
        std::fs::remove_dir_all(path)
    }
}

#[inline]
fn remove_dir<P: AsRef<Path>>(path: P) -> io::Result<()> {
    let path = path.as_ref();

    if cfg!(debug_assertions) {
        println!("Remove dir: {path:?}");
    } else {
        match std::fs::remove_dir(path) {
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

fn remove_empty_ancestors<P: AsRef<Path>>(path: P, relative_degree: usize) -> anyhow::Result<()> {
    if let Some(mut path) = path.as_ref().parent() {
        for _ in 1..=relative_degree {
            match remove_dir(path) {
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
pub fn remove_all_files_in_directory<P: AsRef<Path>>(path: P) -> anyhow::Result<()> {
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
            match remove_dir_all(&path) {
                Ok(_) => (),
                Err(error) if error.kind() == io::ErrorKind::NotFound => {
                    continue;
                },
                Err(error) => return Err(error).with_context(|| anyhow!("{path:?}")),
            }
        } else {
            match remove_file(&path) {
                Ok(_) => (),
                Err(error) if error.kind() == io::ErrorKind::NotFound => {
                    continue;
                },
                Err(error) => return Err(error).with_context(|| anyhow!("{path:?}")),
            }
        }
    }

    Ok(())
}

/// Purge a cache with a specific key.
pub fn remove_one_cache<P: AsRef<Path>, L: AsRef<str>, K: AsRef<str>>(
    cache_path: P,
    levels: L,
    key: K,
) -> anyhow::Result<AppResult> {
    let levels = parse_levels_stage_1(&levels)?;
    let number_of_levels = levels.len();

    let mut levels_usize = Vec::with_capacity(number_of_levels);

    for level in levels {
        let level_usize = level
            .parse::<usize>()
            .with_context(|| anyhow!("The value of levels should be an integer."))?;

        if !(1..=2).contains(&level_usize) {
            return Err(anyhow!("The value of levels should be 1 or 2."));
        }

        levels_usize.push(level_usize);
    }

    let mut hasher = Md5::new();
    hasher.update(key.as_ref());

    let key_md5_value = u128::from_be_bytes(hasher.finalize().into());
    let hashed_key = format!("{:032x}", key_md5_value);

    let mut file_path = cache_path.as_ref().to_path_buf();
    let mut p = 32; // md5's hex string length

    for level_usize in levels_usize {
        file_path.push(&hashed_key[(p - level_usize)..p]);

        p -= level_usize;
    }

    file_path.push(hashed_key);

    match remove_file(&file_path) {
        Ok(_) => {
            remove_empty_ancestors(file_path, number_of_levels)?;

            Ok(AppResult::Ok)
        },
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            Ok(AppResult::AlreadyPurged(file_path))
        },
        Err(error) => Err(error).with_context(|| anyhow!("{file_path:?}")),
    }
}

/// Purge multiple caches via wildcard.
pub fn remove_caches_via_wildcard<P: AsRef<Path>, L: AsRef<str>, K: AsRef<str>>(
    cache_path: P,
    levels: L,
    key: K,
) -> anyhow::Result<()> {
    fn iterate(levels: &[&str], key: &str, path: &Path, level: usize) -> anyhow::Result<()> {
        let number_of_levels = levels.len();

        for dir_entry in path.read_dir().with_context(|| anyhow!("{path:?}"))? {
            let dir_entry = dir_entry.with_context(|| anyhow!("{path:?}"))?;

            let file_type = match dir_entry.file_type() {
                Ok(file_type) => file_type,
                Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
                Err(error) => return Err(error).with_context(|| anyhow!("{dir_entry:?}")),
            };

            if number_of_levels == level {
                if file_type.is_file() {
                    match_key_and_remove_one_cache(dir_entry.path(), key, number_of_levels)?;
                }
            } else if file_type.is_dir() {
                iterate(levels, key, dir_entry.path().as_path(), level + 1)?;
            }
        }

        Ok(())
    }

    let levels = parse_levels_stage_1(&levels)?;

    let key = key.as_ref();
    let key = &key[..(key.len() - 1)];

    let path = cache_path.as_ref();

    iterate(&levels, key, path, 0)
}

fn match_key_and_remove_one_cache<P: AsRef<Path>, K: AsRef<str>>(
    file_path: P,
    key: K,
    number_of_levels: usize,
) -> anyhow::Result<()> {
    let file_path = file_path.as_ref();
    let key = key.as_ref();

    let mut sc: ScannerAscii<_, U384> = ScannerAscii::scan_path2(file_path)?;

    // skip the header
    sc.drop_next_line()?;

    // skip the label
    sc.drop_next_bytes("KEY: ".len())?;

    let read_key = sc
        .next_line_raw()
        .with_context(|| anyhow!("{file_path:?}"))?
        .ok_or(anyhow!("The content of {file_path:?} is incorrect."))?;

    if read_key.starts_with(key.as_bytes()) {
        match remove_file(file_path) {
            Ok(_) => (),
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                return Ok(());
            },
            Err(error) => return Err(error).with_context(|| anyhow!("{file_path:?}")),
        }

        remove_empty_ancestors(file_path, number_of_levels)?;
    }

    Ok(())
}

#[inline]
fn parse_levels_stage_1<'a, L: ?Sized + AsRef<str>>(levels: &'a L) -> anyhow::Result<Vec<&'a str>> {
    let levels: Vec<&'a str> = levels.as_ref().split(':').collect();

    if levels.len() > 3 {
        Err(anyhow!("The number of hierarchy levels cannot be bigger than 3."))
    } else {
        Ok(levels)
    }
}
