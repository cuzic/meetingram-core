//! Runs the authenticated Claude and Codex CLIs in per-call working directories.
//! The system prompt lives in a file and the transcript travels over stdin.
//! Claude returns JSON and can fall back to its legacy flags. Codex streams JSON
//! events for tool-use detection and writes the final answer to a separate file.
//! Each invocation owns a process tree that is stopped before directory cleanup.

use std::ffi::{OsStr, OsString};
use std::io;
use std::path::{Path, PathBuf};
use std::process::ExitStatus;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncReadExt, AsyncWriteExt, BufReader};

use crate::{render_transcript, SummarizeOptions, TranscriptTurn, DEFAULT_SYSTEM_PROMPT};

#[cfg(unix)]
#[path = "cli_backend/unix.rs"]
mod platform;
#[cfg(windows)]
#[path = "cli_backend/windows.rs"]
mod platform;

const DIR_PREFIX: &str = "meetingram-cli-";
const SYSTEM_FILE: &str = "system.txt";
const OUTPUT_FILE: &str = "out.txt";
const CLAUDE_DISALLOWED_TOOLS: &str =
    "Bash,Edit,Write,Read,WebFetch,WebSearch,Task,Glob,Grep,NotebookEdit";
// Verified with Claude Code 2.1.330.
const UNKNOWN_OPTION_PREFIXES: &[&[u8]] = &[b"error: unknown option '"];
const CODEX_DISABLED_FEATURES: &[&str] = &[
    "shell_tool",
    "view_image",
    "apps",
    "multi_agent",
    "code_mode_host",
    "image_generation",
    "plugins",
    "goals",
    "browser_use",
    "computer_use",
    "unified_exec",
];
static CLAUDE_COMPAT: AtomicBool = AtomicBool::new(false);

/// Default CLI time limit for incremental graph updates.
pub const DEFAULT_CLI_TIMEOUT: Duration = Duration::from_secs(60);
pub(crate) const DEFAULT_SUMMARY_TIMEOUT: Duration = Duration::from_secs(600);

/// Whether Claude has switched to legacy flags in this process.
pub fn claude_compatibility_mode() -> bool {
    CLAUDE_COMPAT.load(Ordering::Relaxed)
}

/// Removes leftover CLI working directories from a dedicated base directory.
/// Call at startup, before starting CLI requests using this base.
pub fn cleanup_stale_cli_dirs(base: &Path) -> io::Result<()> {
    let entries = match std::fs::read_dir(base) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    };
    for entry in entries {
        let entry = entry?;
        if entry.file_type()?.is_dir()
            && entry.file_name().to_string_lossy().starts_with(DIR_PREFIX)
        {
            std::fs::remove_dir_all(entry.path())?;
        }
    }
    Ok(())
}

/// Which CLI-based summarization backend to use.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CliBackend {
    ClaudeCode,
    Codex,
}

impl CliBackend {
    /// The executable name looked up on `PATH`.
    pub fn binary(self) -> &'static str {
        match self {
            Self::ClaudeCode => "claude",
            Self::Codex => "codex",
        }
    }

    /// Checks whether the CLI on `PATH` runs successfully with `--version`.
    pub async fn is_available(self) -> bool {
        available(OsStr::new(self.binary())).await
    }
}

async fn available(name: &OsStr) -> bool {
    let Ok(binary) = platform::resolve(name) else {
        return false;
    };
    let Ok(mut process) = platform::Process::spawn(&binary, &["--version".into()], None) else {
        return false;
    };
    drop(process.stdin.take());
    tokio::time::timeout(DEFAULT_CLI_TIMEOUT, async {
        tokio::try_join!(
            process.group.wait(),
            async { tokio::io::copy(&mut process.stdout, &mut tokio::io::sink()).await },
            async { tokio::io::copy(&mut process.stderr, &mut tokio::io::sink()).await },
        )
    })
    .await
    .is_ok_and(|result| result.is_ok_and(|(status, _, _)| status.success()))
}

impl std::fmt::Display for CliBackend {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.binary())
    }
}

#[derive(Debug, thiserror::Error)]
pub enum CliSummarizeError {
    #[error("{0} CLI not found on PATH")]
    NotFound(CliBackend),
    #[error("failed to prepare a temporary working directory: {0}")]
    TempDir(#[source] io::Error),
    #[error("failed to launch {0} CLI: {1}")]
    Spawn(CliBackend, #[source] io::Error),
    #[error("{0} CLI exited with {1}")]
    NonZeroExit(CliBackend, ExitStatus),
    #[error("failed to communicate with {0} CLI: {1}")]
    ReadOutput(CliBackend, #[source] io::Error),
    #[error("failed to parse {0} CLI output: {1}")]
    ParseOutput(CliBackend, String),
    #[error("{0} CLI returned an empty summary")]
    EmptyResponse(CliBackend),
    #[error("{0} CLI attempted to use a tool")]
    ToolUse(CliBackend),
    #[error("{0} CLI timed out")]
    Timeout(CliBackend),
}

struct CliInput<'a> {
    system: &'a str,
    user: &'a str,
    model: Option<&'a str>,
}

/// Summarizes a transcript using the CLI with the default timeout and temp base.
pub async fn summarize_via_cli(
    backend: CliBackend,
    turns: &[TranscriptTurn],
    options: &SummarizeOptions,
) -> Result<String, CliSummarizeError> {
    summarize_via_cli_with_limits(
        backend,
        turns,
        options,
        None,
        options.cli_timeout.unwrap_or(DEFAULT_SUMMARY_TIMEOUT),
    )
    .await
}

/// Summarizes a transcript with a caller-selected temp base and time limit.
/// An explicit base directory must already exist.
pub async fn summarize_via_cli_with_limits(
    backend: CliBackend,
    turns: &[TranscriptTurn],
    options: &SummarizeOptions,
    base: Option<&Path>,
    timeout: Duration,
) -> Result<String, CliSummarizeError> {
    let binary = platform::resolve(OsStr::new(backend.binary()))
        .map_err(|error| launch_error(backend, error))?;
    let input = CliInput {
        system: options
            .system_prompt
            .as_deref()
            .unwrap_or(DEFAULT_SYSTEM_PROMPT),
        user: &render_transcript(turns),
        model: (!options.model.trim().is_empty()).then_some(options.model.as_str()),
    };
    run_cli(backend, &binary, input, base, timeout, &CLAUDE_COMPAT).await
}

fn launch_error(backend: CliBackend, error: io::Error) -> CliSummarizeError {
    if error.kind() == io::ErrorKind::NotFound {
        CliSummarizeError::NotFound(backend)
    } else {
        CliSummarizeError::Spawn(backend, error)
    }
}

struct WorkingDir(Option<tempfile::TempDir>);

impl WorkingDir {
    fn path(&self) -> &Path {
        self.0.as_ref().expect("working directory is live").path()
    }
}

#[cfg(windows)]
fn remove_working_dir(dir: tempfile::TempDir) {
    // A terminated process can briefly retain its working directory.
    for _ in 0..20 {
        match std::fs::remove_dir_all(dir.path()) {
            Ok(()) => return,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return,
            Err(error)
                if matches!(
                    error.kind(),
                    io::ErrorKind::PermissionDenied | io::ErrorKind::DirectoryNotEmpty
                ) || error.raw_os_error() == Some(32) =>
            {
                std::thread::sleep(Duration::from_millis(5));
            }
            Err(_) => return,
        }
    }
}

impl Drop for WorkingDir {
    fn drop(&mut self) {
        #[cfg(windows)]
        if let Some(dir) = self.0.take() {
            spawn_cleanup(move || remove_working_dir(dir));
        }
    }
}

fn spawn_cleanup(cleanup: impl FnOnce() + Send + 'static) -> Option<tokio::task::JoinHandle<()>> {
    if let Ok(runtime) = tokio::runtime::Handle::try_current() {
        Some(runtime.spawn_blocking(cleanup))
    } else {
        std::thread::spawn(cleanup);
        None
    }
}

async fn run_cli(
    backend: CliBackend,
    binary: &Path,
    input: CliInput<'_>,
    base: Option<&Path>,
    timeout: Duration,
    compatibility: &AtomicBool,
) -> Result<String, CliSummarizeError> {
    tokio::time::timeout(timeout, async {
        let base = match base {
            Some(base) => std::path::absolute(base),
            None => std::path::absolute(std::env::temp_dir()),
        }
        .map_err(CliSummarizeError::TempDir)?;
        let system = input.system.to_owned();
        // The worker owns the directory throughout setup. Dropping the
        // request during a write still cleans it up after the write finishes.
        let dir = tokio::task::spawn_blocking(move || {
            let dir = tempfile::Builder::new()
                .prefix(DIR_PREFIX)
                .tempdir_in(base)?;
            std::fs::write(dir.path().join(SYSTEM_FILE), system)?;
            Ok::<_, io::Error>(Arc::new(WorkingDir(Some(dir))))
        })
        .await
        .map_err(|error| CliSummarizeError::TempDir(io::Error::other(error)))?
        .map_err(CliSummarizeError::TempDir)?;
        let compat = backend == CliBackend::ClaudeCode && compatibility.load(Ordering::Relaxed);
        let mut result = run_attempt(backend, binary, &input, &dir, compat).await?;
        if backend == CliBackend::ClaudeCode
            && !compat
            && !result.status.success()
            && result.unknown_option
        {
            compatibility.store(true, Ordering::Relaxed);
            result = run_attempt(backend, binary, &input, &dir, true).await?;
        }
        if !result.status.success() {
            return Err(CliSummarizeError::NonZeroExit(backend, result.status));
        }

        let text = match result.stdout {
            CliStdout::Claude(stdout) => {
                let value: serde_json::Value = serde_json::from_slice(&stdout)
                    .map_err(|e| CliSummarizeError::ParseOutput(backend, e.to_string()))?;
                value
                    .get("result")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| {
                        CliSummarizeError::ParseOutput(backend, "missing result field".into())
                    })?
                    .to_owned()
            }
            CliStdout::MalformedCodexEvent => {
                return Err(CliSummarizeError::ParseOutput(
                    backend,
                    "malformed JSON event".into(),
                ));
            }
            CliStdout::Codex => tokio::fs::read_to_string(dir.path().join(OUTPUT_FILE))
                .await
                .map_err(|e| CliSummarizeError::ReadOutput(backend, e))?,
        };
        if text.trim().is_empty() {
            return Err(CliSummarizeError::EmptyResponse(backend));
        }
        Ok(text)
    })
    .await
    .map_err(|_| CliSummarizeError::Timeout(backend))?
}

fn cli_args(backend: CliBackend, dir: &Path, model: Option<&str>, compat: bool) -> Vec<OsString> {
    let mut args = Vec::new();
    match backend {
        CliBackend::ClaudeCode => {
            args.extend(
                [
                    "--print",
                    "--output-format",
                    "json",
                    "--no-session-persistence",
                ]
                .map(OsString::from),
            );
            if compat {
                args.extend(["--disallowedTools", CLAUDE_DISALLOWED_TOOLS].map(OsString::from));
            } else {
                args.extend(
                    [
                        "--tools",
                        "",
                        "--strict-mcp-config",
                        "--system-prompt-file",
                        SYSTEM_FILE,
                    ]
                    .map(OsString::from),
                );
            }
        }
        CliBackend::Codex => {
            args.extend(
                [
                    "exec",
                    "-",
                    "--json",
                    "--ephemeral",
                    "--skip-git-repo-check",
                    "-s",
                    "read-only",
                    "-C",
                ]
                .map(OsString::from),
            );
            args.extend([
                dir.as_os_str().to_owned(),
                "-o".into(),
                dir.join(OUTPUT_FILE).into_os_string(),
            ]);
            for setting in [
                format!("model_instructions_file=\"{SYSTEM_FILE}\""),
                "project_doc_max_bytes=0".into(),
                "mcp_servers={}".into(),
                "web_search=\"disabled\"".into(),
            ]
            .into_iter()
            .chain(
                CODEX_DISABLED_FEATURES
                    .iter()
                    .map(|name| format!("features.{name}=false")),
            ) {
                args.extend(["-c".into(), setting.into()]);
            }
        }
    }
    if let Some(model) = model {
        let flag = match backend {
            CliBackend::ClaudeCode => "--model",
            CliBackend::Codex => "-m",
        };
        args.extend([flag.into(), model.into()]);
    }
    args
}

enum CliStdout {
    Claude(Vec<u8>),
    Codex,
    MalformedCodexEvent,
}

struct AttemptOutput {
    status: ExitStatus,
    stdout: CliStdout,
    unknown_option: bool,
}

async fn run_attempt(
    backend: CliBackend,
    binary: &Path,
    input: &CliInput<'_>,
    dir: &Arc<WorkingDir>,
    compat: bool,
) -> Result<AttemptOutput, CliSummarizeError> {
    let args = cli_args(backend, dir.path(), input.model, compat);
    let mut process =
        platform::Process::spawn(binary, &args, Some(dir)).map_err(|e| launch_error(backend, e))?;
    let mut stdin = process.stdin.take().expect("CLI stdin is piped");
    let read_error = |error| CliSummarizeError::ReadOutput(backend, error);
    let result = tokio::try_join!(
        async { process.group.wait().await.map_err(read_error) },
        async move {
            let result = async {
                if compat {
                    stdin.write_all(input.system.as_bytes()).await?;
                    stdin.write_all(b"\n\n").await?;
                }
                stdin.write_all(input.user.as_bytes()).await?;
                stdin.flush().await
            }
            .await;
            Ok(result)
        },
        read_stdout(backend, &mut process.stdout),
        async {
            read_stderr(
                &mut process.stderr,
                backend == CliBackend::ClaudeCode && !compat,
            )
            .await
            .map_err(read_error)
        },
    );
    if let Some(cleanup) = process.group.stop() {
        let _ = cleanup.await;
    }
    let (status, write_result, stdout, unknown_option) = result?;
    if let Err(error) = write_result {
        if status.success() && error.kind() != io::ErrorKind::BrokenPipe {
            return Err(read_error(error));
        }
    }
    Ok(AttemptOutput {
        status,
        stdout,
        unknown_option,
    })
}

async fn read_stdout(
    backend: CliBackend,
    mut stdout: impl AsyncRead + Unpin,
) -> Result<CliStdout, CliSummarizeError> {
    let read_error = |error| CliSummarizeError::ReadOutput(backend, error);
    if backend == CliBackend::ClaudeCode {
        let mut response = Vec::new();
        stdout
            .read_to_end(&mut response)
            .await
            .map_err(read_error)?;
        return Ok(CliStdout::Claude(response));
    }

    let mut reader = BufReader::new(stdout);
    let mut line = Vec::new();
    let mut malformed = false;
    while reader
        .read_until(b'\n', &mut line)
        .await
        .map_err(read_error)?
        != 0
    {
        match serde_json::from_slice::<serde_json::Value>(&line) {
            Ok(event)
                if event
                    .get("type")
                    .and_then(|v| v.as_str())
                    .is_some_and(|t| t.starts_with("item.")) =>
            {
                if !matches!(
                    event.pointer("/item/type").and_then(|v| v.as_str()),
                    Some("agent_message" | "reasoning" | "error")
                ) {
                    return Err(CliSummarizeError::ToolUse(backend));
                }
            }
            Ok(_) => {}
            Err(_) if line.iter().all(u8::is_ascii_whitespace) => {}
            Err(_) => malformed = true,
        }
        line.clear();
    }
    if malformed {
        Ok(CliStdout::MalformedCodexEvent)
    } else {
        Ok(CliStdout::Codex)
    }
}

async fn read_stderr(mut stderr: impl AsyncRead + Unpin, detect_unknown: bool) -> io::Result<bool> {
    // Track prefix matches only. Even arbitrarily long stderr lines are drained
    // without storing their contents or including them in an error.
    let mut matched = false;
    let mut candidates = vec![true; UNKNOWN_OPTION_PREFIXES.len()];
    let mut offset = 0;
    let mut buffer = [0; 4096];
    loop {
        let read = stderr.read(&mut buffer).await?;
        if read == 0 {
            return Ok(matched);
        }
        if detect_unknown && !matched {
            for &byte in &buffer[..read] {
                if byte == b'\n' {
                    candidates.fill(true);
                    offset = 0;
                    continue;
                }
                for (&prefix, candidate) in UNKNOWN_OPTION_PREFIXES.iter().zip(&mut candidates) {
                    *candidate &= prefix.get(offset) == Some(&byte);
                    matched |= *candidate && offset + 1 == prefix.len();
                }
                if matched {
                    break;
                }
                offset = offset.saturating_add(1);
            }
        }
    }
}

#[cfg(test)]
#[path = "cli_backend/tests.rs"]
mod tests;
