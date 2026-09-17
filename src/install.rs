use anyhow::{Context, Result, bail};
use git2::{ConfigLevel, Repository};
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

pub const HOOK_NAME: &str = "prepare-commit-msg";
pub const BACKUP_HOOK_NAME: &str = "prepare-commit-msg.local";
const COMMIT_CLEANUP_KEY: &str = "commit.cleanup";
const CLEANUP_WAS_SET_KEY: &str = "git-ai-hook.commit-cleanup-was-set";
const CLEANUP_WAS_LOCAL_KEY: &str = "git-ai-hook.commit-cleanup-was-local";
const CLEANUP_PREVIOUS_VALUE_KEY: &str = "git-ai-hook.previous-commit-cleanup";

#[derive(Debug, Clone)]
pub struct HookPaths {
    pub directory: PathBuf,
    pub hook: PathBuf,
    pub backup: PathBuf,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InstallResult {
    Installed,
    Refreshed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UninstallResult {
    RestoredBackup,
    RemovedManagedHook,
    NotInstalled,
}

pub fn hook_paths(repository: &Repository) -> Result<HookPaths> {
    let configured = repository
        .config()
        .ok()
        .and_then(|config| config.get_path("core.hooksPath").ok());
    let directory = match configured {
        Some(path) if path.is_absolute() => path,
        Some(path) => hook_base_directory(repository).join(path),
        None => repository.path().join("hooks"),
    };

    Ok(HookPaths {
        hook: directory.join(HOOK_NAME),
        backup: directory.join(BACKUP_HOOK_NAME),
        directory,
    })
}

pub fn install(repository: &Repository) -> Result<(HookPaths, InstallResult)> {
    let paths = hook_paths(repository)?;
    fs::create_dir_all(&paths.directory).with_context(|| {
        format!(
            "could not create hooks directory {}",
            paths.directory.display()
        )
    })?;

    let hook_exists = path_exists_or_symlink(&paths.hook);
    let managed = hook_exists && is_managed_hook(&paths.hook);
    if hook_exists && !managed {
        if fs::symlink_metadata(&paths.hook)
            .with_context(|| format!("could not inspect hook {}", paths.hook.display()))?
            .file_type()
            .is_dir()
        {
            bail!("hook path {} is a directory", paths.hook.display())
        }
        if path_exists_or_symlink(&paths.backup) {
            bail!(
                "refusing to overwrite existing hook backup {}",
                paths.backup.display()
            )
        }
    }
    configure_commit_cleanup(repository)?;

    let result = if managed {
        fs::remove_file(&paths.hook)
            .with_context(|| format!("could not replace managed hook {}", paths.hook.display()))?;
        InstallResult::Refreshed
    } else if hook_exists {
        fs::rename(&paths.hook, &paths.backup).with_context(|| {
            format!(
                "could not back up existing hook {} to {}",
                paths.hook.display(),
                paths.backup.display()
            )
        })?;
        InstallResult::Installed
    } else {
        InstallResult::Installed
    };

    let executable = env::current_exe().context("could not locate the running executable")?;
    create_symlink(&executable, &paths.hook).with_context(|| {
        format!(
            "could not create hook symlink {} -> {}",
            paths.hook.display(),
            executable.display()
        )
    })?;

    Ok((paths, result))
}

pub fn uninstall(repository: &Repository) -> Result<(HookPaths, UninstallResult)> {
    let paths = hook_paths(repository)?;
    if !path_exists_or_symlink(&paths.hook) {
        restore_commit_cleanup(repository)?;
        return Ok((paths, UninstallResult::NotInstalled));
    }
    if !is_managed_hook(&paths.hook) {
        bail!(
            "refusing to remove unmanaged hook {}; install it with git-ai-hook first",
            paths.hook.display()
        )
    }
    if path_exists_or_symlink(&paths.backup)
        && fs::symlink_metadata(&paths.backup)
            .with_context(|| format!("could not inspect hook backup {}", paths.backup.display()))?
            .file_type()
            .is_dir()
    {
        bail!("hook backup {} is a directory", paths.backup.display())
    }

    fs::remove_file(&paths.hook)
        .with_context(|| format!("could not remove hook {}", paths.hook.display()))?;
    let result = if path_exists_or_symlink(&paths.backup) {
        fs::rename(&paths.backup, &paths.hook).with_context(|| {
            format!(
                "could not restore hook backup {} to {}",
                paths.backup.display(),
                paths.hook.display()
            )
        })?;
        UninstallResult::RestoredBackup
    } else {
        UninstallResult::RemovedManagedHook
    };
    restore_commit_cleanup(repository)?;

    Ok((paths, result))
}

pub fn is_managed_hook(path: &Path) -> bool {
    let Ok(metadata) = fs::symlink_metadata(path) else {
        return false;
    };
    if !metadata.file_type().is_symlink() {
        return false;
    }
    let Ok(target) = fs::read_link(path) else {
        return false;
    };
    let target = if target.is_absolute() {
        target
    } else {
        path.parent().unwrap_or_else(|| Path::new(".")).join(target)
    };
    let Ok(target) = fs::canonicalize(target) else {
        return false;
    };
    let Ok(executable) = env::current_exe().and_then(fs::canonicalize) else {
        return false;
    };
    target == executable
}

pub fn run_backup_hook(paths: &HookPaths, arguments: &[std::ffi::OsString]) -> Result<Option<i32>> {
    if !path_exists_or_symlink(&paths.backup) {
        return Ok(None);
    }

    let status = Command::new(&paths.backup)
        .args(arguments)
        .status()
        .with_context(|| format!("could not execute chained hook {}", paths.backup.display()))?;
    if status.success() {
        Ok(None)
    } else {
        Ok(Some(status.code().unwrap_or(1)))
    }
}

fn hook_base_directory(repository: &Repository) -> PathBuf {
    repository
        .workdir()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| repository.path().to_path_buf())
}

fn path_exists_or_symlink(path: &Path) -> bool {
    fs::symlink_metadata(path).is_ok()
}

fn configure_commit_cleanup(repository: &Repository) -> Result<()> {
    let mut config = repository
        .config()
        .context("could not open the repository configuration")?;
    let current = config.get_string(COMMIT_CLEANUP_KEY).ok();
    let was_local = config
        .get_entry(COMMIT_CLEANUP_KEY)
        .is_ok_and(|entry| entry.level() == ConfigLevel::Local);
    if current.as_deref() == Some("strip") {
        return Ok(());
    }

    if config.get_string(CLEANUP_WAS_SET_KEY).is_err() {
        match current.as_deref() {
            Some(value) => {
                config
                    .set_str(CLEANUP_WAS_SET_KEY, "true")
                    .context("could not record the existing commit.cleanup value")?;
                config
                    .set_str(CLEANUP_PREVIOUS_VALUE_KEY, value)
                    .context("could not back up the existing commit.cleanup value")?;
                config
                    .set_str(
                        CLEANUP_WAS_LOCAL_KEY,
                        if was_local { "true" } else { "false" },
                    )
                    .context("could not record the commit.cleanup source level")?;
            }
            None => {
                config
                    .set_str(CLEANUP_WAS_SET_KEY, "false")
                    .context("could not record the missing commit.cleanup value")?;
                config
                    .set_str(CLEANUP_WAS_LOCAL_KEY, "false")
                    .context("could not record the missing commit.cleanup source level")?;
            }
        }
    }
    config
        .set_str(COMMIT_CLEANUP_KEY, "strip")
        .context("could not set commit.cleanup=strip for git-ai-hook")?;
    Ok(())
}

fn restore_commit_cleanup(repository: &Repository) -> Result<()> {
    let mut config = repository
        .config()
        .context("could not open the repository configuration")?;
    let Ok(was_set) = config.get_string(CLEANUP_WAS_SET_KEY) else {
        return Ok(());
    };
    let was_local = config.get_string(CLEANUP_WAS_LOCAL_KEY).ok().as_deref() == Some("true");
    let current = config.get_string(COMMIT_CLEANUP_KEY).ok();
    if current.as_deref() == Some("strip") {
        if was_set == "true" {
            let previous = config
                .get_string(CLEANUP_PREVIOUS_VALUE_KEY)
                .context("missing saved commit.cleanup value")?;
            if was_local {
                config
                    .set_str(COMMIT_CLEANUP_KEY, &previous)
                    .context("could not restore the previous commit.cleanup value")?;
            } else {
                config
                    .remove(COMMIT_CLEANUP_KEY)
                    .context("could not remove git-ai-hook's local commit.cleanup override")?;
            }
        } else {
            config
                .remove(COMMIT_CLEANUP_KEY)
                .context("could not remove git-ai-hook's commit.cleanup setting")?;
        }
    }
    let _ = config.remove(CLEANUP_WAS_SET_KEY);
    let _ = config.remove(CLEANUP_WAS_LOCAL_KEY);
    let _ = config.remove(CLEANUP_PREVIOUS_VALUE_KEY);
    Ok(())
}

#[cfg(unix)]
fn create_symlink(target: &Path, link: &Path) -> std::io::Result<()> {
    std::os::unix::fs::symlink(target, link)
}
