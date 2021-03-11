//! # Nginx Cache Purge
//! An alternative way to do `proxy_cache_purge` or `fastcgi_cache_purge` for Nginx.

extern crate md5;
extern crate scanner_rust;

use std::io::{self, ErrorKind};
use std::path::Path;

#[cfg(not(debug_assertions))]
use std::fs;

use scanner_rust::generic_array::typenum::U384;
use scanner_rust::{ScannerAscii, ScannerError};

#[cfg(debug_assertions)]
macro_rules! remove_file {
    ($path:expr) => {
        println!("Remove file: {}", $path.to_string_lossy());
    };
    ($path:expr, $not_found:block) => {
        println!("Remove file: {}", $path.to_string_lossy());
    };
}

#[cfg(not(debug_assertions))]
macro_rules! remove_file {
    ($path:expr) => {
        fs::remove_file($path)?
    };
    ($path:expr, $not_found:block) => {
        match fs::remove_file($path) {
            Ok(_) => (),
            Err(ref err) if err.kind() == ErrorKind::NotFound => $not_found,
            Err(err) => return Err(err.into()),
        }
    };
}

#[cfg(debug_assertions)]
macro_rules! remove_dir_all {
    ($path:expr) => {
        println!("Remove dir all: {}", $path.to_string_lossy());
    };
    ($path:expr, $not_found:block) => {
        println!("Remove dir all: {}", $path.to_string_lossy());
    };
}

#[cfg(not(debug_assertions))]
macro_rules! remove_dir_all {
    ($path:expr) => {
        fs::remove_dir_all($path)?
    };
    ($path:expr, $not_found:block) => {
        match fs::remove_dir_all($path) {
            Ok(_) => (),
            Err(ref err) if err.kind() == ErrorKind::NotFound => $not_found,
            Err(err) => return Err(err.into()),
        }
    };
}

#[cfg(debug_assertions)]
macro_rules! remove_dir {
    ($path:expr) => {
        println!("Remove dir: {}", $path.to_string_lossy());
    };
    ($path:expr, $not_found_other:block) => {
        println!("Remove dir: {}", $path.to_string_lossy());
    };
}

#[cfg(not(debug_assertions))]
macro_rules! remove_dir {
    ($path:expr) => {
        fs::remove_dir($path)?
    };
    ($path:expr, $not_found_other:block) => {
        match fs::remove_dir($path) {
            Ok(_) => (),
            Err(ref err) if err.kind() == ErrorKind::NotFound || err.kind() == ErrorKind::Other => {
                $not_found_other
            }
            Err(err) => return Err(err.into()),
        }
    };
}

/// Do something like `rm -rf /path/to/*`. The `/path/to` directory will not be deleted. This function may be dangerous.
pub fn remove_all_files_in_directory<P: AsRef<Path>>(directory: P) -> Result<(), io::Error> {
    let directory = directory.as_ref();

    for dir_entry in directory.read_dir()? {
        let dir_entry = dir_entry?;

        let file_type = match dir_entry.file_type() {
            Ok(file_type) => file_type,
            Err(ref err) if err.kind() == ErrorKind::NotFound => continue,
            Err(err) => return Err(err),
        };

        let path = dir_entry.path();

        if file_type.is_dir() {
            remove_dir_all!(path, { continue });
        } else {
            remove_file!(path, { continue });
        }
    }

    Ok(())
}

/// Purge a cache with a specific key.
pub fn remove_one_cache<P: AsRef<Path>, L: AsRef<str>, K: AsRef<str>>(
    cache_path: P,
    levels: L,
    key: K,
) -> Result<(), io::Error> {
    let levels = parse_levels_stage_1(&levels)?;
    let number_of_levels = levels.len();

    let mut levels_usize = Vec::with_capacity(number_of_levels);

    for level in levels {
        let level_usize = level.parse::<usize>().map_err(|_| {
            io::Error::new(ErrorKind::InvalidInput, "The value of levels should be an integer.")
        })?;

        if !(1..=2).contains(&level_usize) {
            return Err(io::Error::new(
                ErrorKind::InvalidInput,
                "The value of levels should be 1 or 2.",
            ));
        }

        levels_usize.push(level_usize);
    }

    let key_md5_digest = md5::compute(key.as_ref());
    let key_md5_value = u128::from_be_bytes(key_md5_digest.0);
    let hashed_key = format!("{:032x}", key_md5_value);

    let mut file_path = cache_path.as_ref().to_path_buf();
    let mut p = 32; // md5's hex string length

    for level_usize in levels_usize {
        file_path.push(&hashed_key[(p - level_usize)..p]);

        p -= level_usize;
    }

    file_path.push(hashed_key);

    let file_path = file_path.as_path();

    remove_file!(file_path);

    let mut path = file_path.parent().unwrap();

    for _ in 0..number_of_levels {
        remove_dir!(path, { return Ok(()) });
        path = path.parent().unwrap();
    }

    Ok(())
}

/// Purge multiple caches via wildcard.
pub fn remove_caches_via_wildcard<P: AsRef<Path>, L: AsRef<str>, K: AsRef<str>>(
    cache_path: P,
    levels: L,
    key: K,
) -> Result<(), io::Error> {
    let levels = parse_levels_stage_1(&levels)?;

    let number_of_levels = levels.len();

    let cache_path = cache_path.as_ref();
    let key = key.as_ref();
    let key = &key[..(key.len() - 1)];

    for dir_entry in cache_path.read_dir()? {
        // 0

        let dir_entry = dir_entry?;

        let file_type = match dir_entry.file_type() {
            Ok(file_type) => file_type,
            Err(ref err) if err.kind() == ErrorKind::NotFound => continue,
            Err(err) => return Err(err),
        };

        if file_type.is_dir() {
            for dir_entry in cache_path.read_dir()? {
                // 1
                let dir_entry = dir_entry?;

                let file_type = match dir_entry.file_type() {
                    Ok(file_type) => file_type,
                    Err(ref err) if err.kind() == ErrorKind::NotFound => continue,
                    Err(err) => return Err(err),
                };

                if file_type.is_dir() {
                    for dir_entry in dir_entry.path().read_dir()? {
                        // 2
                        let dir_entry = dir_entry?;

                        let file_type = match dir_entry.file_type() {
                            Ok(file_type) => file_type,
                            Err(ref err) if err.kind() == ErrorKind::NotFound => continue,
                            Err(err) => return Err(err),
                        };

                        if number_of_levels == 1 {
                            if file_type.is_file() {
                                match_key_and_remove_one_cache(
                                    dir_entry.path(),
                                    key,
                                    number_of_levels,
                                )
                                .map_err(|_| io::Error::from(ErrorKind::InvalidData))?;
                            }
                        } else if file_type.is_dir() {
                            for dir_entry in dir_entry.path().read_dir()? {
                                // 3
                                let dir_entry = dir_entry?;

                                let file_type = match dir_entry.file_type() {
                                    Ok(file_type) => file_type,
                                    Err(ref err) if err.kind() == ErrorKind::NotFound => continue,
                                    Err(err) => return Err(err),
                                };

                                if number_of_levels == 2 {
                                    if file_type.is_file() {
                                        match_key_and_remove_one_cache(
                                            dir_entry.path(),
                                            key,
                                            number_of_levels,
                                        )
                                        .map_err(|_| io::Error::from(ErrorKind::InvalidData))?;
                                    }
                                } else if file_type.is_dir() {
                                    for dir_entry in dir_entry.path().read_dir()? {
                                        // 4
                                        let dir_entry = dir_entry?;

                                        let file_type = match dir_entry.file_type() {
                                            Ok(file_type) => file_type,
                                            Err(ref err) if err.kind() == ErrorKind::NotFound => {
                                                continue;
                                            }
                                            Err(err) => return Err(err),
                                        };

                                        if file_type.is_file() {
                                            match_key_and_remove_one_cache(
                                                dir_entry.path(),
                                                key,
                                                number_of_levels,
                                            )
                                            .map_err(|_| io::Error::from(ErrorKind::InvalidData))?;
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    Ok(())
}

fn match_key_and_remove_one_cache<P: AsRef<Path>, K: AsRef<str>>(
    file_path: P,
    key: K,
    number_of_levels: usize,
) -> Result<(), ScannerError> {
    let file_path = file_path.as_ref();
    let key = key.as_ref();

    let mut sc: ScannerAscii<_, U384> = ScannerAscii::scan_path2(file_path)?;

    // skip the header
    sc.drop_next_line()?;

    // skip the label
    sc.drop_next_bytes("KEY: ".len())?;

    let read_key = sc.next_line_raw()?.ok_or(ErrorKind::InvalidData)?;

    if read_key.starts_with(key.as_bytes()) {
        remove_file!(file_path, { return Ok(()) });

        let mut path = file_path.parent().unwrap();

        for _ in 0..number_of_levels {
            remove_dir!(path, { return Ok(()) });
            path = path.parent().unwrap();
        }
    }

    Ok(())
}

#[inline]
fn parse_levels_stage_1<'a, L: ?Sized + AsRef<str>>(
    levels: &'a L,
) -> Result<Vec<&'a str>, io::Error> {
    let levels: Vec<&'a str> = levels.as_ref().split(':').collect();

    if levels.len() > 3 {
        Err(io::Error::new(
            ErrorKind::InvalidInput,
            "The number of hierarchy levels cannot be bigger than 3.",
        ))
    } else {
        Ok(levels)
    }
}
