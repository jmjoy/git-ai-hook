use anyhow::{Context, Result, bail};
use git2::Repository;
use serde::Deserialize;
use std::collections::BTreeMap;
use std::env;
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};

pub const DEFAULT_BASE_URL: &str = "https://api.openai.com/v1";
pub const DEFAULT_MODEL: &str = "gpt-4o-mini";
pub const DEFAULT_LANGUAGE: &str = "en";
pub const DEFAULT_MAX_CONTEXT_TOKENS: u64 = 128_000;
pub const DEFAULT_MAX_OUTPUT_TOKENS: u64 = 16_384;
pub const DEFAULT_TIMEOUT_SECS: u64 = 60;

pub const ENV_BASE_URL: &str = "GIT_AI_HOOK_BASE_URL";
pub const ENV_API_KEY: &str = "GIT_AI_HOOK_API_KEY";
pub const ENV_MODEL: &str = "GIT_AI_HOOK_MODEL";
pub const ENV_LANGUAGE: &str = "GIT_AI_HOOK_LANGUAGE";
pub const ENV_REASONING_EFFORT: &str = "GIT_AI_HOOK_REASONING_EFFORT";
pub const ENV_MAX_CONTEXT_TOKENS: &str = "GIT_AI_HOOK_MAX_CONTEXT_TOKENS";
pub const ENV_MAX_OUTPUT_TOKENS: &str = "GIT_AI_HOOK_MAX_OUTPUT_TOKENS";
pub const ENV_TIMEOUT_SECS: &str = "GIT_AI_HOOK_TIMEOUT_SECS";
pub const ENV_SYSTEM_PROMPT: &str = "GIT_AI_HOOK_SYSTEM_PROMPT";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfigSource {
    Default,
    Global,
    Project,
    Environment,
}

impl fmt::Display for ConfigSource {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let value = match self {
            Self::Default => "default",
            Self::Global => "global config",
            Self::Project => "project config",
            Self::Environment => "environment",
        };
        formatter.write_str(value)
    }
}

#[derive(Debug, Clone)]
pub struct EffectiveConfig {
    pub base_url: String,
    pub api_key: Option<String>,
    pub model: String,
    pub language: String,
    pub reasoning_effort: Option<String>,
    pub max_context_tokens: u64,
    pub max_output_tokens: u64,
    pub timeout_secs: u64,
    pub system_prompt: String,
    pub(crate) origins: BTreeMap<&'static str, ConfigSource>,
}

impl EffectiveConfig {
    pub fn api_key_present(&self) -> bool {
        self.api_key
            .as_deref()
            .is_some_and(|value| !value.trim().is_empty())
    }

    pub fn source_for(&self, field: &'static str) -> ConfigSource {
        self.origins
            .get(field)
            .copied()
            .unwrap_or(ConfigSource::Default)
    }
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct FileConfig {
    settings: Option<PartialSettings>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct PartialSettings {
    base_url: Option<String>,
    api_key: Option<String>,
    model: Option<String>,
    language: Option<String>,
    reasoning_effort: Option<String>,
    max_context_tokens: Option<u64>,
    max_output_tokens: Option<u64>,
    timeout_secs: Option<u64>,
    system_prompt: Option<String>,
}

pub fn global_config_path() -> Result<PathBuf> {
    let root =
        dirs::config_dir().context("could not determine the user configuration directory")?;
    Ok(root.join("git-ai-hook").join("config.toml"))
}

pub fn project_config_path(repository: &Repository) -> Option<PathBuf> {
    repository
        .workdir()
        .map(|path| path.join(".git-ai-hook.toml"))
}

pub fn discover_repository() -> Result<Repository> {
    Repository::open_from_env().context("not inside a Git repository")
}

pub fn discover_repository_optional() -> Option<Repository> {
    Repository::open_from_env().ok()
}

pub fn load_effective(repository: Option<&Repository>) -> Result<EffectiveConfig> {
    let mut config = default_config();

    let global_path = global_config_path()?;
    if let Some(settings) = load_file(&global_path)? {
        apply_settings(&mut config, settings, ConfigSource::Global);
    }

    if let Some(path) = repository.and_then(project_config_path)
        && let Some(settings) = load_file(&path)?
    {
        apply_settings(&mut config, settings, ConfigSource::Project);
    }

    apply_settings(
        &mut config,
        environment_settings()?,
        ConfigSource::Environment,
    );
    validate_static(&config)?;
    Ok(config)
}

pub fn validate_for_request(config: &EffectiveConfig) -> Result<()> {
    validate_static(config)?;
    if !config.api_key_present() {
        bail!("API key is missing; set {ENV_API_KEY} or settings.api_key")
    }
    Ok(())
}

pub fn write_global_template(force: bool) -> Result<PathBuf> {
    let path = global_config_path()?;
    let parent = path
        .parent()
        .context("global config path has no parent directory")?;
    fs::create_dir_all(parent).with_context(|| {
        format!(
            "could not create configuration directory {}",
            parent.display()
        )
    })?;

    if path.exists() && !force {
        bail!(
            "configuration already exists at {}; use config init --force to replace it",
            path.display()
        );
    }

    fs::write(&path, config_template())
        .with_context(|| format!("could not write configuration file {}", path.display()))?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600))
            .with_context(|| format!("could not secure configuration file {}", path.display()))?;
    }

    Ok(path)
}

pub fn config_template() -> String {
    let commented_prompt = default_system_prompt()
        .lines()
        .map(|line| format!("# {line}"))
        .collect::<Vec<_>>()
        .join("\n");

    format!(
        r#"[settings]
base_url = "{DEFAULT_BASE_URL}"
api_key = ""
model = "{DEFAULT_MODEL}"
# language = "{DEFAULT_LANGUAGE}"
# reasoning_effort = "medium"
# max_context_tokens = {DEFAULT_MAX_CONTEXT_TOKENS}
# max_output_tokens = {DEFAULT_MAX_OUTPUT_TOKENS}
# timeout_secs = {DEFAULT_TIMEOUT_SECS}
# system_prompt = """
{commented_prompt}
# """
"#
    )
}

fn default_config() -> EffectiveConfig {
    let mut origins = BTreeMap::new();
    for field in field_names() {
        origins.insert(*field, ConfigSource::Default);
    }

    EffectiveConfig {
        base_url: DEFAULT_BASE_URL.to_string(),
        api_key: None,
        model: DEFAULT_MODEL.to_string(),
        language: DEFAULT_LANGUAGE.to_string(),
        reasoning_effort: None,
        max_context_tokens: DEFAULT_MAX_CONTEXT_TOKENS,
        max_output_tokens: DEFAULT_MAX_OUTPUT_TOKENS,
        timeout_secs: DEFAULT_TIMEOUT_SECS,
        system_prompt: default_system_prompt().to_string(),
        origins,
    }
}

pub fn default_system_prompt() -> &'static str {
    "You are a Git commit message generation assistant. Only output the commit message; do not include explanations, labels, or Markdown code fences.\n\nUse this format:\n\n<type>[!]: <one-line summary>\n\n- <detail>\n\nAllowed types: feat, fix, refactor, docs, style, test, chore, perf, build, ci. Add `!` immediately after the type only when the change breaks backward compatibility, for example `feat!: remove the legacy API`. The first line must be 72 characters or fewer. Keep the message concise. Include detail bullets only when they add meaningful context; for small or limited changes, output only the summary line. When details are needed, group changes of the same kind into one bullet rather than splitting them across multiple bullets. Every detail bullet must start with `- `. When there are no details, omit both the blank separator and bullets."
}

fn load_file(path: &Path) -> Result<Option<PartialSettings>> {
    if !path.exists() {
        return Ok(None);
    }

    let content = fs::read_to_string(path)
        .with_context(|| format!("could not read configuration file {}", path.display()))?;
    let parsed: FileConfig = toml::from_str(&content)
        .with_context(|| format!("invalid configuration file {}", path.display()))?;
    Ok(parsed.settings)
}

fn environment_settings() -> Result<PartialSettings> {
    Ok(PartialSettings {
        base_url: env_value(ENV_BASE_URL)?,
        api_key: env_value(ENV_API_KEY)?,
        model: env_value(ENV_MODEL)?,
        language: env_value(ENV_LANGUAGE)?,
        reasoning_effort: env_value(ENV_REASONING_EFFORT)?,
        max_context_tokens: parse_env_u64(ENV_MAX_CONTEXT_TOKENS)?,
        max_output_tokens: parse_env_u64(ENV_MAX_OUTPUT_TOKENS)?,
        timeout_secs: parse_env_u64(ENV_TIMEOUT_SECS)?,
        system_prompt: env_value(ENV_SYSTEM_PROMPT)?,
    })
}

fn env_value(name: &str) -> Result<Option<String>> {
    match env::var(name) {
        Ok(value) => Ok(Some(value)),
        Err(env::VarError::NotPresent) => Ok(None),
        Err(env::VarError::NotUnicode(_)) => bail!("{name} is not valid Unicode"),
    }
}

fn parse_env_u64(name: &str) -> Result<Option<u64>> {
    let Some(value) = env_value(name)? else {
        return Ok(None);
    };
    let parsed = value
        .parse::<u64>()
        .with_context(|| format!("{name} must be an unsigned integer"))?;
    Ok(Some(parsed))
}

fn apply_settings(config: &mut EffectiveConfig, settings: PartialSettings, source: ConfigSource) {
    apply_string(
        &mut config.base_url,
        settings.base_url,
        "base_url",
        source,
        &mut config.origins,
    );
    apply_option_string(
        &mut config.api_key,
        settings.api_key,
        "api_key",
        source,
        &mut config.origins,
    );
    apply_string(
        &mut config.model,
        settings.model,
        "model",
        source,
        &mut config.origins,
    );
    apply_string(
        &mut config.language,
        settings.language,
        "language",
        source,
        &mut config.origins,
    );
    apply_option_string(
        &mut config.reasoning_effort,
        settings.reasoning_effort,
        "reasoning_effort",
        source,
        &mut config.origins,
    );
    apply_u64(
        &mut config.max_context_tokens,
        settings.max_context_tokens,
        "max_context_tokens",
        source,
        &mut config.origins,
    );
    apply_u64(
        &mut config.max_output_tokens,
        settings.max_output_tokens,
        "max_output_tokens",
        source,
        &mut config.origins,
    );
    apply_u64(
        &mut config.timeout_secs,
        settings.timeout_secs,
        "timeout_secs",
        source,
        &mut config.origins,
    );
    apply_string(
        &mut config.system_prompt,
        settings.system_prompt,
        "system_prompt",
        source,
        &mut config.origins,
    );
}

fn apply_string(
    destination: &mut String,
    value: Option<String>,
    field: &'static str,
    source: ConfigSource,
    origins: &mut BTreeMap<&'static str, ConfigSource>,
) {
    if let Some(value) = value {
        *destination = value;
        origins.insert(field, source);
    }
}

fn apply_option_string(
    destination: &mut Option<String>,
    value: Option<String>,
    field: &'static str,
    source: ConfigSource,
    origins: &mut BTreeMap<&'static str, ConfigSource>,
) {
    if let Some(value) = value {
        *destination = Some(value);
        origins.insert(field, source);
    }
}

fn apply_u64(
    destination: &mut u64,
    value: Option<u64>,
    field: &'static str,
    source: ConfigSource,
    origins: &mut BTreeMap<&'static str, ConfigSource>,
) {
    if let Some(value) = value {
        *destination = value;
        origins.insert(field, source);
    }
}

fn validate_static(config: &EffectiveConfig) -> Result<()> {
    let base_url = config.base_url.trim();
    if !(base_url.starts_with("https://") || base_url.starts_with("http://"))
        || base_url.split("://").nth(1).is_none_or(str::is_empty)
        || base_url.contains(char::is_whitespace)
    {
        bail!("settings.base_url must be an http:// or https:// URL")
    }
    if config.model.trim().is_empty() {
        bail!("settings.model must not be empty")
    }
    if !is_bcp47_like(&config.language) {
        bail!("settings.language must be a BCP 47 language tag such as en or zh-CN")
    }
    if config.max_context_tokens == 0 {
        bail!("settings.max_context_tokens must be greater than zero")
    }
    if config.max_output_tokens == 0 {
        bail!("settings.max_output_tokens must be greater than zero")
    }
    if config.timeout_secs == 0 {
        bail!("settings.timeout_secs must be greater than zero")
    }
    if config.system_prompt.trim().is_empty() {
        bail!("settings.system_prompt must not be empty")
    }
    if config
        .reasoning_effort
        .as_deref()
        .is_some_and(|value| value.trim().is_empty())
    {
        bail!("settings.reasoning_effort must not be empty when set")
    }
    Ok(())
}

fn is_bcp47_like(value: &str) -> bool {
    let mut subtags = value.split('-');
    let Some(primary) = subtags.next() else {
        return false;
    };
    if !(2..=3).contains(&primary.len())
        || !primary
            .chars()
            .all(|character| character.is_ascii_alphabetic())
    {
        return false;
    }
    subtags.all(|subtag| {
        (1..=8).contains(&subtag.len())
            && subtag
                .chars()
                .all(|character| character.is_ascii_alphanumeric())
    })
}

fn field_names() -> &'static [&'static str] {
    &[
        "base_url",
        "api_key",
        "model",
        "language",
        "reasoning_effort",
        "max_context_tokens",
        "max_output_tokens",
        "timeout_secs",
        "system_prompt",
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_common_language_tags() {
        assert!(is_bcp47_like("en"));
        assert!(is_bcp47_like("zh-CN"));
        assert!(is_bcp47_like("pt-BR"));
        assert!(!is_bcp47_like("english"));
        assert!(!is_bcp47_like("en_Us"));
    }

    #[test]
    fn template_uses_settings_table() {
        assert!(config_template().starts_with("[settings]"));
        assert!(!config_template().contains("[api]"));
    }

    #[test]
    fn template_comments_out_optional_settings_and_default_system_prompt() {
        let template = config_template();
        let parsed: FileConfig = toml::from_str(&template).expect("parse config template");
        let settings = parsed.settings.expect("settings table");
        assert_eq!(settings.base_url.as_deref(), Some(DEFAULT_BASE_URL));
        assert_eq!(settings.api_key.as_deref(), Some(""));
        assert_eq!(settings.model.as_deref(), Some(DEFAULT_MODEL));
        assert_eq!(settings.language, None);
        assert_eq!(settings.max_context_tokens, None);
        assert_eq!(settings.max_output_tokens, None);
        assert_eq!(settings.timeout_secs, None);
        assert_eq!(settings.system_prompt, None);
        assert!(template.contains("# system_prompt = \"\"\""));
        for line in default_system_prompt().lines() {
            assert!(template.contains(&format!("# {line}")));
        }
    }

    #[test]
    fn applies_layers_per_field() {
        let mut config = default_config();
        apply_settings(
            &mut config,
            PartialSettings {
                model: Some("global-model".to_string()),
                language: Some("pt-BR".to_string()),
                ..Default::default()
            },
            ConfigSource::Global,
        );
        apply_settings(
            &mut config,
            PartialSettings {
                model: Some("project-model".to_string()),
                ..Default::default()
            },
            ConfigSource::Project,
        );
        apply_settings(
            &mut config,
            PartialSettings {
                api_key: Some("environment-key".to_string()),
                ..Default::default()
            },
            ConfigSource::Environment,
        );

        assert_eq!(config.model, "project-model");
        assert_eq!(config.language, "pt-BR");
        assert_eq!(config.api_key.as_deref(), Some("environment-key"));
        assert_eq!(config.source_for("model"), ConfigSource::Project);
        assert_eq!(config.source_for("language"), ConfigSource::Global);
        assert_eq!(config.source_for("api_key"), ConfigSource::Environment);
    }

    #[test]
    fn rejects_legacy_api_table() {
        assert!(toml::from_str::<FileConfig>("[api]\nmodel = \"old\"\n").is_err());
    }

    #[test]
    fn default_prompt_requires_concise_grouped_details() {
        let prompt = default_system_prompt();
        assert!(prompt.starts_with("You are a Git commit message generation assistant."));
        assert!(prompt.contains("Keep the message concise"));
        assert!(prompt.contains("for small or limited changes, output only the summary line"));
        assert!(prompt.contains("group changes of the same kind into one bullet"));
        assert!(prompt.contains("Every detail bullet must start with `- `"));
        assert!(prompt.contains("breaks backward compatibility"));
    }
}
