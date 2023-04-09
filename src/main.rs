use std::{env, error::Error};

use clap::{Arg, Command};
use concat_with::concat_line;
use nginx_cache_purge::*;
use terminal_size::terminal_size;

const APP_NAME: &str = "Nginx Cache Purge";
const CARGO_PKG_VERSION: &str = env!("CARGO_PKG_VERSION");
const CARGO_PKG_AUTHORS: &str = env!("CARGO_PKG_AUTHORS");

fn main() -> Result<(), Box<dyn Error>> {
    let matches = Command::new(APP_NAME)
        .term_width( terminal_size().map(|(width, _)| width.0 as usize).unwrap_or(0))
        .version(CARGO_PKG_VERSION)
        .author(CARGO_PKG_AUTHORS)
        .about(concat!("An alternative way to do proxy_cache_purge or fastcgi_cache_purge for Nginx.\n\nEXAMPLES:\n", concat_line!(prefix "nginx-cache-purge ",
                "/path/to/cache 1:2 http/blog/       # Purge the cache with the key \"http/blog/\" in the \"cache zone\" whose \"path\" is /path/to/cache, \"levels\" is 1:2",
                "/path/to/cache 1:1:1 http/blog*     # Purge the caches with the key which has \"http/blog\" as its prefix in the \"cache zone\" whose \"path\" is /path/to/cache, \"levels\" is 1:1:1",
                "/path/to/cache 2 *                  # Purge all caches in the \"cache zone\" whose \"path\" is /path/to/cache, \"levels\" is 1:1:1",
            )))
        .arg(Arg::new("CACHE_PATH")
            .required(true)
            .help("Assign the path set by proxy_cache_path or fastcgi_cache_path.")
        )
        .arg(Arg::new("LEVELS")
            .required(true)
            .help("Assign the levels set by proxy_cache_path or fastcgi_cache_path.")
        )
        .arg(Arg::new("KEY")
            .required(true)
            .help("Assign the key set by proxy_cache_key or fastcgi_cache_key.")
        )
        .after_help("Enjoy it! https://magiclen.org")
        .get_matches();

    let cache_path = matches.value_of("CACHE_PATH").unwrap();
    let levels = matches.value_of("LEVELS").unwrap();
    let key = matches.value_of("KEY").unwrap();

    if key.ends_with('*') {
        if key.len() == 1 {
            remove_all_files_in_directory(cache_path)?;
        } else {
            remove_caches_via_wildcard(cache_path, levels, key)?;
        }
    } else {
        remove_one_cache(cache_path, levels, key)?;
    }

    Ok(())
}
