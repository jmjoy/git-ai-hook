# git-ai-hook
[English](README.md) | [简体中文](README.zh-CN.md)

`git-ai-hook` generates Conventional Commit-style messages from the staged Git diff. It can be used directly from the command line or installed as a `prepare-commit-msg` hook.

> Platform support: Linux and macOS only. Windows is not supported.

## Install

Build and install the binary with Cargo:

```bash
cargo install --path .
```

From the Git repository where you want the hook:

```bash
git-ai-hook install
```

The command discovers the repository with `git2`, respects `core.hooksPath`, and creates a `prepare-commit-msg` symlink to the running binary. If a different hook already exists, it is moved to `prepare-commit-msg.local` and is run first on every commit.

To make `git commit --no-edit` discard the recorded `#` prompt lines reliably, installation sets the repository-local `commit.cleanup` to `strip`. The prior value is recorded under tool-specific Git config keys and restored during uninstall; a value you change after installation is left untouched.

## Configuration

Create the global configuration file:

```bash
git-ai-hook config init
git-ai-hook config path
```

The global file is located at `dirs::config_dir()/git-ai-hook/config.toml`. A repository can override individual settings with `.git-ai-hook.toml` at its worktree root.

```toml
[settings]
base_url = "https://api.openai.com/v1"
api_key = "sk-..."
model = "gpt-4o-mini"
# language = "en"
# reasoning_effort = "medium"
# max_context_tokens = 128000
# max_output_tokens = 16384
# timeout_secs = 60
# system_prompt = """
# <custom system prompt>
# """
```

`language` accepts BCP 47-style language tags such as `en`, `zh-CN`, and `pt-BR`. The configured language is appended to the system prompt as a required output-language instruction.

Each field is resolved independently in this order:

1. Dedicated environment variables
2. Project `.git-ai-hook.toml`
3. Global configuration
4. Built-in defaults

Supported environment variables are:

```text
GIT_AI_HOOK_BASE_URL
GIT_AI_HOOK_API_KEY
GIT_AI_HOOK_MODEL
GIT_AI_HOOK_LANGUAGE
GIT_AI_HOOK_REASONING_EFFORT
GIT_AI_HOOK_MAX_CONTEXT_TOKENS
GIT_AI_HOOK_MAX_OUTPUT_TOKENS
GIT_AI_HOOK_TIMEOUT_SECS
GIT_AI_HOOK_SYSTEM_PROMPT
```

Only the `[settings]` TOML table is supported. Legacy `[api]` configuration is rejected so a typo does not silently fall back to defaults.

### OpenAI-Compatible Endpoints

The client uses Rig's OpenAI Chat Completions compatibility path with the configured base URL. For example:

```toml
[settings]
base_url = "https://api.deepseek.com"
api_key = "..."
model = "deepseek-chat"
language = "en"
```

For Ollama's OpenAI-compatible endpoint:

```toml
[settings]
base_url = "http://localhost:11434/v1"
api_key = "ollama"
model = "llama3.2"
language = "en"
```

## Commands

```text
git-ai-hook install
git-ai-hook uninstall
git-ai-hook status
git-ai-hook generate
git-ai-hook config path
git-ai-hook config init [--force]
git-ai-hook config validate
```

- `generate` collects the staged diff and prints a clean commit message to stdout without changing Git files.
- `status` reports hook state, configuration sources, and whether an API key is present. It never prints key material.
- `config validate` validates the effective local configuration and sends a minimal real completion request to the configured model. It can incur a small provider charge.
- `config init` refuses to overwrite an existing global file unless `--force` is supplied.

## Hook Behavior

When Git invokes the hook, `git-ai-hook`:

1. Runs `prepare-commit-msg.local` first when present. A nonzero exit from that original hook is preserved.
2. Exits without generation for `git commit -m` / `-F` (`source == "message"`), `GIT_AI_HOOK_SKIP=1`, a missing API key, or a non-comment message already written by the user.
3. Collects the complete staged index diff. Binary and lock files contribute only file metadata, not patch content.
4. Builds the model prompt from the changed-file manifest and text patches. The full diff is collected first; `max_context_tokens` then conservatively limits the patch content sent to the model while retaining file metadata.
5. Shows an stderr spinner while the request is in flight and clears it on success, timeout, or failure.
6. Inserts the generated message first, then a comment block containing the complete system and user prompts, and finally the original Git comment content unchanged.

The resulting message file has this shape:

```text
feat: add generated commit messages

# --- git-ai-hook prompt begin ---
# <complete system prompt>
# ---
# <complete user prompt>
# --- git-ai-hook prompt end ---
# <original Git comments>
```

Git ignores the prompt block because every line is a comment. This makes `git commit --no-edit` use the generated message while retaining the editor context. The prompt block can contain staged-diff content, so avoid sharing an editor backup or temporary commit message file if that content is sensitive.

Model errors, timeouts, configuration errors, and message-file errors are logged under the user cache/config area and return success from this tool's hook path, so they do not block a commit. Invalid model output falls back to `chore: update N files`.

For backward-incompatible changes, generated Conventional Commit subjects use `!` immediately after the type, for example `feat!: remove the legacy API`.

## Uninstall

Run this inside the repository:

```bash
git-ai-hook uninstall
```

The command removes only a symlink recognized as managed by the current binary. If `prepare-commit-msg.local` exists, it is atomically restored to `prepare-commit-msg`; otherwise the managed link is removed.
