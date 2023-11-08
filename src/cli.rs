use std::path::PathBuf;

use clap::{CommandFactory, FromArgMatches, Parser};
use concat_with::concat_line;
use terminal_size::terminal_size;

const APP_NAME: &str = "Nginx Cache Purge";
const CARGO_PKG_VERSION: &str = env!("CARGO_PKG_VERSION");
const CARGO_PKG_AUTHORS: &str = env!("CARGO_PKG_AUTHORS");

const AFTER_HELP: &str = "Enjoy it! https://magiclen.org";

const APP_ABOUT: &str = concat!(
    "An alternative way to do proxy_cache_purge or fastcgi_cache_purge for Nginx.\n\nEXAMPLES:\n",
    concat_line!(prefix "nginx-cache-purge ",
        "/path/to/cache 1:2 http/blog/       # Purge the cache with the key \"http/blog/\" in the \"cache zone\" whose \"path\" is /path/to/cache, \"levels\" is 1:2",
        "/path/to/cache 1:1:1 http/blog*     # Purge the caches with the key which has \"http/blog\" as its prefix in the \"cache zone\" whose \"path\" is /path/to/cache, \"levels\" is 1:1:1",
        "/path/to/cache 2 *                  # Purge all caches in the \"cache zone\" whose \"path\" is /path/to/cache, \"levels\" is 1:1:1",
    )
);

#[derive(Debug, Parser)]
#[command(name = APP_NAME)]
#[command(term_width = terminal_size().map(|(width, _)| width.0 as usize).unwrap_or(0))]
#[command(version = CARGO_PKG_VERSION)]
#[command(author = CARGO_PKG_AUTHORS)]
#[command(after_help = AFTER_HELP)]
pub struct CLIArgs {
    #[arg(help = "Assign the path set by proxy_cache_path or fastcgi_cache_path")]
    pub cache_path: PathBuf,

    #[arg(help = "Assign the levels set by proxy_cache_path or fastcgi_cache_path")]
    pub levels: String,

    #[arg(help = "Assign the key set by proxy_cache_key or fastcgi_cache_key")]
    pub key: String,
}

pub fn get_args() -> CLIArgs {
    let args = CLIArgs::command();

    let about = format!("{APP_NAME} {CARGO_PKG_VERSION}\n{CARGO_PKG_AUTHORS}\n{APP_ABOUT}");

    let args = args.about(about);

    let matches = args.get_matches();

    match CLIArgs::from_arg_matches(&matches) {
        Ok(args) => args,
        Err(err) => {
            err.exit();
        },
    }
}
