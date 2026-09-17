# git-ai-hook

[English](README.md) | [简体中文](README.zh-CN.md)

`git-ai-hook` 根据 Git 暂存区 diff 生成 Conventional Commit 风格的提交信息。它既可作为 CLI 使用，也可安装为 `prepare-commit-msg` 钩子。

> 平台支持：仅支持 Linux 和 macOS，**不支持 Windows**。

## 安装

先用 Cargo 构建并安装二进制：

```bash
cargo install --path .
```

在需要启用钩子的 Git 仓库中运行：

```bash
git ai-hook install
```

该命令通过 `git2` 发现仓库，优先使用 `core.hooksPath`，并创建指向当前二进制的 `prepare-commit-msg` 符号链接。如果原本存在其他钩子，会被移动到 `prepare-commit-msg.local`，每次提交时先执行它。

为确保 `git commit --no-edit` 能稳定忽略记录用的 `#` 提示词行，安装时会将仓库本地的 `commit.cleanup` 设为 `strip`。原值会写入工具专用 Git 配置键，并在卸载时恢复；安装后由用户自行修改的值不会被覆盖。

## 配置

创建全局配置文件：

```bash
git ai-hook config init
git ai-hook config path
```

全局配置路径为 `dirs::config_dir()/git-ai-hook/config.toml`。仓库工作区根目录下的 `.git-ai-hook.toml` 可以按字段覆盖全局配置。

```toml
[settings]
base_url = "https://api.openai.com/v1"
api_key = "sk-..."
model = "gpt-4o-mini"
# language = "zh-CN"
# reasoning_effort = "medium"
# max_context_tokens = 128000
# max_output_tokens = 16384
# timeout_secs = 60
# debug = false
# system_prompt = """
# <自定义 system prompt>
# """
```

`language` 使用 BCP 47 风格语言码，例如 `en`、`zh-CN` 和 `pt-BR`。该语言要求会自动追加到 system prompt。

`debug` 默认值为 `false`。设为 `true` 后，生成消息会在末尾附加完整的 system/user prompt 注释；user prompt 可能包含暂存 diff 内容。

每个字段独立按以下优先级合并：

1. 专用环境变量
2. 项目 `.git-ai-hook.toml`
3. 全局配置
4. 内置默认值

支持的环境变量：

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

只支持 `[settings]` 表。旧的 `[api]` 表会被拒绝，避免拼写错误时静默回退到默认值。

### OpenAI 兼容端点

客户端通过 Rig 的 OpenAI Chat Completions 兼容路径请求配置的 `base_url`。DeepSeek 示例：

```toml
[settings]
base_url = "https://api.deepseek.com"
api_key = "..."
model = "deepseek-chat"
language = "zh-CN"
```

Ollama OpenAI 兼容端点示例：

```toml
[settings]
base_url = "http://localhost:11434/v1"
api_key = "ollama"
model = "llama3.2"
language = "zh-CN"
```

## 命令

```text
git ai-hook install
git ai-hook uninstall
git ai-hook status
git ai-hook generate
git ai-hook config path
git ai-hook config init [--force]
git ai-hook config validate
```

- `generate` 收集暂存 diff，并只将干净的提交信息输出到 stdout，不修改 Git 文件。`debug = true` 时，还会输出包含完整提示词的注释块。
- `status` 显示钩子状态、配置来源和 API Key 是否存在，不会输出密钥内容。
- `config validate` 校验合并后的本地配置，并向已配置的模型发送一次最小真实请求；该操作可能产生少量模型费用。
- `config init` 默认不覆盖已有配置；使用 `--force` 才会替换。

## 钩子机制

Git 调用钩子时，`git-ai-hook` 会：

1. 若存在 `prepare-commit-msg.local`，先执行它；原钩子返回非零时保留该退出码。
2. 当 `source == "message"`、`GIT_AI_HOOK_SKIP=1`、缺少 API Key，或用户已经写入非注释内容时，直接跳过生成。
3. 完整收集暂存区 index diff。二进制文件和 lock 文件只记录文件元数据，不发送 patch。
4. 使用变更文件清单和文本 patch 构建提示词。完整 diff 会先收集到内存；之后 `max_context_tokens` 以保守估算限制实际发送的 patch 内容，同时保留文件清单。
5. 请求期间在 stderr 显示 spinner，并在成功、超时或失败时清除。
6. 将 AI 生成内容写在最前面，并原样保留 Git 原有注释内容。仅当 `debug = true` 时，才会在两者之间写入包含完整 system/user prompt 的注释块。

启用 `debug = true` 时，消息文件格式：

```text
feat: add generated commit messages

# --- git-ai-hook prompt begin ---
# <完整 system prompt>
# ---
# <完整 user prompt>
# --- git-ai-hook prompt end ---
# <原有 Git 注释>
```

由于提示词块的每一行都以 `#` 开头，Git 会忽略它。默认 `debug = false` 时不会写入提示词块。因此 `git commit --no-edit` 会直接使用生成内容，同时编辑器仍可看到完整上下文。提示词块可能含有暂存 diff；若 diff 敏感，请注意编辑器备份和临时提交信息文件。

模型错误、超时、配置错误和消息文件错误会写入用户缓存/配置目录下的日志，本工具的钩子路径始终返回成功，不阻塞提交。模型输出不符合格式时会回退为 `chore: update N files`。

如果变更破坏向后兼容性，生成的 Conventional Commit 首行会在 type 后紧跟 `!`，例如 `feat!: remove the legacy API`。

## 卸载

在目标仓库中运行：

```bash
git ai-hook uninstall
```

该命令只会删除当前二进制能够识别的受管符号链接。若存在 `prepare-commit-msg.local`，会原子地恢复为 `prepare-commit-msg`；否则仅移除本工具创建的链接。

## 许可证

本项目采用 [木兰宽松许可证，第 2 版](LICENSE)（`MulanPSL-2.0`）。
