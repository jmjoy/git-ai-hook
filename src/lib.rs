#![cfg_attr(not(any(target_os = "linux", target_os = "macos")), allow(dead_code))]

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
compile_error!("git-ai-hook supports Linux and macOS only; Windows is not supported.");

mod cli;
mod config;
mod diff;
mod hook;
mod install;
mod llm;
mod logging;
mod prompt;

use anyhow::Result;
use std::ffi::OsString;
use std::path::Path;

pub fn run() -> Result<i32> {
    let args = std::env::args_os().collect::<Vec<_>>();
    let argv0 = args
        .first()
        .cloned()
        .unwrap_or_else(|| OsString::from("git-ai-hook"));

    if is_hook_invocation(&argv0) {
        return hook::run_hook(&args[1..]);
    }

    cli::run_cli(args)
}

fn is_hook_invocation(argv0: &OsString) -> bool {
    Path::new(argv0)
        .file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name == install::HOOK_NAME)
}
