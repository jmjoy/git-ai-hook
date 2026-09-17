use std::fs::{self, OpenOptions};
use std::io::Write;
use std::time::{SystemTime, UNIX_EPOCH};

pub fn log_hook_error(context: &str, error: &dyn std::fmt::Display) {
    let Some(root) = dirs::cache_dir().or_else(dirs::config_dir) else {
        return;
    };

    let directory = root.join("git-ai-hook");
    if fs::create_dir_all(&directory).is_err() {
        return;
    }

    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0);
    let path = directory.join("git-ai-hook.log");
    let Ok(mut file) = OpenOptions::new().create(true).append(true).open(path) else {
        return;
    };

    let _ = writeln!(file, "[{timestamp}] {context}: {error}");
}
