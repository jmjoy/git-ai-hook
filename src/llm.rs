use crate::config::EffectiveConfig;
use crate::diff::StagedChanges;
use crate::prompt::{PromptBundle, build_prompts, clean_model_output};
use anyhow::{Context, Result, bail};
use indicatif::{ProgressBar, ProgressDrawTarget, ProgressStyle};
use rig_core::{
    client::CompletionClient,
    completion::{AssistantContent, CompletionError, CompletionModel},
    providers::openai,
};
use serde_json::json;
use std::time::Duration;

pub async fn generate_message(
    config: &EffectiveConfig,
    changes: &StagedChanges,
) -> Result<(String, PromptBundle)> {
    let (raw, prompts) = generate_raw_message(config, changes).await?;
    Ok((
        clean_model_output(config, &raw, changes.file_count()),
        prompts,
    ))
}

pub async fn generate_raw_message(
    config: &EffectiveConfig,
    changes: &StagedChanges,
) -> Result<(String, PromptBundle)> {
    let prompts = build_prompts(config, changes);
    let raw = request_with_spinner(config, &prompts, "Generating commit message...").await?;
    Ok((raw, prompts))
}

pub async fn validate_connection(config: &EffectiveConfig) -> Result<()> {
    let prompts = PromptBundle {
        system: format!(
            "Respond only with a Conventional Commit message in the language identified by the BCP 47 tag \"{}\".",
            config.language.trim()
        ),
        user: "Validate the configured model connection.".to_string(),
    };
    let mut validation_config = config.clone();
    validation_config.max_output_tokens = validation_config.max_output_tokens.min(8);
    let response =
        request_with_spinner(&validation_config, &prompts, "Validating configuration...").await?;
    if response.trim().is_empty() {
        bail!("configured model returned an empty completion")
    }
    Ok(())
}

async fn request_with_spinner(
    config: &EffectiveConfig,
    prompts: &PromptBundle,
    message: &str,
) -> Result<String> {
    let spinner = ProgressBar::with_draw_target(None, ProgressDrawTarget::stderr_with_hz(12));
    let style = ProgressStyle::with_template("{spinner} {msg}")
        .unwrap_or_else(|_| ProgressStyle::default_spinner());
    spinner.set_style(style);
    spinner.set_message(message.to_string());
    spinner.enable_steady_tick(Duration::from_millis(80));

    let result = request_completion(config, prompts).await;
    spinner.finish_and_clear();
    result
}

async fn request_completion(config: &EffectiveConfig, prompts: &PromptBundle) -> Result<String> {
    let api_key = config
        .api_key
        .as_deref()
        .filter(|value| !value.trim().is_empty())
        .context("API key is missing")?;

    let client = openai::Client::builder()
        .api_key(api_key)
        .base_url(config.base_url.trim())
        .build()
        .context("could not create the OpenAI-compatible client")?
        .completions_api();
    let model = client.completion_model(config.model.trim());
    let send_request = |use_developer_role| {
        let mut request = model
            .completion_request(prompts.user.clone())
            .preamble(prompts.system.clone())
            .max_tokens(config.max_output_tokens);

        if let Some(reasoning_effort) = config.reasoning_effort.as_deref() {
            request = request.additional_params(json!({
                "reasoning_effort": reasoning_effort.trim(),
            }));
        }

        if use_developer_role {
            // Rig's typed Chat Completions messages serialize instructions as `system`.
            // Override the wire messages to preserve their priority on developer-only APIs.
            request = request.additional_params(json!({
                "messages": [
                    {"role": "developer", "content": prompts.system},
                    {"role": "user", "content": prompts.user},
                ],
            }));
        }

        request.send()
    };
    let completion = async {
        match send_request(false).await {
            Err(error) if system_role_rejected(&error) => send_request(true).await,
            result => result,
        }
    };
    let response = tokio::time::timeout(Duration::from_secs(config.timeout_secs), completion)
        .await
        .map_err(|_| {
            anyhow::anyhow!(
                "model request timed out after {} seconds",
                config.timeout_secs
            )
        })?
        .context("OpenAI-compatible completion request failed")?;

    let text = response
        .choice
        .into_iter()
        .filter_map(|content| match content {
            AssistantContent::Text(text) => Some(text.text),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("");

    if text.trim().is_empty() {
        bail!("model returned no text completion")
    }
    Ok(text)
}

fn system_role_rejected(error: &CompletionError) -> bool {
    if error
        .provider_response_status()
        .map(|status| status.as_u16())
        != Some(400)
    {
        return false;
    }
    let Ok(Some(body)) = error.provider_response_json() else {
        return false;
    };
    let Some(message) = body
        .pointer("/error/message")
        .and_then(serde_json::Value::as_str)
    else {
        return false;
    };
    let message = message.to_ascii_lowercase();
    message.contains("system messages are not allowed")
        || (message.contains("messages[0].role")
            && message.contains("'system'")
            && message.contains("does not support"))
}
