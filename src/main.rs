#[macro_use]
extern crate concat_with;
extern crate clap;
extern crate terminal_size;

extern crate nginx_cache_purge;

extern crate hex;
extern crate md5;

use std::env;

use clap::{App, Arg};
use terminal_size::terminal_size;

use nginx_cache_purge::*;

const APP_NAME: &str = "Nginx Cache Purge";
const CARGO_PKG_VERSION: &str = env!("CARGO_PKG_VERSION");
const CARGO_PKG_AUTHORS: &str = env!("CARGO_PKG_AUTHORS");

fn main() -> Result<(), String> {
    let matches = App::new(APP_NAME)
        .set_term_width( terminal_size().map(|(width, _)| width.0 as usize).unwrap_or(0))
        .version(CARGO_PKG_VERSION)
        .author(CARGO_PKG_AUTHORS)
        .about(concat!("An alternative way to do proxy_cache_purge or fastcgi_cache_purge for Nginx.\n\nEXAMPLES:\n", concat_line!(prefix "nginx-cache-purge ",
                "/path/to/cache 1:2 http/blog/     # Purges the cache with \"KEY: http/blog/\" in the \"cache zone\" whose \"path\" is /path/to/cache, \"levels\" is 1:2",
            )))
        .arg(Arg::with_name("CACHE_PATH")
            .required(true)
            .help("Assigns the path set by proxy_cache_path or fastcgi_cache_path.")
        )
        .arg(Arg::with_name("LEVELS")
            .required(true)
            .help("Assigns the levels set by proxy_cache_path or fastcgi_cache_path.")
        )
        .arg(Arg::with_name("KEY")
            .required(true)
            .help("Assigns the key set by proxy_cache_key or fastcgi_cache_key.")
        )
        .after_help("Enjoy it! https://magiclen.org")
        .get_matches();

    let cache_path = matches.value_of("CACHE_PATH").unwrap();
    let levels = matches.value_of("LEVELS").unwrap();
    let key = matches.value_of("KEY").unwrap();

    if key.ends_with('*') {
        remove_caches_via_wildcard(cache_path, levels, key).map_err(|err| err.to_string())?;
    } else {
        remove_one_cache(cache_path, levels, key).map_err(|err| err.to_string())?;
    }

    Ok(())
}
