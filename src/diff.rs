use anyhow::{Context, Result};
use git2::{Delta, DiffDelta, DiffFormat, DiffOptions, Repository};
use std::collections::BTreeMap;
use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChangeKind {
    Text,
    Binary,
    Lock,
}

impl ChangeKind {
    pub fn label(self) -> &'static str {
        match self {
            Self::Text => "text",
            Self::Binary => "binary",
            Self::Lock => "lock",
        }
    }
}

#[derive(Debug, Clone)]
pub struct ChangedFile {
    pub path: String,
    pub status: String,
    pub kind: ChangeKind,
    pub patch: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct StagedChanges {
    pub files: Vec<ChangedFile>,
}

impl StagedChanges {
    pub fn file_count(&self) -> usize {
        self.files.len()
    }

    pub fn manifest(&self) -> String {
        if self.files.is_empty() {
            return "- no staged files".to_string();
        }

        self.files
            .iter()
            .map(|file| format!("- [{}] {} ({})", file.status, file.path, file.kind.label()))
            .collect::<Vec<_>>()
            .join("\n")
    }
}

pub fn collect_staged_changes(repository: &Repository) -> Result<StagedChanges> {
    let index = repository.index().context("could not open the Git index")?;
    let head_tree = repository
        .head()
        .ok()
        .and_then(|head| head.peel_to_tree().ok());
    let mut options = DiffOptions::new();
    options.context_lines(3);
    options.max_size(-1);

    let diff = repository
        .diff_tree_to_index(head_tree.as_ref(), Some(&index), Some(&mut options))
        .context("could not collect the staged diff")?;

    let mut files = Vec::new();
    let mut indices = BTreeMap::new();
    for delta in diff.deltas() {
        let key = delta_key(&delta);
        let kind = if is_lock_delta(&delta) {
            ChangeKind::Lock
        } else if is_binary_delta(repository, &delta) {
            ChangeKind::Binary
        } else {
            ChangeKind::Text
        };
        let index = files.len();
        indices.insert(key, index);
        files.push(ChangedFile {
            path: display_path(&delta),
            status: status_label(delta.status()).to_string(),
            kind,
            patch: (kind == ChangeKind::Text).then(String::new),
        });
    }

    diff.print(DiffFormat::Patch, |delta, _hunk, line| {
        if let Some(index) = indices.get(&delta_key(&delta)).copied()
            && let Some(patch) = files[index].patch.as_mut()
        {
            append_patch_line(patch, line.origin(), line.content());
        }
        true
    })
    .context("could not render the staged diff")?;

    Ok(StagedChanges { files })
}

fn append_patch_line(target: &mut String, origin: char, content: &[u8]) {
    if matches!(origin, ' ' | '+' | '-') {
        target.push(origin);
    }
    target.push_str(&String::from_utf8_lossy(content));
}

fn delta_key(delta: &DiffDelta<'_>) -> String {
    format!(
        "{}\u{0}{}",
        delta
            .old_file()
            .path()
            .map(path_display)
            .unwrap_or_default(),
        delta
            .new_file()
            .path()
            .map(path_display)
            .unwrap_or_default()
    )
}

fn display_path(delta: &DiffDelta<'_>) -> String {
    let old = delta.old_file().path().map(path_display);
    let new = delta.new_file().path().map(path_display);
    match (old, new) {
        (Some(old), Some(new)) if old != new => format!("{old} -> {new}"),
        (_, Some(new)) => new,
        (Some(old), None) => old,
        (None, None) => "<unknown path>".to_string(),
    }
}

fn path_display(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

fn is_lock_delta(delta: &DiffDelta<'_>) -> bool {
    [delta.old_file().path(), delta.new_file().path()]
        .into_iter()
        .flatten()
        .any(is_lock_path)
}

fn is_binary_delta(repository: &Repository, delta: &DiffDelta<'_>) -> bool {
    delta.flags().is_binary()
        || delta.old_file().is_binary()
        || delta.new_file().is_binary()
        || [delta.old_file().id(), delta.new_file().id()]
            .into_iter()
            .filter(|id| !id.is_zero())
            .any(|id| repository.find_blob(id).is_ok_and(|blob| blob.is_binary()))
}

fn is_lock_path(path: &Path) -> bool {
    let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
        return false;
    };
    name.ends_with(".lock")
        || matches!(
            name,
            "package-lock.json"
                | "npm-shrinkwrap.json"
                | "pnpm-lock.yaml"
                | "yarn.lock"
                | "bun.lockb"
                | "go.sum"
                | "Pipfile.lock"
                | "poetry.lock"
                | "Gemfile.lock"
                | "composer.lock"
        )
}

fn status_label(status: Delta) -> &'static str {
    match status {
        Delta::Added => "added",
        Delta::Deleted => "deleted",
        Delta::Modified => "modified",
        Delta::Renamed => "renamed",
        Delta::Copied => "copied",
        Delta::Typechange => "typechange",
        Delta::Untracked => "untracked",
        Delta::Ignored => "ignored",
        Delta::Unreadable => "unreadable",
        Delta::Conflicted => "conflicted",
        Delta::Unmodified => "unmodified",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognises_common_lock_files() {
        assert!(is_lock_path(Path::new("Cargo.lock")));
        assert!(is_lock_path(Path::new("package-lock.json")));
        assert!(is_lock_path(Path::new("vendor/poetry.lock")));
        assert!(!is_lock_path(Path::new("src/clock.rs")));
    }
}
