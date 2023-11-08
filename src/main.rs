mod cli;
mod functions;

use std::process::{ExitCode, Termination};

use cli::*;

#[derive(Debug)]
pub enum AppResult {
    Ok            = 0,
    AlreadyPurged = 44,
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
        ExitCode::from(self as u8)
    }
}

fn main() -> anyhow::Result<AppResult> {
    let args = get_args();

    if args.key.ends_with('*') {
        if args.key.len() == 1 {
            functions::remove_all_files_in_directory(args.cache_path).map(|ok| ok.into())
        } else {
            functions::remove_caches_via_wildcard(args.cache_path, args.levels, args.key)
                .map(|ok| ok.into())
        }
    } else {
        functions::remove_one_cache(args.cache_path, args.levels, args.key)
    }
}
