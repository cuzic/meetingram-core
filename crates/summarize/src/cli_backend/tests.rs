use super::*;
use std::future::Future;

#[test]
fn cleanup_removes_only_cli_directories() {
    let base = tempfile::tempdir().unwrap();
    let stale = base.path().join("meetingram-cli-abandoned");
    let unrelated = base.path().join("other");
    std::fs::create_dir(&stale).unwrap();
    std::fs::create_dir(&unrelated).unwrap();
    std::fs::write(stale.join("system.txt"), "synthetic instruction").unwrap();
    cleanup_stale_cli_dirs(base.path()).unwrap();
    assert!(!stale.exists());
    assert!(unrelated.exists());
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
    )
}

#[cfg(unix)]
fn script(dir: &Path, contents: &str) -> PathBuf {
    use std::os::unix::fs::PermissionsExt;
    let script = dir.join("sample-cli");
    std::fs::write(&script, format!("#!/bin/sh\n{contents}")).unwrap();
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o700)).unwrap();
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
                printf "error: unknown option '--tools'\n" >&2
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
    assert_eq!(work.path().read_dir().unwrap().count(), 0);
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
    assert_eq!(work.path().read_dir().unwrap().count(), 0);
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
    assert_eq!(work.path().read_dir().unwrap().count(), 0);
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
        assert_eq!(work.path().read_dir().unwrap().count(), 0);
        std::fs::remove_file(&pid_file).unwrap();
    }
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
        assert_eq!(work.read_dir().unwrap().count(), 0);
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
        assert_eq!(work.read_dir().unwrap().count(), 0, "cancel = {cancel}");
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
