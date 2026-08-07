mod cli;
mod functions;
#[cfg(feature = "service")]
mod server;
#[cfg(feature = "service")]
mod uds_serve;

use std::{
    path::{Path, PathBuf},
    process::{ExitCode, Termination},
};

use cli::*;
#[cfg(feature = "service")]
use server::*;

#[derive(Debug, PartialEq, Eq)]
pub enum AppResult {
    Ok,
    AlreadyPurged(PathBuf),
    CacheIgnored,
    AlreadyPurgedWildcard,
}

impl Termination for AppResult {
    #[inline]
    fn report(self) -> ExitCode {
        let exit_code = match self {
            AppResult::Ok => 0u8,
            AppResult::AlreadyPurged(file_path) => {
                eprintln!("Hint: {file_path:?} does not exist");

                44
            },
            AppResult::CacheIgnored => {
                eprintln!("Warning: cache is excluded from being purged");

                44
            },
            AppResult::AlreadyPurgedWildcard => 44,
        };

        ExitCode::from(exit_code)
    }
}

#[inline]
fn purge<P: AsRef<Path>>(
    cache_path: P,
    levels: &[usize],
    key: &str,
    exclude_keys: &[&str],
) -> anyhow::Result<AppResult> {
    if key.contains('*') {
        functions::remove_caches_via_wildcard(cache_path, levels, key, exclude_keys)
    } else {
        functions::remove_one_cache(cache_path, levels, key, exclude_keys)
    }
}

fn main() -> anyhow::Result<AppResult> {
    let args = get_args();

    match &args.command {
        CLICommands::Purge {
            cache_path,
            levels,
            key,
            exclude_keys,
            dry_run,
        } => {
            functions::set_dry_run(*dry_run);

            let levels = functions::parse_levels(levels)?;

            purge(
                cache_path,
                &levels,
                key,
                &exclude_keys.iter().map(|s| s.as_str()).collect::<Vec<&str>>(),
            )
        },
        #[cfg(feature = "service")]
        CLICommands::Start {
            socket_file_path,
            zones,
            dry_run,
        } => {
            functions::set_dry_run(*dry_run);

            let zones = parse_zones(zones)?;

            tokio::runtime::Runtime::new()?.block_on(server_main(socket_file_path.as_path(), zones))
        },
    }
}
