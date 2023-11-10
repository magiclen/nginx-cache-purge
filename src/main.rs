mod cli;
mod functions;
#[cfg(feature = "service")]
mod server;

use std::{
    path::{Path, PathBuf},
    process::{ExitCode, Termination},
};

use cli::*;
#[cfg(feature = "service")]
use server::*;

#[derive(Debug)]
pub enum AppResult {
    Ok,
    AlreadyPurged(PathBuf),
    AlreadyPurgedWildcard,
}

impl From<()> for AppResult {
    #[inline]
    fn from(_: ()) -> Self {
        AppResult::Ok
    }
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
            AppResult::AlreadyPurgedWildcard => 44,
        };

        ExitCode::from(exit_code)
    }
}

#[inline]
fn purge<P: AsRef<Path>, L: AsRef<str>, K: AsRef<str>>(
    cache_path: P,
    levels: L,
    key: K,
) -> anyhow::Result<AppResult> {
    let cache_path = cache_path.as_ref();
    let levels = levels.as_ref();
    let key = key.as_ref();

    if key.contains('*') {
        functions::remove_caches_via_wildcard(cache_path, levels, key)
    } else {
        functions::remove_one_cache(cache_path, levels, key)
    }
}

fn main() -> anyhow::Result<AppResult> {
    let args = get_args();

    match &args.command {
        CLICommands::Purge {
            cache_path,
            levels,
            key,
        } => purge(cache_path, levels, key),
        #[cfg(feature = "service")]
        CLICommands::Start {
            socket_file_path,
        } => server_main(socket_file_path.as_path()),
    }
}
