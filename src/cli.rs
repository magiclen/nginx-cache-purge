use std::path::PathBuf;

use clap::{CommandFactory, FromArgMatches, Parser, Subcommand};
use concat_with::concat_line;
use terminal_size::terminal_size;

const APP_NAME: &str = "Nginx Cache Purge";
const CARGO_PKG_VERSION: &str = env!("CARGO_PKG_VERSION");
const CARGO_PKG_AUTHORS: &str = env!("CARGO_PKG_AUTHORS");

const AFTER_HELP: &str = "Enjoy it! https://magiclen.org";

const APP_ABOUT: &str = concat!(
    "An alternative way to do proxy_cache_purge or fastcgi_cache_purge for Nginx.\n\nEXAMPLES:\n",
    concat_line!(prefix "nginx-cache-purge ",
        "p /path/to/cache 1:2 http/blog/             # Purge the cache with the key \"http/blog/\" in the \"cache zone\" whose \"path\" is /path/to/cache, \"levels\" is 1:2",
        "p /path/to/cache 1:1:1 'http/blog*'         # Purge the caches with the key which has \"http/blog\" as its prefix in the \"cache zone\" whose \"path\" is /path/to/cache, \"levels\" is 1:1:1",
        "p /path/to/cache 2:1 '*/help*'              # Purge the caches with the key which contains the substring \"/help\" in the \"cache zone\" whose \"path\" is /path/to/cache, \"levels\" is 2:1",
        "p /path/to/cache '' 'http/blog*'            # Purge the caches in the \"cache zone\" whose \"path\" is /path/to/cache and which has no \"levels\"",
        "p /path/to/cache 1 '*'                      # Purge all caches in the \"cache zone\" whose \"path\" is /path/to/cache, \"levels\" is 1",
        "p /path/to/cache 2 '*' -e 'http/static/*'   # Purge all caches except for those whose key starts with \"http/static/\" in the \"cache zone\" whose \"path\" is /path/to/cache, \"levels\" is 2",
        "p /path/to/cache 1:2 '*' --dry-run          # List the caches that would be purged without removing anything",
        "s                                           # Start a server which listens on \"/tmp/nginx-cache-purge.sock\" to handle purge requests",
        "s /run/nginx-cache-purge.sock               # Start a server which listens on \"/run/nginx-cache-purge.sock\" to handle purge requests",
        "s --zone my_cache /path/to/cache 1:2        # Start a server which only accepts purge requests for the \"my_cache\" zone",
    )
);

#[derive(Debug, Parser)]
#[command(name = APP_NAME)]
#[command(term_width = terminal_size().map(|(width, _)| width.0 as usize).unwrap_or(0))]
#[command(version = CARGO_PKG_VERSION)]
#[command(author = CARGO_PKG_AUTHORS)]
#[command(after_help = AFTER_HELP)]
pub struct CLIArgs {
    #[command(subcommand)]
    pub command: CLICommands,
}

#[derive(Debug, Subcommand)]
pub enum CLICommands {
    #[command(visible_alias = "p")]
    #[command(about = "Purge the cache immediately")]
    #[command(after_help = AFTER_HELP)]
    Purge {
        #[arg(value_hint = clap::ValueHint::DirPath)]
        #[arg(help = "Assign the path set by proxy_cache_path or fastcgi_cache_path")]
        cache_path: PathBuf,

        #[arg(help = "Assign the levels set by proxy_cache_path or fastcgi_cache_path (can be \
                      empty if it is not set)")]
        levels: String,

        #[arg(help = "Assign the key set by proxy_cache_key or fastcgi_cache_key")]
        key: String,

        #[arg(short, long, visible_alias = "exclude-key")]
        #[arg(help = "Assign a key that should be excluded (can be used multiple times)")]
        exclude_keys: Vec<String>,

        #[arg(long)]
        #[arg(help = "Show what would be purged without removing anything")]
        dry_run: bool,
    },
    #[cfg(feature = "service")]
    #[command(visible_alias = "s")]
    #[command(about = "Start a server to handle purge requests")]
    #[command(after_help = AFTER_HELP)]
    Start {
        #[arg(default_value = "/tmp/nginx-cache-purge.sock")]
        #[arg(value_hint = clap::ValueHint::FilePath)]
        socket_file_path: PathBuf,

        #[arg(long = "zone", num_args = 3)]
        #[arg(value_names = ["NAME", "PATH", "LEVELS"])]
        #[arg(help = "Define a cache zone that purge requests may refer to by name (can be used \
                      multiple times). Once any zone is defined, requests are no longer allowed \
                      to name a cache path themselves")]
        zones: Vec<String>,

        #[arg(long)]
        #[arg(help = "Show what would be purged without removing anything")]
        dry_run: bool,
    },
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
