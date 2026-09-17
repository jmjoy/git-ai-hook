use crate::config::EffectiveConfig;
use crate::diff::StagedChanges;

const ALLOWED_TYPES: [&str; 10] = [
    "feat", "fix", "refactor", "docs", "style", "test", "chore", "perf", "build", "ci",
];

#[derive(Debug, Clone)]
pub struct PromptBundle {
    pub system: String,
    pub user: String,
}

pub fn build_prompts(config: &EffectiveConfig, changes: &StagedChanges) -> PromptBundle {
    let system = format!(
        "{}\n\nWrite the commit message in the language identified by the BCP 47 tag \"{}\".",
        config.system_prompt.trim_end(),
        config.language.trim()
    );
    let manifest = changes.manifest();
    let mut user = format!(
        "Generate a commit message for the staged Git changes below.\n\nChanged files:\n{manifest}\n\nStaged diff:\n"
    );

    // A fixed three-byte estimate is intentionally conservative relative to ordinary prose
    // tokenization while remaining provider-agnostic for OpenAI-compatible backends.
    let reserved = estimate_tokens(&system)
        .saturating_add(config.max_output_tokens)
        .saturating_add(estimate_tokens(&user));
    let mut remaining = config.max_context_tokens.saturating_sub(reserved);
    let mut truncated = false;

    for file in &changes.files {
        let Some(patch) = &file.patch else {
            continue;
        };
        let cost = estimate_tokens(patch);
        if cost <= remaining {
            user.push_str(patch);
            if !patch.ends_with('\n') {
                user.push('\n');
            }
            remaining = remaining.saturating_sub(cost);
            continue;
        }

        let marker = "\n[staged diff truncated by max_context_tokens]\n";
        let marker_cost = estimate_tokens(marker);
        let prefix = prefix_for_budget(patch, remaining.saturating_sub(marker_cost));
        if !prefix.is_empty() {
            user.push_str(&prefix);
            if !prefix.ends_with('\n') {
                user.push('\n');
            }
        }
        user.push_str(marker);
        truncated = true;
        break;
    }

    if !truncated && changes.files.is_empty() {
        user.push_str("(There are no staged changes.)\n");
    }

    PromptBundle { system, user }
}

pub fn clean_model_output(output: &str, file_count: usize) -> String {
    let cleaned = output
        .lines()
        .filter(|line| !line.trim().starts_with("```"))
        .collect::<Vec<_>>()
        .join("\n");
    let cleaned = cleaned.trim();

    if is_valid_message(cleaned) {
        cleaned.to_string()
    } else {
        fallback_message(file_count)
    }
}

pub fn fallback_message(file_count: usize) -> String {
    format!("chore: update {file_count} files")
}

pub fn is_non_comment_content(content: &str) -> bool {
    content.lines().any(|line| {
        let trimmed = line.trim();
        !trimmed.is_empty() && !trimmed.starts_with('#')
    })
}

pub fn assemble_message_file(
    message: &str,
    prompts: &PromptBundle,
    original_comments: &str,
) -> String {
    let mut assembled = String::new();
    assembled.push_str(message.trim());
    assembled.push_str("\n\n# --- git-ai-hook prompt begin ---\n");
    append_commented_lines(&mut assembled, &prompts.system);
    assembled.push_str("# ---\n");
    append_commented_lines(&mut assembled, &prompts.user);
    assembled.push_str("# --- git-ai-hook prompt end ---\n");
    assembled.push_str(original_comments);
    assembled
}

fn append_commented_lines(target: &mut String, value: &str) {
    for line in value.split('\n') {
        target.push_str("# ");
        target.push_str(line.strip_suffix('\r').unwrap_or(line));
        target.push('\n');
    }
}

fn is_valid_message(message: &str) -> bool {
    let Some(first_line) = message.lines().next() else {
        return false;
    };
    if first_line.chars().count() > 72 {
        return false;
    }
    let Some((kind, summary)) = first_line.split_once(": ") else {
        return false;
    };
    ALLOWED_TYPES.contains(&kind) && !summary.trim().is_empty()
}

fn estimate_tokens(value: &str) -> u64 {
    (value.len() as u64).div_ceil(3)
}

fn prefix_for_budget(value: &str, token_budget: u64) -> String {
    let byte_budget = token_budget.saturating_mul(3) as usize;
    if byte_budget >= value.len() {
        return value.to_string();
    }

    let mut end = byte_budget;
    while end > 0 && !value.is_char_boundary(end) {
        end -= 1;
    }
    value[..end].to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{
        ConfigSource, DEFAULT_BASE_URL, DEFAULT_LANGUAGE, DEFAULT_MODEL, DEFAULT_TIMEOUT_SECS,
        EffectiveConfig,
    };
    use crate::diff::{ChangeKind, ChangedFile};
    use std::collections::BTreeMap;

    fn config(max_context_tokens: u64) -> EffectiveConfig {
        EffectiveConfig {
            base_url: DEFAULT_BASE_URL.to_string(),
            api_key: Some("test".to_string()),
            model: DEFAULT_MODEL.to_string(),
            language: DEFAULT_LANGUAGE.to_string(),
            reasoning_effort: None,
            max_context_tokens,
            max_output_tokens: 10,
            timeout_secs: DEFAULT_TIMEOUT_SECS,
            system_prompt: "Only output a commit message.".to_string(),
            origins: BTreeMap::from([("api_key", ConfigSource::Environment)]),
        }
    }

    #[test]
    fn strips_fences_and_validates_first_line() {
        assert_eq!(
            clean_model_output("```text\nfeat: add hook\n\n- wire it up\n```", 2),
            "feat: add hook\n\n- wire it up"
        );
        assert_eq!(clean_model_output("A title", 2), "chore: update 2 files");
    }

    #[test]
    fn preserves_comment_suffix_after_prompt_block() {
        let bundle = PromptBundle {
            system: "system\nsecond".to_string(),
            user: "user".to_string(),
        };
        let result = assemble_message_file("fix: repair", &bundle, "# existing\n");
        assert!(result.starts_with("fix: repair\n\n# --- git-ai-hook prompt begin ---"));
        assert!(result.contains("# system\n# second\n# ---\n# user\n"));
        assert!(result.ends_with("# existing\n"));
    }

    #[test]
    fn skips_non_comment_messages() {
        assert!(is_non_comment_content("# comment\nuser message\n"));
        assert!(!is_non_comment_content("# comment\n\n"));
    }

    #[test]
    fn truncates_patch_but_keeps_file_manifest() {
        let changes = StagedChanges {
            files: vec![ChangedFile {
                path: "src/main.rs".to_string(),
                status: "modified".to_string(),
                kind: ChangeKind::Text,
                patch: Some(
                    "diff --git a/src/main.rs b/src/main.rs\n+very long change\n".repeat(20),
                ),
            }],
        };
        let prompts = build_prompts(&config(40), &changes);
        assert!(prompts.user.contains("src/main.rs"));
        assert!(prompts.user.contains("truncated by max_context_tokens"));
    }
}
