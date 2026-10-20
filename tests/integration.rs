use git2::Repository;
use std::env;
use std::fs;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread;
use std::time::Duration;

static NEXT_TEMP_ID: AtomicUsize = AtomicUsize::new(0);

struct MockServer {
    base_url: String,
    handle: thread::JoinHandle<Vec<String>>,
}

impl MockServer {
    fn start(request_count: usize, status: u16) -> Self {
        Self::start_with_content(
            request_count,
            status,
            "feat: generate commit message\n\n- include staged changes",
        )
    }

    fn start_with_content(request_count: usize, status: u16, content: &str) -> Self {
        let body = if status == 200 {
            serde_json::json!({
                "id": "mock",
                "object": "chat.completion",
                "created": 0,
                "model": "mock-model",
                "choices": [{
                    "index": 0,
                    "message": {"role": "assistant", "content": content},
                    "finish_reason": "stop",
                }],
                "usage": {"prompt_tokens": 1, "completion_tokens": 1, "total_tokens": 2},
            })
            .to_string()
        } else {
            r#"{"error":{"message":"mock failure"}}"#.to_string()
        };
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind mock server");
        let address = listener.local_addr().expect("mock address");
        let handle = thread::spawn(move || {
            let mut requests = Vec::new();
            for _ in 0..request_count {
                let (mut stream, _) = listener.accept().expect("accept request");
                stream
                    .set_read_timeout(Some(Duration::from_secs(5)))
                    .expect("set timeout");
                let request = read_http_request(&mut stream);
                requests.push(request);
                let reason = if status == 200 {
                    "OK"
                } else {
                    "Internal Server Error"
                };
                write!(
                    stream,
                    "HTTP/1.1 {status} {reason}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                )
                .expect("write response");
            }
            requests
        });
        Self {
            base_url: format!("http://{address}"),
            handle,
        }
    }

    fn finish(self) -> Vec<String> {
        self.handle.join().expect("mock server panicked")
    }
}

fn read_http_request(stream: &mut TcpStream) -> String {
    let mut bytes = Vec::new();
    let mut buffer = [0_u8; 4096];
    loop {
        let read = stream.read(&mut buffer).expect("read request");
        if read == 0 {
            break;
        }
        bytes.extend_from_slice(&buffer[..read]);
        let Some(header_end) = find_header_end(&bytes) else {
            continue;
        };
        let headers = String::from_utf8_lossy(&bytes[..header_end]);
        let content_length = headers
            .lines()
            .find_map(|line| line.strip_prefix("Content-Length: "))
            .and_then(|value| value.trim().parse::<usize>().ok())
            .unwrap_or(0);
        if bytes.len() >= header_end + 4 + content_length {
            break;
        }
    }
    String::from_utf8_lossy(&bytes).into_owned()
}

fn find_header_end(bytes: &[u8]) -> Option<usize> {
    bytes.windows(4).position(|window| window == b"\r\n\r\n")
}

fn binary() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_git-ai-hook"))
}

fn temporary_directory(name: &str) -> PathBuf {
    let id = NEXT_TEMP_ID.fetch_add(1, Ordering::Relaxed);
    let path = env::temp_dir().join(format!("git-ai-hook-{name}-{}-{id}", std::process::id()));
    fs::create_dir_all(&path).expect("create temporary directory");
    path
}

fn initialize_repository(name: &str) -> (PathBuf, Repository) {
    let path = temporary_directory(name);
    let repository = Repository::init(&path).expect("initialize repository");
    repository
        .config()
        .expect("repository config")
        .set_str("user.name", "Git AI Hook Test")
        .expect("set user name");
    repository
        .config()
        .expect("repository config")
        .set_str("user.email", "test@example.invalid")
        .expect("set user email");
    (path, repository)
}

fn stage_file(repository: &Repository, root: &Path, relative: &str, contents: &str) {
    let path = root.join(relative);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("create parent directories");
    }
    fs::write(&path, contents).expect("write staged file");
    let mut index = repository.index().expect("open index");
    index.add_path(Path::new(relative)).expect("add index path");
    index.write().expect("write index");
}

fn write_project_config(root: &Path, base_url: &str, debug: bool) {
    fs::write(
        root.join(".git-ai-hook.toml"),
        format!(
            "[settings]\nbase_url = \"{base_url}\"\napi_key = \"test-key\"\nmodel = \"mock-model\"\nlanguage = \"zh-CN\"\nreasoning_effort = \"medium\"\nmax_context_tokens = 4096\nmax_output_tokens = 64\ntimeout_secs = 5\ndebug = {debug}\n"
        ),
    )
    .expect("write project config");
}

fn child(command: &mut Command, config_root: &Path) -> std::process::Output {
    child_with_debug(command, config_root, None)
}

fn hook_log_path(config_root: &Path) -> PathBuf {
    let cache_root = if cfg!(target_os = "macos") {
        config_root.join("Library/Caches")
    } else {
        config_root.to_path_buf()
    };
    cache_root.join("git-ai-hook/git-ai-hook.log")
}

fn child_with_debug(
    command: &mut Command,
    config_root: &Path,
    debug: Option<&str>,
) -> std::process::Output {
    for variable in [
        "GIT_AI_HOOK_BASE_URL",
        "GIT_AI_HOOK_API_KEY",
        "GIT_AI_HOOK_MODEL",
        "GIT_AI_HOOK_LANGUAGE",
        "GIT_AI_HOOK_REASONING_EFFORT",
        "GIT_AI_HOOK_MAX_CONTEXT_TOKENS",
        "GIT_AI_HOOK_MAX_OUTPUT_TOKENS",
        "GIT_AI_HOOK_TIMEOUT_SECS",
        "GIT_AI_HOOK_SYSTEM_PROMPT",
        "GIT_AI_HOOK_DEBUG",
    ] {
        command.env_remove(variable);
    }
    if let Some(debug) = debug {
        command.env("GIT_AI_HOOK_DEBUG", debug);
    }
    command
        .env("HOME", config_root)
        .env("XDG_CONFIG_HOME", config_root)
        .env("XDG_CACHE_HOME", config_root)
        .output()
        .expect("run child")
}

#[test]
fn debug_hook_generates_prompt_block_and_commit_no_edit_uses_message() {
    let (root, repository) = initialize_repository("hook-generation");
    stage_file(&repository, &root, "src/example.rs", "pub fn added() {}\n");
    stage_file(&repository, &root, "Cargo.lock", "lockfile-version = 1\n");
    stage_file(
        &repository,
        &root,
        "assets/image.bin",
        "\0not a text patch\n",
    );
    let mock = MockServer::start(3, 200);
    write_project_config(&root, &mock.base_url, true);
    let config_root = root.join("xdg-config");
    let message_path = root.join(".git/COMMIT_EDITMSG");
    fs::write(
        &message_path,
        "# Please enter the commit message.\n# Second comment.\n",
    )
    .expect("write message template");

    let mut validate = Command::new(binary());
    validate.current_dir(&root).args(["config", "validate"]);
    let validate_output = child(&mut validate, &config_root);
    assert!(validate_output.status.success(), "{:?}", validate_output);

    let mut install = Command::new(binary());
    install.current_dir(&root).arg("install");
    let install_output = child(&mut install, &config_root);
    assert!(install_output.status.success(), "{:?}", install_output);

    let hook = root.join(".git/hooks/prepare-commit-msg");
    assert!(
        fs::symlink_metadata(&hook)
            .expect("hook metadata")
            .file_type()
            .is_symlink()
    );
    let mut invoke_hook = Command::new(&hook);
    invoke_hook
        .current_dir(&root)
        .arg(&message_path)
        .arg("template");
    let hook_output = child(&mut invoke_hook, &config_root);
    assert!(hook_output.status.success(), "{:?}", hook_output);

    let message = fs::read_to_string(&message_path).expect("read generated message");
    assert!(message.starts_with("feat: generate commit message\n\n"));
    assert!(message.contains("# --- git-ai-hook prompt begin ---"));
    assert!(message.contains(
        "# Write the commit message in the language identified by the BCP 47 tag \"zh-CN\"."
    ));
    assert!(message.contains("# Please enter the commit message.\n# Second comment.\n"));

    let mut commit = Command::new("git");
    commit.current_dir(&root).args(["commit", "--no-edit"]);
    let commit_output = child(&mut commit, &config_root);
    assert!(commit_output.status.success(), "{:?}", commit_output);

    let requests = mock.finish();
    assert_eq!(requests.len(), 3);
    let hook_request = &requests[1];
    assert!(hook_request.contains("POST /chat/completions HTTP/1.1"));
    assert!(hook_request.contains("mock-model"));
    assert!(hook_request.contains("reasoning_effort"));
    assert!(hook_request.contains("src/example.rs"));
    assert!(hook_request.contains("Cargo.lock (lock)"));
    assert!(hook_request.contains("assets/image.bin (binary)"));
    assert!(!hook_request.contains("lockfile-version"));
    assert!(!hook_request.contains("not a text patch"));

    let log_output = Command::new("git")
        .current_dir(&root)
        .args(["log", "-1", "--format=%B"])
        .output()
        .expect("read commit message");
    let committed = String::from_utf8(log_output.stdout).expect("utf8 commit message");
    assert!(committed.starts_with("feat: generate commit message"));
    assert!(!committed.contains("git-ai-hook prompt"));

    fs::remove_dir_all(root).expect("remove temporary repository");
}

#[test]
fn hook_omits_prompt_block_by_default() {
    let (root, repository) = initialize_repository("hook-without-debug");
    stage_file(&repository, &root, "src/default.rs", "pub fn added() {}\n");
    let mock = MockServer::start(1, 200);
    write_project_config(&root, &mock.base_url, false);
    let config_root = root.join("xdg-config");
    let message_path = root.join(".git/COMMIT_EDITMSG");
    let original = "# Please enter the commit message.\n";
    fs::write(&message_path, original).expect("write message template");

    let mut install = Command::new(binary());
    install.current_dir(&root).arg("install");
    assert!(child(&mut install, &config_root).status.success());

    let hook = root.join(".git/hooks/prepare-commit-msg");
    let mut invoke_hook = Command::new(&hook);
    invoke_hook
        .current_dir(&root)
        .arg(&message_path)
        .arg("template");
    let output = child(&mut invoke_hook, &config_root);
    assert!(output.status.success(), "{:?}", output);

    let message = fs::read_to_string(&message_path).expect("read generated message");
    assert_eq!(
        message,
        format!("feat: generate commit message\n\n- include staged changes\n\n{original}")
    );
    assert!(!message.contains("git-ai-hook prompt"));
    assert!(!message.contains("src/default.rs"));
    assert_eq!(mock.finish().len(), 1);

    fs::remove_dir_all(root).expect("remove temporary repository");
}

#[test]
fn debug_environment_override_adds_prompt_block() {
    let (root, repository) = initialize_repository("debug-environment");
    stage_file(
        &repository,
        &root,
        "src/environment.rs",
        "pub fn added() {}\n",
    );
    let mock = MockServer::start(1, 200);
    write_project_config(&root, &mock.base_url, false);
    let config_root = root.join("xdg-config");
    let message_path = root.join(".git/COMMIT_EDITMSG");
    fs::write(&message_path, "# Please enter the commit message.\n")
        .expect("write message template");

    let mut install = Command::new(binary());
    install.current_dir(&root).arg("install");
    assert!(child(&mut install, &config_root).status.success());

    let hook = root.join(".git/hooks/prepare-commit-msg");
    let mut invoke_hook = Command::new(&hook);
    invoke_hook
        .current_dir(&root)
        .arg(&message_path)
        .arg("template");
    let output = child_with_debug(&mut invoke_hook, &config_root, Some("true"));
    assert!(output.status.success(), "{:?}", output);

    let message = fs::read_to_string(&message_path).expect("read generated message");
    assert!(message.contains("# --- git-ai-hook prompt begin ---"));
    assert!(message.contains("# diff --git a/src/environment.rs b/src/environment.rs"));
    assert_eq!(mock.finish().len(), 1);

    fs::remove_dir_all(root).expect("remove temporary repository");
}

#[test]
fn generate_emits_prompt_block_only_when_debug_enabled() {
    let (root, repository) = initialize_repository("generate-debug-output");
    stage_file(&repository, &root, "src/generate.rs", "pub fn added() {}\n");
    let mock = MockServer::start(2, 200);
    let config_root = root.join("xdg-config");

    write_project_config(&root, &mock.base_url, false);
    let mut clean_generate = Command::new(binary());
    clean_generate.current_dir(&root).arg("generate");
    let clean_output = child(&mut clean_generate, &config_root);
    assert!(clean_output.status.success(), "{:?}", clean_output);
    let clean_stdout = String::from_utf8(clean_output.stdout).expect("clean stdout is UTF-8");
    assert_eq!(
        clean_stdout,
        "feat: generate commit message\n\n- include staged changes\n"
    );

    write_project_config(&root, &mock.base_url, true);
    let mut debug_generate = Command::new(binary());
    debug_generate.current_dir(&root).arg("generate");
    let debug_output = child(&mut debug_generate, &config_root);
    assert!(debug_output.status.success(), "{:?}", debug_output);
    let debug_stdout = String::from_utf8(debug_output.stdout).expect("debug stdout is UTF-8");
    assert!(
        debug_stdout.starts_with("feat: generate commit message\n\n- include staged changes\n\n")
    );
    assert!(debug_stdout.contains("# --- git-ai-hook prompt begin ---"));
    assert!(debug_stdout.contains("# diff --git a/src/generate.rs b/src/generate.rs"));
    assert_eq!(mock.finish().len(), 2);

    fs::remove_dir_all(root).expect("remove temporary repository");
}

#[test]
fn generate_no_fallback_preserves_invalid_model_response_and_code_fences() {
    let (root, repository) = initialize_repository("generate-no-fallback");
    stage_file(&repository, &root, "src/generate.rs", "pub fn added() {}\n");
    let response = "```text\nThis is not a Conventional Commit\n```";
    let mock = MockServer::start_with_content(3, 200, response);
    let config_root = root.join("xdg-config");
    write_project_config(&root, &mock.base_url, false);

    let mut default_generate = Command::new(binary());
    default_generate.current_dir(&root).arg("generate");
    let default_output = child(&mut default_generate, &config_root);
    assert!(default_output.status.success(), "{:?}", default_output);
    assert_eq!(
        String::from_utf8(default_output.stdout).expect("default stdout is UTF-8"),
        "chore: update 1 files\n"
    );

    let mut raw_generate = Command::new(binary());
    raw_generate
        .current_dir(&root)
        .args(["generate", "--no-fallback"]);
    let raw_output = child(&mut raw_generate, &config_root);
    assert!(raw_output.status.success(), "{:?}", raw_output);
    assert_eq!(
        String::from_utf8(raw_output.stdout).expect("raw stdout is UTF-8"),
        format!("{response}\n")
    );

    write_project_config(&root, &mock.base_url, true);
    let mut debug_generate = Command::new(binary());
    debug_generate
        .current_dir(&root)
        .args(["generate", "--no-fallback"]);
    let debug_output = child(&mut debug_generate, &config_root);
    assert!(debug_output.status.success(), "{:?}", debug_output);
    let debug_stdout = String::from_utf8(debug_output.stdout).expect("debug stdout is UTF-8");
    assert!(debug_stdout.starts_with(&format!("{response}\n\n")));
    assert!(debug_stdout.contains("# --- git-ai-hook prompt begin ---"));
    assert!(debug_stdout.contains("# diff --git a/src/generate.rs b/src/generate.rs"));
    assert_eq!(mock.finish().len(), 3);

    fs::remove_dir_all(root).expect("remove temporary repository");
}

#[test]
fn install_backs_up_and_uninstall_restores_original_hook() {
    let (root, repository) = initialize_repository("restore-hook");
    repository
        .config()
        .expect("repository config")
        .set_str("commit.cleanup", "verbatim")
        .expect("set previous cleanup mode");
    let config_root = root.join("xdg-config");
    let hook = root.join(".git/hooks/prepare-commit-msg");
    fs::write(&hook, "#!/bin/sh\nexit 0\n").expect("write original hook");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&hook, fs::Permissions::from_mode(0o755))
            .expect("make hook executable");
    }

    let mut install = Command::new(binary());
    install.current_dir(&root).arg("install");
    let install_output = child(&mut install, &config_root);
    assert!(install_output.status.success(), "{:?}", install_output);
    let backup = root.join(".git/hooks/prepare-commit-msg.local");
    assert!(backup.exists());
    assert!(
        fs::symlink_metadata(&hook)
            .expect("managed hook metadata")
            .file_type()
            .is_symlink()
    );
    assert_eq!(
        Repository::open(&root)
            .expect("reopen repository")
            .config()
            .expect("repository config")
            .get_string("commit.cleanup")
            .expect("managed cleanup mode"),
        "strip"
    );

    let mut uninstall = Command::new(binary());
    uninstall.current_dir(&root).arg("uninstall");
    let uninstall_output = child(&mut uninstall, &config_root);
    assert!(uninstall_output.status.success(), "{:?}", uninstall_output);
    assert_eq!(
        fs::read_to_string(&hook).expect("restored hook"),
        "#!/bin/sh\nexit 0\n"
    );
    assert!(!backup.exists());
    assert_eq!(
        Repository::open(&root)
            .expect("reopen repository")
            .config()
            .expect("repository config")
            .get_string("commit.cleanup")
            .expect("restored cleanup mode"),
        "verbatim"
    );

    fs::remove_dir_all(root).expect("remove temporary repository");
}

#[test]
fn original_hook_failure_is_preserved_and_manual_message_is_not_replaced() {
    let (root, _repository) = initialize_repository("hook-semantics");
    let config_root = root.join("xdg-config");
    let hook = root.join(".git/hooks/prepare-commit-msg");
    fs::write(&hook, "#!/bin/sh\nexit 23\n").expect("write failing hook");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&hook, fs::Permissions::from_mode(0o755))
            .expect("make hook executable");
    }

    let mut install = Command::new(binary());
    install.current_dir(&root).arg("install");
    assert!(child(&mut install, &config_root).status.success());

    let message_path = root.join(".git/COMMIT_EDITMSG");
    fs::write(&message_path, "fix: manual message\n").expect("write manual message");
    let managed_hook = root.join(".git/hooks/prepare-commit-msg");
    let mut invoke = Command::new(&managed_hook);
    invoke.current_dir(&root).arg(&message_path).arg("template");
    let output = child(&mut invoke, &config_root);
    assert_eq!(output.status.code(), Some(23));
    assert_eq!(
        fs::read_to_string(&message_path).expect("read manual message"),
        "fix: manual message\n"
    );

    fs::remove_dir_all(root).expect("remove temporary repository");
}

#[test]
fn model_failure_is_logged_without_blocking_the_commit() {
    let (root, repository) = initialize_repository("nonblocking-failure");
    stage_file(
        &repository,
        &root,
        "src/failure.rs",
        "pub fn changed() {}\n",
    );
    let mock = MockServer::start(1, 500);
    write_project_config(&root, &mock.base_url, false);
    let config_root = root.join("xdg-config");
    let message_path = root.join(".git/COMMIT_EDITMSG");
    let original = "# Git comment that must remain.\n";
    fs::write(&message_path, original).expect("write message template");

    let mut install = Command::new(binary());
    install.current_dir(&root).arg("install");
    assert!(child(&mut install, &config_root).status.success());

    let managed_hook = root.join(".git/hooks/prepare-commit-msg");
    let mut invoke = Command::new(&managed_hook);
    invoke.current_dir(&root).arg(&message_path).arg("template");
    let output = child(&mut invoke, &config_root);
    assert!(output.status.success(), "{:?}", output);
    assert_eq!(
        fs::read_to_string(&message_path).expect("read message"),
        original
    );
    assert_eq!(mock.finish().len(), 1);

    let log = hook_log_path(&config_root);
    assert!(log.exists(), "expected hook error log at {}", log.display());
    assert!(
        fs::read_to_string(log)
            .expect("read hook error log")
            .contains("prepare-commit-msg")
    );

    fs::remove_dir_all(root).expect("remove temporary repository");
}

#[test]
fn user_written_message_skips_generation_without_contacting_the_model() {
    let (root, repository) = initialize_repository("manual-message");
    stage_file(&repository, &root, "src/manual.rs", "pub fn manual() {}\n");
    let listener = TcpListener::bind("127.0.0.1:0").expect("reserve endpoint port");
    let unavailable_base_url = format!(
        "http://{}",
        listener.local_addr().expect("endpoint address")
    );
    drop(listener);
    write_project_config(&root, &unavailable_base_url, false);
    let config_root = root.join("xdg-config");
    let message_path = root.join(".git/COMMIT_EDITMSG");
    let original = "fix: keep the user-written message\n";
    fs::write(&message_path, original).expect("write manual message");

    let mut install = Command::new(binary());
    install.current_dir(&root).arg("install");
    assert!(child(&mut install, &config_root).status.success());

    let managed_hook = root.join(".git/hooks/prepare-commit-msg");
    let mut invoke = Command::new(&managed_hook);
    invoke.current_dir(&root).arg(&message_path).arg("template");
    let output = child(&mut invoke, &config_root);
    assert!(output.status.success(), "{:?}", output);
    assert_eq!(
        fs::read_to_string(&message_path).expect("read manual message"),
        original
    );
    assert!(!hook_log_path(&config_root).exists());

    fs::remove_dir_all(root).expect("remove temporary repository");
}
