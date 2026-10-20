# git-ai-hook

[English](README.md) | [简体中文](README.zh-CN.md)

`git-ai-hook` generates Conventional Commit-style messages from the staged Git diff. It can be used directly from the command line or installed as a `prepare-commit-msg` hook.

> Platform support: Linux and macOS only. Windows is not supported.

## Quick Start

1. Install the binary:

   ```bash
   cargo install git-ai-hook
   ```

2. Create the global configuration file and print its path. Set at least `api_key` in that file; the [Configuration](#configuration) section covers provider settings and other options.

   ```bash
   git ai-hook config init
   git ai-hook config path
   ```

3. In the Git repository where you want generated commit messages, install the hook:

   ```bash
   cd /path/to/repository
   git ai-hook install
   ```

4. Stage the changes you want to commit, then let the hook generate a message:

   ```bash
   git add <files>
   git commit --no-edit
   ```

   `git commit --no-edit` accepts the generated message as is. To review or edit the AI-generated message, run `git commit` without `--no-edit`; Git opens the configured editor after the hook generates it. Do not use `git commit -m` or `git commit -F`, because those forms intentionally skip generation.

## Configuration

The global file created in the Quick Start is located at `dirs::config_dir()/git-ai-hook/config.toml`, and `git ai-hook config path` prints its actual path. `git ai-hook config init` refuses to overwrite an existing global file unless `--force` is supplied. A repository can override individual settings with `.git-ai-hook.toml` at its worktree root.

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
# debug = false
# system_prompt = """
# <custom system prompt>
# """
```

`language` accepts BCP 47-style language tags such as `en`, `zh-CN`, and `pt-BR`. The configured language is appended to the system prompt as a required output-language instruction.

`debug` defaults to `false`. Set it to `true` to append the complete system and user prompts as comments after generated messages. The user prompt can include staged diff content.

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
GIT_AI_HOOK_DEBUG
```

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

## Hook Installation

When `git ai-hook install` runs in a Git repository, it discovers the repository with `git2`, respects `core.hooksPath`, and creates a `prepare-commit-msg` symlink to the running binary. If a different hook already exists, it is moved to `prepare-commit-msg.local` and runs first on every commit.

To make `git commit --no-edit` discard recorded `#` prompt lines reliably, installation sets the repository-local `commit.cleanup` to `strip`. The prior value is recorded under tool-specific Git config keys and restored during uninstall; a value you change after installation is left untouched.

## Additional Commands

- `generate` collects the staged diff and prints a clean commit message to stdout without changing Git files. With `debug = true`, it also prints the complete prompt block as comments.
- `generate --no-fallback` prints the model response without format validation, code-fence removal, or replacement with `chore: update N files`, so you can inspect invalid output. Leading and trailing whitespace is trimmed. With `debug = true`, prompt comments are still appended.
- `status` reports hook state, configuration sources, and whether an API key is present. It never prints key material.
- `config validate` validates the effective local configuration and sends a minimal real completion request to the configured model. It can incur a small provider charge.

## Hook Behavior

When Git invokes the hook, `git-ai-hook`:

1. Runs `prepare-commit-msg.local` first when present. A nonzero exit from that original hook is preserved.
2. Exits without generation when Git supplies an explicit message (`source == "message"`), `GIT_AI_HOOK_SKIP=1`, no API key is configured, or the user has already written non-comment content.
3. Collects the complete staged index diff. Binary and lock files contribute only file metadata, not patch content.
4. Builds the model prompt from the changed-file manifest and text patches. The full diff is collected first; `max_context_tokens` then conservatively limits the patch content sent to the model while retaining file metadata.
5. Shows an stderr spinner while the request is in flight and clears it on success, timeout, or failure.
6. Inserts the generated message first and preserves the original Git comment content unchanged. With `debug = true`, it inserts a comment block containing the complete system and user prompts between them.

With `debug = true`, the resulting message file has this shape:

```text
feat: add generated commit messages

# --- git-ai-hook prompt begin ---
# <complete system prompt>
# ---
# <complete user prompt>
# --- git-ai-hook prompt end ---
# <original Git comments>
```

Git ignores the prompt block because every line is a comment. With the default `debug = false`, the prompt block is omitted. The prompt block can contain staged-diff content, so avoid sharing an editor backup or temporary commit message file if that content is sensitive.

Model errors, timeouts, configuration errors, and message-file errors are logged under the user cache/config area and return success from this tool's hook path, so they do not block a commit. With the built-in system prompt, invalid model output falls back to `chore: update N files`; a customized `system_prompt` bypasses Conventional Commit validation.

For backward-incompatible changes, generated Conventional Commit subjects use `!` immediately after the type, for example `feat!: remove the legacy API`.

## Uninstall

Run this inside the repository:

```bash
git ai-hook uninstall
```

The command removes only a symlink recognized as managed by the current binary. If `prepare-commit-msg.local` exists, it is atomically restored to `prepare-commit-msg`; otherwise the managed link is removed.

## License

Licensed under the [Mulan Permissive Software License, Version 2](LICENSE) (`MulanPSL-2.0`).
