use crate::config::{
    discover_repository, discover_repository_optional, global_config_path, load_effective,
    project_config_path, validate_for_request, write_global_template,
};
use crate::diff::collect_staged_changes;
use crate::install::{self, InstallResult, UninstallResult};
use crate::llm::{generate_message, generate_raw_message, validate_connection};
use crate::prompt::assemble_message_file;
use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use std::ffi::OsString;
use std::fs;

#[derive(Debug, Parser)]
#[command(
    name = "git-ai-hook",
    version,
    about = "Generate Git commit messages with an OpenAI-compatible API"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Install the prepare-commit-msg hook for this repository.
    Install,
    /// Remove this tool's hook and restore a backed-up hook when present.
    Uninstall,
    /// Show hook and effective configuration status.
    Status,
    /// Generate a commit message from the staged diff and print it.
    Generate {
        /// Print the model response without cleanup or fallback for invalid output.
        #[arg(long)]
        no_fallback: bool,
    },
    /// Inspect or initialize configuration.
    Config {
        #[command(subcommand)]
        command: ConfigCommand,
    },
}

#[derive(Debug, Subcommand)]
enum ConfigCommand {
    /// Print the global configuration file path.
    Path,
    /// Create the global configuration template.
    Init {
        /// Replace an existing global configuration file.
        #[arg(long)]
        force: bool,
    },
    /// Validate the effective config and make a minimal model request.
    Validate,
}

pub fn run_cli(arguments: Vec<OsString>) -> Result<i32> {
    let cli = Cli::try_parse_from(arguments)?;
    match cli.command {
        Command::Install => install_command(),
        Command::Uninstall => uninstall_command(),
        Command::Status => status_command(),
        Command::Generate { no_fallback } => generate_command(no_fallback),
        Command::Config { command } => config_command(command),
    }
}

fn install_command() -> Result<i32> {
    let repository = discover_repository()?;
    let (paths, result) = install::install(&repository)?;
    match result {
        InstallResult::Installed => println!("Installed hook at {}", paths.hook.display()),
        InstallResult::Refreshed => println!("Refreshed hook at {}", paths.hook.display()),
    }
    if paths.backup.exists() {
        println!("Chained backup hook: {}", paths.backup.display());
    }
    Ok(0)
}

fn uninstall_command() -> Result<i32> {
    let repository = discover_repository()?;
    let (paths, result) = install::uninstall(&repository)?;
    match result {
        UninstallResult::RestoredBackup => {
            println!("Restored hook backup to {}", paths.hook.display())
        }
        UninstallResult::RemovedManagedHook => println!("Removed hook {}", paths.hook.display()),
        UninstallResult::NotInstalled => println!("git-ai-hook is not installed"),
    }
    Ok(0)
}

fn generate_command(no_fallback: bool) -> Result<i32> {
    let repository = discover_repository()?;
    let config = load_effective(Some(&repository))?;
    validate_for_request(&config)?;
    let changes = collect_staged_changes(&repository)?;
    let runtime = tokio::runtime::Runtime::new().context("could not start the async runtime")?;
    let (message, prompts) = if no_fallback {
        runtime.block_on(generate_raw_message(&config, &changes))?
    } else {
        runtime.block_on(generate_message(&config, &changes))?
    };
    let output = assemble_message_file(&message, config.debug.then_some(&prompts), "");
    print!("{output}");
    Ok(0)
}

fn config_command(command: ConfigCommand) -> Result<i32> {
    match command {
        ConfigCommand::Path => {
            println!("{}", global_config_path()?.display());
            Ok(0)
        }
        ConfigCommand::Init { force } => {
            let path = write_global_template(force)?;
            println!("Wrote configuration template to {}", path.display());
            Ok(0)
        }
        ConfigCommand::Validate => {
            let repository = discover_repository_optional();
            let config = load_effective(repository.as_ref())?;
            validate_for_request(&config)?;
            let runtime =
                tokio::runtime::Runtime::new().context("could not start the async runtime")?;
            runtime.block_on(validate_connection(&config))?;
            println!("Configuration is valid and the configured model responded.");
            Ok(0)
        }
    }
}

fn status_command() -> Result<i32> {
    let repository = discover_repository_optional();
    let config = load_effective(repository.as_ref())?;

    println!("Global config: {}", global_config_path()?.display());
    match repository.as_ref() {
        Some(repository) => {
            println!("Repository: {}", repository.path().display());
            if let Some(project) = project_config_path(repository) {
                let state = if project.exists() {
                    "present"
                } else {
                    "absent"
                };
                println!("Project config: {} ({state})", project.display());
            }
            let paths = install::hook_paths(repository)?;
            let hook_state = if install::is_managed_hook(&paths.hook) {
                "installed"
            } else if fs::symlink_metadata(&paths.hook).is_ok() {
                "unmanaged"
            } else {
                "not installed"
            };
            let backup_state = if fs::symlink_metadata(&paths.backup).is_ok() {
                "present"
            } else {
                "absent"
            };
            println!("Hooks directory: {}", paths.directory.display());
            println!("Hook: {hook_state}");
            println!("Backup hook: {backup_state}");
        }
        None => println!("Repository: not inside a Git repository"),
    }

    let api_key_state = if config.api_key_present() {
        "present"
    } else {
        "missing"
    };
    println!(
        "API key: {api_key_state} ({})",
        config.source_for("api_key")
    );
    println!(
        "Base URL: {} ({})",
        config.base_url,
        config.source_for("base_url")
    );
    println!("Model: {} ({})", config.model, config.source_for("model"));
    println!(
        "Language: {} ({})",
        config.language,
        config.source_for("language")
    );
    println!(
        "Context/output tokens: {}/{}",
        config.max_context_tokens, config.max_output_tokens
    );
    println!(
        "Debug prompt comments: {} ({})",
        if config.debug { "enabled" } else { "disabled" },
        config.source_for("debug")
    );
    Ok(0)
}
