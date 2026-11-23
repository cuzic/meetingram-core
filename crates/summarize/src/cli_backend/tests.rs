use super::*;
use std::future::Future;

#[test]
fn cleanup_preserves_recent_legacy_and_unrelated_directories() {
    let base = tempfile::tempdir().unwrap();
    let stale = base.path().join("meetingram-cli-abandoned");
    let unrelated = base.path().join("other");
    std::fs::create_dir(&stale).unwrap();
    std::fs::create_dir(&unrelated).unwrap();
    std::fs::write(stale.join("system.txt"), "synthetic instruction").unwrap();
    cleanup_stale_cli_dirs(base.path()).unwrap();
    assert!(stale.exists());
    assert!(unrelated.exists());
}

#[cfg(unix)]
#[test]
fn cleanup_removes_old_legacy_cli_directories() {
    let base = tempfile::tempdir().unwrap();
    let old = base.path().join("meetingram-cli-abandoned");
    std::fs::create_dir(&old).unwrap();
    std::fs::File::open(&old)
        .unwrap()
        .set_modified(SystemTime::now() - STALE_DIR_AGE - Duration::from_secs(1))
        .unwrap();

    cleanup_stale_cli_dirs(base.path()).unwrap();
    assert!(!old.exists());
}

#[cfg(any(target_os = "linux", target_os = "macos", windows))]
#[test]
fn cleanup_preserves_live_process_and_removes_its_dir_after_exit() {
    use std::process::{Command, Stdio};

    let base = tempfile::tempdir().unwrap();
    let mut child = Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "cli_backend::tests::wait_for_cleanup_test_parent",
            "--ignored",
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .spawn()
        .unwrap();
    let pid = child.id();
    let start = match platform::process_start(pid) {
        ProcessStart::Known(start) => start,
        _ => {
            child.kill().unwrap();
            child.wait().unwrap();
            panic!("fixture process start time unavailable");
        }
    };
    let live = base.path().join(format!("{pid}-{start}"));
    let own = base.path().join(process_dir_name());
    let reused = base.path().join(format!("{pid}-{}", start + 1));

    for path in [&live, &own, &reused] {
        std::fs::create_dir_all(path.join("meetingram-cli-fixture")).unwrap();
    }
    let cleanup = cleanup_stale_cli_dirs(base.path());
    let live_preserved = live.exists();
    child.kill().unwrap();
    child.wait().unwrap();

    cleanup.unwrap();
    assert!(live_preserved);
    assert!(own.exists());
    assert!(!reused.exists());
    cleanup_stale_cli_dirs(base.path()).unwrap();
    assert!(!live.exists());
}

#[cfg(any(target_os = "linux", target_os = "macos", windows))]
#[test]
#[ignore = "launched as a live process by the startup cleanup test"]
fn wait_for_cleanup_test_parent() {
    io::copy(&mut io::stdin().lock(), &mut io::sink()).unwrap();
}

#[test]
fn cli_argument_sequences() {
    let dir = Path::new("temp space 日本語");
    let lean = cli_args(CliBackend::ClaudeCode, dir, Some("sonnet"), false);
    assert_eq!(
        lean,
        [
            "--print",
            "--output-format",
            "json",
            "--no-session-persistence",
            "--tools",
            "",
            "--strict-mcp-config",
            "--system-prompt-file",
            "system.txt",
            "--model",
            "sonnet"
        ]
        .map(OsString::from),
    );
    let compat = cli_args(CliBackend::ClaudeCode, dir, Some("sonnet"), true);
    assert_eq!(
        compat,
        [
            "--print",
            "--output-format",
            "json",
            "--no-session-persistence",
            "--disallowedTools",
            "Bash,Edit,Write,Read,WebFetch,WebSearch,Task,Glob,Grep,NotebookEdit",
            "--model",
            "sonnet"
        ]
        .map(OsString::from),
    );
    let codex = cli_args(CliBackend::Codex, dir, Some("gpt-5.5"), false);
    let mut expected = [
        "exec",
        "-",
        "--json",
        "--ephemeral",
        "--skip-git-repo-check",
        "-s",
        "read-only",
        "-C",
        "temp space 日本語",
        "-o",
    ]
    .map(OsString::from)
    .to_vec();
    expected.push(dir.join("out.txt").into_os_string());
    expected.extend(
        [
            "-c",
            "model_instructions_file=\"system.txt\"",
            "-c",
            "project_doc_max_bytes=0",
            "-c",
            "mcp_servers={}",
            "-c",
            "web_search=\"disabled\"",
            "-c",
            "features.shell_tool=false",
            "-c",
            "features.view_image=false",
            "-c",
            "features.apps=false",
            "-c",
            "features.multi_agent=false",
            "-c",
            "features.code_mode_host=false",
            "-c",
            "features.image_generation=false",
            "-c",
            "features.plugins=false",
            "-c",
            "features.goals=false",
            "-c",
            "features.browser_use=false",
            "-c",
            "features.computer_use=false",
            "-c",
            "features.unified_exec=false",
            "-m",
            "gpt-5.5",
        ]
        .map(OsString::from),
    );
    assert_eq!(codex, expected);
}

fn run_fixture<'a>(
    backend: CliBackend,
    binary: &'a Path,
    user: &'a str,
    base: &'a Path,
    timeout: Duration,
    compatibility: &'a AtomicBool,
) -> impl Future<Output = Result<String, CliSummarizeError>> + 'a {
    run_cli(
        backend,
        binary,
        CliInput {
            system: "synthetic instruction",
            user,
            model: None,
        },
        Some(base),
        timeout,
        compatibility,
        None,
    )
}

#[cfg(unix)]
#[tokio::test]
async fn herdr_sends_prompt_on_stdin_and_accepts_cli_result() {
    let dir = tempfile::tempdir().unwrap();
    use crate::Summarizer;
    let hc = script(
        dir.path(),
        r#"
[ "$HC_SESSION:$1" = "synthetic:connect" ] || exit 41
[ -z "${ANTHROPIC_API_KEY+x}${OPENAI_BASE_URL+x}${HC_SOCKET+x}" ] || exit 46
IFS= read -r identity || exit 40
case "$identity" in *'"method":"server.status"'*) ;; *) exit 40 ;; esac
printf '%s\n' '{"jsonrpc":"2.0","id":1,"result":{"core":{"id":"test-core"},"pid":123}}'
IFS= read -r auth || exit 42
case "$auth" in *'"input":null'*) ;; *) exit 43 ;; esac
printf '%s\n' '{"jsonrpc":"2.0","id":2,"result":{"code":0,"stdout":"eyJsb2dnZWRJbiI6dHJ1ZSwiYXV0aE1ldGhvZCI6ImNsYXVkZS5haSIsImFwaVByb3ZpZGVyIjoiZmlyc3RQYXJ0eSJ9","stderr":"","timed_out":false,"truncated":false}}'
[ ! -e "$(dirname "$0")/disconnect" ] || exit 0
IFS= read -r request || exit 44
case "$request" in *'"input":"c3ludGhldGljIHRyYW5zY3JpcHQ="'*) ;; *) exit 45 ;; esac
printf '%s\n' '{"jsonrpc":"2.0","id":3,"result":{"code":0,"stdout":"eyJpc19lcnJvciI6ZmFsc2UsInJlc3VsdCI6InN5bnRoZXRpYyByZXN1bHQifQ==","stderr":"","timed_out":false,"truncated":false}}'
"#,
    );
    let summarizer = HerdrSummarizer::new(
        CliBackend::ClaudeCode,
        hc,
        "synthetic".into(),
        PathBuf::from("/synthetic/claude"),
        dir.path().to_path_buf(),
        Arc::new(FixedPreflight(true)),
    );
    let result = summarizer
        .complete(&CompletionRequest::new(
            "synthetic instruction",
            "synthetic transcript",
            "",
            Duration::from_secs(3),
        ))
        .await
        .unwrap();
    assert_eq!(result, "synthetic result");
    std::fs::write(dir.path().join("disconnect"), []).unwrap();
    let req = CompletionRequest::new(
        "instruction",
        "synthetic transcript",
        "",
        Duration::from_secs(3),
    );
    assert!(matches!(
        summarizer.complete(&req).await,
        Err(SummarizerError::Cli(CliSummarizeError::ReadOutput(..)))
    ));
    assert!(matches!(
        summarizer.complete(&req).await,
        Err(SummarizerError::HerdrStopped)
    ));
}

#[tokio::test]
async fn herdr_codex_requires_completed_turn() {
    let partial =
        br#"{"type":"item.completed","item":{"type":"agent_message","text":"synthetic"}}"#;
    assert!(matches!(
        read_stdout(CliBackend::Codex, partial.as_slice(), true)
            .await
            .unwrap(),
        CliStdout::MalformedCodexEvent
    ));
    let completed = [partial.as_slice(), b"\n", br#"{"type":"turn.completed"}"#].concat();
    assert!(matches!(
        read_stdout(CliBackend::Codex, completed.as_slice(), true)
            .await
            .unwrap(),
        CliStdout::Codex
    ));
}

#[test]
fn herdr_recognizes_subscription_authentication_only() {
    let claude = |output| subscription_status(CliBackend::ClaudeCode, output, b"");
    assert!(claude(
        br#"{"loggedIn":true,"authMethod":"claude.ai","apiProvider":"firstParty"}"#
    ));
    assert!(!claude(
        br#"{"loggedIn":true,"authMethod":"api_key","apiProvider":"firstParty"}"#
    ));
    assert!(!claude(
        br#"{"loggedIn":true,"authMethod":"oauth_token","apiProvider":"firstParty"}"#
    ));
    assert!(!claude(
        br#"{"loggedIn":true,"authMethod":"third_party","apiProvider":"bedrock"}"#
    ));
    assert!(!claude(b"unknown"));
    let codex = |output| subscription_status(CliBackend::Codex, b"", output);
    assert!(codex(b"Logged in using ChatGPT\n"));
    assert!(!codex(b"Logged in using an API key\n"));
    assert!(!codex(b"Not logged in\n"));
}

#[cfg(unix)]
struct FixedPreflight(bool);

#[cfg(unix)]
#[async_trait::async_trait]
impl HerdrPreflight for FixedPreflight {
    async fn verify(&self, session: &str, core_id: &str, pid: u32) -> bool {
        self.0 && session == "synthetic" && core_id == "test-core" && pid == 123
    }
}

#[cfg(unix)]
#[tokio::test]
async fn herdr_verifies_before_sending_and_does_not_resend_after_failure() {
    use crate::Summarizer;

    let dir = tempfile::tempdir().unwrap();
    let hc = script(
        dir.path(),
        r#"
[ "$HC_SESSION:$1" = "synthetic:connect" ] || exit 49
IFS= read -r identity || exit 40
case "$identity" in *'"method":"server.status"'*) ;; *) exit 40 ;; esac
printf '%s\n' '{"jsonrpc":"2.0","id":1,"result":{"core":{"id":"test-core"},"pid":123}}'
IFS= read -r request || exit 42
printf '%s\n' '{"jsonrpc":"2.0","id":2,"result":{"code":0,"stdout":"eyJsb2dnZWRJbiI6dHJ1ZSwiYXV0aE1ldGhvZCI6ImNsYXVkZS5haSIsImFwaVByb3ZpZGVyIjoiZmlyc3RQYXJ0eSJ9","stderr":"","timed_out":false,"truncated":false}}'
IFS= read -r request || exit 0
printf x >> "$(dirname "$0")/attempts"
printf '%s\n' '{"jsonrpc":"2.0","id":3,"result":{"code":1,"stdout":"","stderr":"","timed_out":false,"truncated":false}}'
"#,
    );
    let req = CompletionRequest::new(
        "synthetic instruction",
        "synthetic data",
        "",
        Duration::from_secs(3),
    );
    let build = |verified| {
        HerdrSummarizer::new(
            CliBackend::ClaudeCode,
            hc.clone(),
            "synthetic".into(),
            PathBuf::from("/synthetic/claude"),
            dir.path().to_path_buf(),
            Arc::new(FixedPreflight(verified)),
        )
    };
    let unverified = build(false);
    assert!(matches!(
        unverified.complete(&req).await,
        Err(SummarizerError::HerdrUnverified)
    ));
    assert!(!dir.path().join("attempts").exists());

    let verified = build(true);
    assert!(matches!(
        verified.complete(&req).await,
        Err(SummarizerError::Cli(CliSummarizeError::ParseOutput(..)))
    ));
    assert!(matches!(
        verified.complete(&req).await,
        Err(SummarizerError::HerdrStopped)
    ));
    assert_eq!(std::fs::read(dir.path().join("attempts")).unwrap(), b"x");
}

#[cfg(unix)]
fn script(dir: &Path, contents: &str) -> PathBuf {
    let script = dir.join("sample-cli");
    // Open the fixture for writing only in the shell. Parallel tests that fork
    // from this process cannot inherit a write descriptor and cause ETXTBSY.
    let status = std::process::Command::new("/bin/sh")
        .args([
            "-c",
            r#"printf '#!/bin/sh\n%s' "$1" > "$2" && chmod 700 "$2""#,
            "--",
        ])
        .arg(contents)
        .arg(&script)
        .status()
        .unwrap();
    assert!(status.success());
    script
}

#[cfg(unix)]
#[tokio::test]
async fn claude_retries_unknown_option_once_and_remembers_compatibility() {
    let base = tempfile::tempdir().unwrap();
    let work = tempfile::tempdir().unwrap();
    let cli = script(
        base.path(),
        r#"
        attempt_log="$(dirname "$0")/attempts"
        case "$5" in
            --tools)
                echo lean >>"$attempt_log"
                printf "synthetic startup notice\nerror: unknown option '--tools'\n" >&2
                exit 1
                ;;
            --disallowedTools)
                echo compat >>"$attempt_log"
                test "$(cat)" = 'synthetic instruction

synthetic transcript' || exit 2
                printf '{"result":"synthetic summary"}\n'
                ;;
            *) exit 3 ;;
        esac
    "#,
    );
    let compat = AtomicBool::new(false);
    for _ in 0..2 {
        let response = run_fixture(
            CliBackend::ClaudeCode,
            &cli,
            "synthetic transcript",
            work.path(),
            Duration::from_secs(5),
            &compat,
        )
        .await
        .unwrap();
        assert_eq!(response, "synthetic summary");
        assert!(compat.load(Ordering::Relaxed));
    }
    assert_eq!(
        std::fs::read_to_string(base.path().join("attempts")).unwrap(),
        "lean\ncompat\ncompat\n"
    );
    assert!(working_dirs_are_gone(work.path()));
}

#[cfg(unix)]
#[tokio::test]
async fn codex_rejects_tool_use_before_exit_and_discards_output() {
    let base = tempfile::tempdir().unwrap();
    let work = tempfile::tempdir().unwrap();
    let cli = script(
        base.path(),
        r#"
        cat >/dev/null
        printf 'synthetic summary' >out.txt
        printf '%s\n' '{"type":"item.started","item":{"type":"agent_message"}}'
        printf '%s\n' '{"type":"item.started","item":{"type":"new_tool_type"}}'
        exec sleep 30
    "#,
    );
    let error = run_fixture(
        CliBackend::Codex,
        &cli,
        "synthetic transcript",
        work.path(),
        Duration::from_secs(3),
        &AtomicBool::new(false),
    )
    .await
    .unwrap_err();
    assert_eq!(crate::SummarizerError::from(error).kind(), "tool_use");
    assert!(working_dirs_are_gone(work.path()));
}

#[cfg(unix)]
#[tokio::test]
async fn early_exit_reports_status_and_drops_stderr() {
    let base = tempfile::tempdir().unwrap();
    let work = tempfile::tempdir().unwrap();
    let cli = script(base.path(), "echo 'synthetic-stderr-marker' >&2\nexit 7\n");
    for backend in [CliBackend::ClaudeCode, CliBackend::Codex] {
        let error = run_fixture(
            backend,
            &cli,
            &"x\n".repeat(100_000),
            work.path(),
            Duration::from_secs(5),
            &AtomicBool::new(false),
        )
        .await
        .unwrap_err();
        assert!(
            matches!(error, CliSummarizeError::NonZeroExit(_, status) if status.code() == Some(7))
        );
        assert!(!format!("{error} {error:?}").contains("synthetic-stderr-marker"));
        assert_eq!(crate::SummarizerError::from(error).kind(), "non_zero_exit");
    }
    assert!(working_dirs_are_gone(work.path()));
}

#[cfg(unix)]
#[tokio::test]
async fn timeout_and_drop_stop_grandchild_and_remove_work_directory() {
    let base = tempfile::tempdir().unwrap();
    let work = tempfile::tempdir().unwrap();
    let cli = script(
        base.path(),
        r#"
        sh -c 'echo "$$" > "$1"; exec sleep 30' sh "$(dirname "$0")/grandchild" &
        wait
    "#,
    );
    let pid_file = base.path().join("grandchild");
    let compat = AtomicBool::new(false);
    for cancel in [false, true] {
        let mut call = Box::pin(run_fixture(
            CliBackend::ClaudeCode,
            &cli,
            "synthetic transcript",
            work.path(),
            Duration::from_secs(if cancel { 10 } else { 1 }),
            &compat,
        ));
        let pid = tokio::select! {
            result = &mut call => panic!("CLI exited without reporting descendant: {result:?}"),
            pid = wait_for_pid(&pid_file) => pid,
        };
        if cancel {
            drop(call);
        } else {
            let error = call.await.unwrap_err();
            assert_eq!(crate::SummarizerError::from(error).kind(), "timeout");
        }
        let stopped = tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                let output = std::process::Command::new("ps")
                    .args(["-o", "stat=", "-p", &pid.to_string()])
                    .output()
                    .unwrap();
                // A killed orphan can remain as a zombie until init reaps it.
                if !output.status.success() || output.stdout.starts_with(b"Z") {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await;
        if stopped.is_err() {
            // SAFETY: pid is the still-running descendant started by this test.
            unsafe { libc::kill(pid as libc::pid_t, libc::SIGKILL) };
        }
        stopped.expect("grandchild survived process cleanup");
        wait_for_cleanup(work.path()).await;
        std::fs::remove_file(&pid_file).unwrap();
    }
}

fn working_dirs_are_gone(base: &Path) -> bool {
    match base.join(process_dir_name()).read_dir() {
        Ok(mut entries) => entries.next().is_none(),
        Err(error) if error.kind() == io::ErrorKind::NotFound => true,
        Err(error) => panic!("failed to read CLI process directory: {error}"),
    }
}

async fn wait_for_cleanup(work: &Path) {
    tokio::time::timeout(Duration::from_secs(2), async {
        while !working_dirs_are_gone(work) {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("CLI working directory survived cleanup");
}

#[cfg(windows)]
#[test]
fn windows_path_search_uses_absolute_directories_in_order() {
    let name = OsStr::new("fixture");
    let first = tempfile::tempdir().unwrap();
    let second = tempfile::tempdir().unwrap();
    let first_cmd = first.path().join("fixture.cmd");
    let first_exe = first.path().join("fixture.exe");
    std::fs::write(&first_cmd, "").unwrap();
    std::fs::write(second.path().join("fixture.exe"), "").unwrap();
    let path = std::env::join_paths([first.path(), second.path()]).unwrap();
    assert_eq!(platform::resolve_in_path(name, &path).unwrap(), first_cmd);
    std::fs::write(&first_exe, "").unwrap();
    assert_eq!(platform::resolve_in_path(name, &path).unwrap(), first_exe);
}

async fn wait_for_pid(path: &Path) -> u32 {
    loop {
        if let Some(pid) = std::fs::read_to_string(path)
            .ok()
            .and_then(|contents| contents.trim().parse().ok())
        {
            return pid;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

#[cfg(windows)]
#[test]
fn windows_resolves_batch_shim_and_stops_its_descendants() {
    let fixture = tempfile::tempdir().unwrap();
    let bin = fixture.path().join("CLI 日本語");
    let work = fixture.path().join("work 日本語");
    std::fs::create_dir(&bin).unwrap();
    std::fs::create_dir(&work).unwrap();
    std::fs::write(
        bin.join("claude.cmd"),
        r#"@echo off
if "%~1"=="--version" exit /b 0
"%~dp0fixture.exe" %*
exit /b %errorlevel%
"#
        .replace('\n', "\r\n"),
    )
    .unwrap();
    let source = bin.join("fixture.rs");
    std::fs::write(&source, include_str!("windows_fixture.rs")).unwrap();
    let build = std::process::Command::new("rustc")
        .arg(&source)
        .arg("-o")
        .arg(bin.join("fixture.exe"))
        .output()
        .unwrap();
    assert!(
        build.status.success(),
        "fixture compilation failed: {}",
        String::from_utf8_lossy(&build.stderr)
    );
    let paths = std::env::join_paths(std::iter::once(bin.clone()).chain(std::env::split_paths(
        &std::env::var_os("PATH").unwrap_or_default(),
    )))
    .unwrap();
    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "cli_backend::tests::windows_fixture_subprocess",
            "--ignored",
            "--nocapture",
        ])
        .env("PATH", paths)
        .env("MEETINGRAM_TEST_CLI_BIN", bin)
        .env("MEETINGRAM_TEST_CLI_WORK", work)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[cfg(windows)]
#[tokio::test]
#[ignore = "launched by the Windows test with an isolated PATH"]
async fn windows_fixture_subprocess() {
    use std::os::windows::io::{FromRawHandle, OwnedHandle};
    use windows_sys::Win32::Foundation::{ERROR_INVALID_PARAMETER, WAIT_OBJECT_0};
    use windows_sys::Win32::System::Threading::{
        OpenProcess, WaitForSingleObject, PROCESS_SYNCHRONIZE,
    };

    let Some(bin) = std::env::var_os("MEETINGRAM_TEST_CLI_BIN").map(PathBuf::from) else {
        return;
    };
    let work = PathBuf::from(std::env::var_os("MEETINGRAM_TEST_CLI_WORK").unwrap());
    let name = OsStr::new("claude");
    let original_dir = std::env::current_dir().unwrap();
    std::env::set_current_dir(bin.parent().unwrap()).unwrap();
    let relative = Path::new(bin.file_name().unwrap());
    let relative_only = std::env::join_paths([relative]).unwrap();
    assert_eq!(
        platform::resolve_in_path(name, &relative_only)
            .unwrap_err()
            .kind(),
        io::ErrorKind::NotFound
    );
    let empty_first = std::env::join_paths([Path::new(""), bin.as_path()]).unwrap();
    std::fs::write("claude.exe", "").unwrap();
    assert_eq!(
        platform::resolve_in_path(name, &empty_first).unwrap(),
        bin.join("claude.cmd")
    );
    std::env::set_current_dir(original_dir).unwrap();
    let cli = platform::resolve(name).unwrap();
    assert_eq!(cli, bin.join("claude.cmd"));
    assert!(CliBackend::ClaudeCode.is_available().await);
    let user = "abcdefghij\n".repeat(3000);
    let compat = AtomicBool::new(false);
    for backend in [CliBackend::ClaudeCode, CliBackend::Codex] {
        let response = run_fixture(
            backend,
            &cli,
            &user,
            &work,
            Duration::from_secs(15),
            &compat,
        )
        .await
        .unwrap_or_else(|error| panic!("{backend}: {error}"));
        assert_eq!(response, "synthetic summary");
        wait_for_cleanup(&work).await;
    }

    std::fs::write(bin.join("sleep"), "").unwrap();
    let pid_file = bin.join("grandchild");
    for cancel in [false, true] {
        let mut call = Box::pin(run_fixture(
            CliBackend::ClaudeCode,
            &cli,
            &user,
            &work,
            Duration::from_secs(if cancel { 15 } else { 6 }),
            &compat,
        ));
        let pid = tokio::select! {
            result = &mut call => panic!("CLI exited without reporting descendant: {result:?}"),
            pid = wait_for_pid(&pid_file) => pid,
        };
        // SAFETY: OpenProcess takes a numeric PID and returns an owned handle on success.
        let handle = unsafe { OpenProcess(PROCESS_SYNCHRONIZE, 0, pid) };
        if handle.is_null() {
            assert_eq!(
                io::Error::last_os_error().raw_os_error(),
                Some(ERROR_INVALID_PARAMETER as i32)
            );
            panic!("descendant exited before cancellation");
        }
        // SAFETY: OpenProcess succeeded and returned an exclusively owned handle.
        let process = unsafe { OwnedHandle::from_raw_handle(handle) };
        if cancel {
            drop(call);
        } else {
            let error = call.await.unwrap_err();
            assert_eq!(crate::SummarizerError::from(error).kind(), "timeout");
        }
        // SAFETY: process keeps the handle alive.
        assert_eq!(unsafe { WaitForSingleObject(handle, 1000) }, WAIT_OBJECT_0);
        drop(process);
        wait_for_cleanup(&work).await;
        std::fs::remove_file(&pid_file).unwrap();
    }
}
// Real subprocess calls against the actually-installed, logged-in `claude`/
// `codex` CLIs — costs real tokens/time, so `#[ignore]`d. Run explicitly with
// `cargo test -p summarize -- --ignored`.
#[tokio::test]
#[ignore]
async fn summarize_via_cli_claude_code_smoke_test() {
    let turns = vec![TranscriptTurn {
        speaker: Some("Alice".to_string()),
        text: "The synthetic project is due Friday.".to_string(),
    }];
    let options = SummarizeOptions::new("sonnet");
    let result = summarize_via_cli(CliBackend::ClaudeCode, &turns, &options).await;
    let text = result.expect("claude CLI summarize should succeed");
    assert!(!text.trim().is_empty());
}

#[tokio::test]
#[ignore]
async fn summarize_via_cli_codex_smoke_test() {
    let turns = vec![TranscriptTurn {
        speaker: Some("Alice".to_string()),
        text: "The synthetic project is due Friday.".to_string(),
    }];
    let options = SummarizeOptions::new("gpt-5.5");
    let result = summarize_via_cli(CliBackend::Codex, &turns, &options).await;
    let text = result.expect("codex CLI summarize should succeed");
    assert!(!text.trim().is_empty());
}

#[cfg(unix)]
#[test]
fn configured_cli_base_is_used_and_cleaned() {
    let fixture = tempfile::tempdir().unwrap();
    let work = tempfile::tempdir().unwrap();
    let cli = script(
        fixture.path(),
        r#"
        test "$(pwd -P)" != "$MEETINGRAM_TEST_CLI_WORK" || exit 1
        test "$(cd ../.. && pwd -P)" = "$MEETINGRAM_TEST_CLI_WORK" || exit 2
        test -f system.txt || exit 3
        printf '%s\n' '{"result":"synthetic response"}'
        "#,
    );
    std::fs::rename(cli, fixture.path().join("claude")).unwrap();
    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "cli_backend::tests::configured_cli_base_subprocess",
            "--ignored",
        ])
        .env("PATH", fixture.path())
        .env(
            "MEETINGRAM_TEST_CLI_WORK",
            work.path().canonicalize().unwrap(),
        )
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[cfg(unix)]
#[tokio::test]
#[ignore = "launched by configured_cli_base_is_used_and_cleaned with an isolated PATH"]
async fn configured_cli_base_subprocess() {
    let Some(work) = std::env::var_os("MEETINGRAM_TEST_CLI_WORK").map(PathBuf::from) else {
        return;
    };
    let credentials = tempfile::tempdir().unwrap();
    let store = Arc::new(credential_store::EncryptedFileStore::new(credentials.path()).unwrap());
    let cli = crate::build_summarizer("claude-cli", store, None, Some(work.clone())).unwrap();
    let options = SummarizeOptions::new("sonnet");
    let request = CompletionRequest::new(
        "Synthetic instruction",
        "Synthetic message",
        "sonnet",
        Duration::from_secs(5),
    );
    let summary = cli.summarize(&[], &options).await;
    wait_for_cleanup(&work).await;
    let completion = cli.complete(&request).await;
    wait_for_cleanup(&work).await;

    assert_eq!(summary.unwrap(), "synthetic response");
    assert_eq!(completion.unwrap(), "synthetic response");
}

#[tokio::test]
#[ignore]
async fn complete_via_claude_code_smoke_test() {
    use crate::{CliSummarizer, Summarizer};

    let request = CompletionRequest::new(
        "Reply with the supplied line.",
        "The synthetic project is due Friday.",
        "sonnet",
        Duration::from_secs(60),
    );
    let text = CliSummarizer::new(CliBackend::ClaudeCode, None)
        .complete(&request)
        .await
        .expect("claude CLI completion should succeed");
    assert!(!text.trim().is_empty());
}

#[cfg(unix)]
#[tokio::test]
async fn codex_allows_code_mode_error_but_requires_a_response() {
    let base = tempfile::tempdir().unwrap();
    for has_response in [true, false] {
        let mut fixture = String::from(
            r#"printf '%s\n' '{"type":"item.completed","item":{"type":"error","message":"Code Mode is unavailable because code-mode host is disabled."}}'
"#,
        );
        if has_response {
            fixture.push_str(
                r#"printf '%s\n' '{"type":"item.completed","item":{"type":"agent_message","text":"synthetic summary"}}'
printf 'synthetic summary' >out.txt
"#,
            );
        }
        let cli = script(base.path(), &fixture);
        let output = run_fixture(
            CliBackend::Codex,
            &cli,
            "synthetic transcript",
            base.path(),
            Duration::from_secs(5),
            &AtomicBool::new(false),
        )
        .await;
        if has_response {
            assert_eq!(output.unwrap(), "synthetic summary");
        } else {
            assert!(
                matches!(output, Err(CliSummarizeError::ReadOutput(_, error)) if error.kind() == io::ErrorKind::NotFound)
            );
        }
    }
}

#[cfg(unix)]
#[tokio::test]
async fn claude_retries_only_after_failure_with_a_line_prefix() {
    let base = tempfile::tempdir().unwrap();
    for (diagnostic_prefix, status) in [("", 0), ("embedded ", 1)] {
        let cli = script(
            base.path(),
            &format!(
                r#"
            test "$5" = --tools || exit 99
            printf "%serror: unknown option '--tools'\n" '{diagnostic_prefix}' >&2
            printf '{{"result":"synthetic summary"}}\n'
            exit {status}
            "#,
            ),
        );
        let compat = AtomicBool::new(false);
        let output = run_fixture(
            CliBackend::ClaudeCode,
            &cli,
            "synthetic transcript",
            base.path(),
            Duration::from_secs(5),
            &compat,
        )
        .await;
        assert!(!compat.load(Ordering::Relaxed));
        if status == 0 {
            assert_eq!(output.unwrap(), "synthetic summary");
        } else {
            assert!(
                matches!(output, Err(CliSummarizeError::NonZeroExit(_, status)) if status.code() == Some(1))
            );
        }
    }
}

#[cfg(unix)]
#[tokio::test]
async fn unix_stops_descendants_when_the_leader_exits() {
    let base = tempfile::tempdir().unwrap();
    let work = tempfile::tempdir().unwrap();
    let cli = script(
        base.path(),
        r#"
        sleep 30 &
        printf '{"result":"synthetic summary"}\n'
        "#,
    );
    let output = run_fixture(
        CliBackend::ClaudeCode,
        &cli,
        "synthetic transcript",
        work.path(),
        Duration::from_secs(2),
        &AtomicBool::new(false),
    )
    .await
    .unwrap();
    assert_eq!(output, "synthetic summary");
    assert!(working_dirs_are_gone(work.path()));
}

#[cfg(windows)]
#[tokio::test]
async fn windows_cleanup_runs_off_the_async_worker() {
    let worker = std::thread::current().id();
    spawn_cleanup(move || assert_ne!(std::thread::current().id(), worker))
        .unwrap()
        .await
        .unwrap();
}

#[cfg(unix)]
#[tokio::test]
async fn completion_delivers_prompts_without_putting_them_in_arguments() {
    let base = tempfile::tempdir().unwrap();
    let cli = script(
        base.path(),
        r#"
        test "$#" -eq 9 || exit 2
        test "$8" = '--system-prompt-file' || exit 3
        test "$(cat "$9")" = 'synthetic system' || exit 4
        test "$(cat)" = 'synthetic user' || exit 5
        for arg do
            case "$arg" in *synthetic*) exit 6 ;; esac
        done
        printf '{"result":"synthetic response"}\n'
    "#,
    );
    let request = CompletionRequest::new(
        "synthetic system",
        "synthetic user",
        "",
        Duration::from_secs(5),
    );
    assert_eq!(
        complete_with_binary(CliBackend::ClaudeCode, cli.as_os_str(), &request, None)
            .await
            .unwrap(),
        "synthetic response",
    );

    let cli = script(base.path(), "exec sleep 30");
    let request = CompletionRequest::new("", "", "", Duration::from_millis(50));
    assert_eq!(
        complete_with_binary(CliBackend::ClaudeCode, cli.as_os_str(), &request, None)
            .await
            .unwrap_err()
            .kind(),
        "timeout",
    );
}

#[cfg(unix)]
#[tokio::test(start_paused = true)]
async fn summarize_enforces_its_default_and_explicit_deadlines() {
    use std::future::poll_fn;
    use std::task::Poll;

    let base = tempfile::tempdir().unwrap();
    let cli = script(base.path(), "exec sleep 30");
    for (options, seconds) in [
        (SummarizeOptions::new(""), 600),
        (
            SummarizeOptions::new("").with_cli_timeout(Duration::from_secs(3)),
            3,
        ),
    ] {
        let mut response = std::pin::pin!(summarize_with_binary(
            CliBackend::ClaudeCode,
            cli.as_os_str(),
            &[],
            &options,
            None,
        ));
        assert!(poll_fn(|cx| Poll::Ready(response.as_mut().poll(cx)))
            .await
            .is_pending());
        tokio::time::advance(Duration::from_secs(seconds - 1)).await;
        assert!(poll_fn(|cx| Poll::Ready(response.as_mut().poll(cx)))
            .await
            .is_pending());
        tokio::time::advance(Duration::from_secs(1) + Duration::from_millis(1)).await;
        assert!(matches!(
            poll_fn(|cx| Poll::Ready(response.as_mut().poll(cx))).await,
            Poll::Ready(Err(CliSummarizeError::Timeout(CliBackend::ClaudeCode)))
        ));
    }
}
