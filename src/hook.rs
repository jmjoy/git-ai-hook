use crate::config::{discover_repository, load_effective};
use crate::diff::collect_staged_changes;
use crate::install::{hook_paths, run_backup_hook};
use crate::llm::generate_message;
use crate::logging::log_hook_error;
use crate::prompt::{assemble_message_file, is_non_comment_content};
use anyhow::{Context, Result};
use std::env;
use std::ffi::OsString;
use std::fs;
use std::path::PathBuf;

pub const SKIP_ENV: &str = "GIT_AI_HOOK_SKIP";

pub fn run_hook(arguments: &[OsString]) -> Result<i32> {
    match run_hook_inner(arguments) {
        Ok(code) => Ok(code),
        Err(error) => {
            log_hook_error("prepare-commit-msg", &error);
            Ok(0)
        }
    }
}

fn run_hook_inner(arguments: &[OsString]) -> Result<i32> {
    let repository = discover_repository()?;
    let paths = hook_paths(&repository)?;
    if let Some(code) = run_backup_hook(&paths, arguments)? {
        return Ok(code);
    }

    let message_path = message_path(arguments)?;
    let source = arguments.get(1).and_then(|argument| argument.to_str());
    if source == Some("message") || skip_requested() {
        return Ok(0);
    }

    let config = load_effective(Some(&repository))?;
    if !config.api_key_present() {
        return Ok(0);
    }

    let original = fs::read_to_string(&message_path).with_context(|| {
        format!(
            "could not read commit message file {}",
            message_path.display()
        )
    })?;
    if is_non_comment_content(&original) {
        return Ok(0);
    }

    let changes = collect_staged_changes(&repository)?;
    let runtime = tokio::runtime::Runtime::new().context("could not start the async runtime")?;
    let (message, prompts) = runtime.block_on(generate_message(&config, &changes))?;
    let assembled = assemble_message_file(&message, config.debug.then_some(&prompts), &original);
    fs::write(&message_path, assembled).with_context(|| {
        format!(
            "could not write commit message file {}",
            message_path.display()
        )
    })?;
    Ok(0)
}

fn message_path(arguments: &[OsString]) -> Result<PathBuf> {
    let argument = arguments
        .first()
        .context("prepare-commit-msg did not provide a commit message file")?;
    Ok(PathBuf::from(argument))
}

fn skip_requested() -> bool {
    env::var(SKIP_ENV).ok().as_deref() == Some("1")
}
