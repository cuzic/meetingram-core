//! Runs the authenticated Claude and Codex CLIs in per-call working directories.
//! The system prompt lives in a file and the transcript travels over stdin.
//! Claude returns JSON and can fall back to its legacy flags. Codex streams JSON
//! events for tool-use detection and writes the final answer to a separate file.
//! Each invocation owns a process tree that is stopped before directory cleanup.

use std::ffi::{OsStr, OsString};
use std::io;
use std::path::{Path, PathBuf};
use std::process::{ExitStatus, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use base64::Engine;
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncReadExt, AsyncWriteExt, BufReader};

use crate::{
    render_transcript, CompletionRequest, SummarizeOptions, SummarizerError, TranscriptTurn,
    DEFAULT_SYSTEM_PROMPT,
};

#[cfg(unix)]
#[path = "cli_backend/unix.rs"]
mod platform;
#[cfg(windows)]
#[path = "cli_backend/windows.rs"]
mod platform;

const DIR_PREFIX: &str = "meetingram-cli-";
const STALE_DIR_AGE: Duration = Duration::from_secs(60 * 60);
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
const DEFAULT_SUMMARY_TIMEOUT: Duration = Duration::from_secs(600);

/// Whether Claude has switched to legacy flags in this process.
pub fn claude_compatibility_mode() -> bool {
    CLAUDE_COMPAT.load(Ordering::Relaxed)
}

enum ProcessStart {
    Known(u64),
    Alive,
    Dead,
    Unknown,
}

fn process_dir_name() -> &'static str {
    static NAME: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    NAME.get_or_init(|| {
        let pid = std::process::id();
        match platform::process_start(pid) {
            ProcessStart::Known(start) => format!("{pid}-{start}"),
            _ => {
                let now = SystemTime::now()
                    .duration_since(SystemTime::UNIX_EPOCH)
                    .unwrap_or_default();
                format!("{pid}-t{}", now.as_nanos())
            }
        }
    })
}

/// Removes leftover CLI working directories from a dedicated application base.
/// Keeps this process's directory and those of other live processes. Legacy
/// directories and directories whose owner cannot be checked are eligible for
/// removal after one hour without modification.
pub fn cleanup_stale_cli_dirs(base: &Path) -> io::Result<()> {
    let entries = match std::fs::read_dir(base) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    };
    for entry in entries {
        let entry = entry?;
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        if name == process_dir_name() {
            continue;
        }
        let result = (|| {
            if entry.file_type()?.is_dir() && is_stale_cli_dir(&entry, name)? {
                std::fs::remove_dir_all(entry.path())?;
            }
            Ok::<_, io::Error>(())
        })();
        // Another instance can be cleaning up the same stale directory.
        match result {
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            other => other?,
        }
    }
    Ok(())
}

fn is_stale_cli_dir(entry: &std::fs::DirEntry, name: &str) -> io::Result<bool> {
    if !name.starts_with(DIR_PREFIX) {
        let Some((pid, start)) = name.split_once('-') else {
            return Ok(false);
        };
        let Ok(pid) = pid.parse::<u32>() else {
            return Ok(false);
        };
        let start = match start.parse::<u64>() {
            Ok(start) => Some(start),
            Err(_)
                if start
                    .strip_prefix('t')
                    .is_some_and(|s| s.parse::<u128>().is_ok()) =>
            {
                None
            }
            Err(_) => return Ok(false),
        };
        if pid == 0 {
            return Ok(false);
        }
        match platform::process_start(pid) {
            ProcessStart::Dead => return Ok(true),
            ProcessStart::Known(current) => return Ok(start.is_some_and(|start| start != current)),
            ProcessStart::Alive => return Ok(false),
            ProcessStart::Unknown => {}
        }
    }

    let modified = entry.metadata()?.modified()?;
    Ok(modified.elapsed().is_ok_and(|age| age > STALE_DIR_AGE))
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
    summarize_via_cli_with_base(backend, turns, options, None).await
}

pub(crate) async fn summarize_via_cli_with_base(
    backend: CliBackend,
    turns: &[TranscriptTurn],
    options: &SummarizeOptions,
    base: Option<&Path>,
) -> Result<String, CliSummarizeError> {
    summarize_with_binary(backend, OsStr::new(backend.binary()), turns, options, base).await
}

async fn summarize_with_binary(
    backend: CliBackend,
    name: &OsStr,
    turns: &[TranscriptTurn],
    options: &SummarizeOptions,
    base: Option<&Path>,
) -> Result<String, CliSummarizeError> {
    summarize_with_limits_and_binary(
        backend,
        name,
        turns,
        options,
        base,
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
    summarize_with_limits_and_binary(
        backend,
        OsStr::new(backend.binary()),
        turns,
        options,
        base,
        timeout,
    )
    .await
}

async fn summarize_with_limits_and_binary(
    backend: CliBackend,
    name: &OsStr,
    turns: &[TranscriptTurn],
    options: &SummarizeOptions,
    base: Option<&Path>,
    timeout: Duration,
) -> Result<String, CliSummarizeError> {
    let input = CliInput {
        system: options
            .system_prompt
            .as_deref()
            .unwrap_or(DEFAULT_SYSTEM_PROMPT),
        user: &render_transcript(turns),
        model: (!options.model.trim().is_empty()).then_some(options.model.as_str()),
    };
    run_named_cli(backend, name, input, base, timeout).await
}

pub(crate) async fn complete_via_cli(
    backend: CliBackend,
    req: &CompletionRequest,
    base: Option<&Path>,
) -> Result<String, SummarizerError> {
    complete_with_binary(backend, OsStr::new(backend.binary()), req, base).await
}

async fn complete_with_binary(
    backend: CliBackend,
    name: &OsStr,
    req: &CompletionRequest,
    base: Option<&Path>,
) -> Result<String, SummarizerError> {
    req.validate()?;
    let input = CliInput {
        system: &req.system,
        user: &req.user,
        model: (!req.model.trim().is_empty()).then_some(req.model.as_str()),
    };
    Ok(run_named_cli(backend, name, input, base, req.timeout).await?)
}

async fn run_named_cli(
    backend: CliBackend,
    name: &OsStr,
    input: CliInput<'_>,
    base: Option<&Path>,
    timeout: Duration,
) -> Result<String, CliSummarizeError> {
    let binary = platform::resolve(name).map_err(|error| launch_error(backend, error))?;
    run_cli(backend, &binary, input, base, timeout, &CLAUDE_COMPAT, None).await
}

/// Verifies the current account, tool restrictions and subscription-only
/// billing for the dedicated Herdr connection. Receives no meeting text.
#[async_trait::async_trait]
pub trait HerdrPreflight: Send + Sync {
    async fn verify(&self, session: &str, core_id: &str, pid: u32) -> bool;
}

struct HerdrCommand {
    binary: PathBuf,
    session: String,
}

impl HerdrCommand {
    fn command(&self) -> tokio::process::Command {
        let mut command = tokio::process::Command::new(&self.binary);
        command
            .env("HC_SESSION", &self.session)
            .env_remove("HC_SOCKET")
            .env_remove("HC_TERMINAL_ID")
            .env_remove("SSH_CONNECTION")
            .env_remove("SSH_CLIENT")
            .env_remove("SSH_TTY")
            .kill_on_drop(true);
        command
    }
}

struct HerdrRpc {
    _client: tokio::process::Child,
    stdin: tokio::process::ChildStdin,
    stdout: BufReader<tokio::process::ChildStdout>,
    next_id: u64,
}

#[derive(serde::Deserialize)]
struct HerdrExecResult {
    code: Option<i32>,
    timed_out: bool,
    truncated: bool,
    stdout: String,
    stderr: String,
}

impl HerdrRpc {
    fn connect(herdr: &HerdrCommand) -> io::Result<Self> {
        let mut client = herdr
            .command()
            .arg("connect")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()?;
        Ok(Self {
            stdin: client.stdin.take().expect("Herdr stdin is piped"),
            stdout: BufReader::new(client.stdout.take().expect("Herdr stdout is piped")),
            _client: client,
            next_id: 0,
        })
    }

    async fn exec(
        &mut self,
        binary: &Path,
        args: &[OsString],
        dir: &Path,
        input: Option<&str>,
        timeout: Duration,
    ) -> io::Result<HerdrExecResult> {
        let wire_error =
            || io::Error::new(io::ErrorKind::InvalidData, "invalid Herdr exec response");
        let argv = std::iter::once(binary.as_os_str())
            .chain(args.iter().map(OsString::as_os_str))
            .map(|value| value.to_str().ok_or_else(wire_error))
            .collect::<io::Result<Vec<_>>>()?;
        let cwd = dir.to_str().ok_or_else(wire_error)?;
        self.call(
            "machine.exec",
            serde_json::json!({
                "argv": argv,
                "cwd": cwd,
                "input": input.map(|body| base64::engine::general_purpose::STANDARD.encode(body)),
                "timeout": timeout.as_secs_f64().clamp(1.0, 600.0)
            }),
        )
        .await
    }

    async fn identity(&mut self) -> io::Result<(String, u32)> {
        #[derive(serde::Deserialize)]
        struct Core {
            id: String,
        }
        #[derive(serde::Deserialize)]
        struct Status {
            core: Core,
            pid: u32,
        }
        let status: Status = self.call("server.status", serde_json::json!({})).await?;
        if status.core.id.is_empty() || status.pid == 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "invalid Herdr identity",
            ));
        }
        Ok((status.core.id, status.pid))
    }

    async fn call<T: serde::de::DeserializeOwned>(
        &mut self,
        method: &str,
        params: serde_json::Value,
    ) -> io::Result<T> {
        let wire_error =
            || io::Error::new(io::ErrorKind::InvalidData, "invalid Herdr RPC response");
        self.next_id += 1;
        let mut request = serde_json::to_vec(&serde_json::json!({
            "jsonrpc": "2.0",
            "id": self.next_id,
            "method": method,
            "params": params,
        }))
        .map_err(|_| wire_error())?;
        request.push(b'\n');
        self.stdin.write_all(&request).await?;
        self.stdin.flush().await?;
        const MAX_REPLY_BYTES: u64 = 16 * 1024 * 1024;
        let mut reply = Vec::new();
        (&mut self.stdout)
            .take(MAX_REPLY_BYTES)
            .read_until(b'\n', &mut reply)
            .await?;
        if reply.last() != Some(&b'\n') {
            return Err(wire_error());
        }
        let mut response: serde_json::Value =
            serde_json::from_slice(&reply).map_err(|_| wire_error())?;
        if response["id"].as_u64() != Some(self.next_id) || response.get("error").is_some() {
            return Err(wire_error());
        }
        serde_json::from_value(response["result"].take()).map_err(|_| wire_error())
    }
}

/// A dedicated Herdr connection shared by consumers of the same provider.
/// Failed or cancelled calls stop the connection until it is explicitly rebuilt.
pub struct HerdrSummarizer {
    backend: CliBackend,
    herdr: HerdrCommand,
    provider_binary: PathBuf,
    temp_base: PathBuf,
    preflight: Arc<dyn HerdrPreflight>,
    stopped: tokio::sync::Mutex<bool>,
}

impl HerdrSummarizer {
    pub fn new(
        backend: CliBackend,
        herdr_binary: PathBuf,
        session: String,
        provider_binary: PathBuf,
        temp_base: PathBuf,
        preflight: Arc<dyn HerdrPreflight>,
    ) -> Self {
        Self {
            backend,
            herdr: HerdrCommand {
                binary: herdr_binary,
                session,
            },
            provider_binary,
            temp_base,
            preflight,
            stopped: tokio::sync::Mutex::new(false),
        }
    }

    async fn has_subscription_login(&self, connection: &mut HerdrRpc) -> bool {
        let args: Vec<OsString> = match self.backend {
            CliBackend::ClaudeCode => ["auth", "status", "--json"].as_slice(),
            CliBackend::Codex => ["login", "status"].as_slice(),
        }
        .iter()
        .map(OsString::from)
        .collect();
        let query = connection.exec(
            &self.provider_binary,
            &args,
            &self.temp_base,
            None,
            Duration::from_secs(10),
        );
        let Ok(Ok(result)) = tokio::time::timeout(Duration::from_secs(12), query).await else {
            return false;
        };
        let encoding = base64::engine::general_purpose::STANDARD;
        if result.code != Some(0)
            || result.timed_out
            || result.truncated
            || result.stdout.len() > 16 * 1024
            || result.stderr.len() > 16 * 1024
        {
            return false;
        }
        match (
            encoding.decode(result.stdout),
            encoding.decode(result.stderr),
        ) {
            (Ok(stdout), Ok(stderr)) => subscription_status(self.backend, &stdout, &stderr),
            _ => false,
        }
    }

    async fn run(&self, input: CliInput<'_>, timeout: Duration) -> Result<String, SummarizerError> {
        tokio::time::timeout(timeout, async {
            let mut stopped = self.stopped.lock().await;
            if *stopped {
                return Err(SummarizerError::HerdrStopped);
            }
            // Cancellation after this point leaves the connection stopped.
            *stopped = true;
            if self.herdr.session.is_empty()
                || !self.herdr.binary.is_absolute()
                || !self.provider_binary.is_absolute()
            {
                return Err(SummarizerError::HerdrUnverified);
            }
            let mut connection = HerdrRpc::connect(&self.herdr)
                .map_err(|error| launch_error(self.backend, error))?;
            let (core_id, pid) = connection
                .identity()
                .await
                .map_err(|error| CliSummarizeError::ReadOutput(self.backend, error))?;
            if !self
                .preflight
                .verify(&self.herdr.session, &core_id, pid)
                .await
                || !self.has_subscription_login(&mut connection).await
            {
                return Err(SummarizerError::HerdrUnverified);
            }
            let result = run_cli(
                self.backend,
                &self.provider_binary,
                input,
                Some(&self.temp_base),
                timeout,
                &CLAUDE_COMPAT,
                Some(&mut connection),
            )
            .await?;
            *stopped = false;
            Ok(result)
        })
        .await
        .map_err(|_| SummarizerError::Timeout)?
    }
}

#[async_trait::async_trait]
impl crate::Summarizer for HerdrSummarizer {
    async fn summarize(
        &self,
        turns: &[TranscriptTurn],
        options: &SummarizeOptions,
    ) -> Result<String, SummarizerError> {
        self.run(
            CliInput {
                system: options
                    .system_prompt
                    .as_deref()
                    .unwrap_or(DEFAULT_SYSTEM_PROMPT),
                user: &render_transcript(turns),
                model: (!options.model.trim().is_empty()).then_some(options.model.as_str()),
            },
            options.cli_timeout.unwrap_or(DEFAULT_SUMMARY_TIMEOUT),
        )
        .await
    }

    async fn complete(&self, req: &CompletionRequest) -> Result<String, SummarizerError> {
        req.validate()?;
        self.run(
            CliInput {
                system: &req.system,
                user: &req.user,
                model: (!req.model.trim().is_empty()).then_some(req.model.as_str()),
            },
            req.timeout,
        )
        .await
    }
}

fn subscription_status(backend: CliBackend, stdout: &[u8], stderr: &[u8]) -> bool {
    match backend {
        CliBackend::ClaudeCode => {
            serde_json::from_slice::<serde_json::Value>(stdout).is_ok_and(|status| {
                status["loggedIn"].as_bool() == Some(true)
                    && status["authMethod"].as_str() == Some("claude.ai")
                    && status["apiProvider"].as_str() == Some("firstParty")
            })
        }
        CliBackend::Codex => [stdout, stderr].into_iter().any(|stream| {
            stream
                .split(|byte| *byte == b'\n')
                .any(|line| line.trim_ascii() == b"Logged in using ChatGPT")
        }),
    }
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
    herdr: Option<&mut HerdrRpc>,
) -> Result<String, CliSummarizeError> {
    tokio::time::timeout(timeout, async {
        let dedicated_base = base.is_some();
        let base = std::path::absolute(base.map_or_else(std::env::temp_dir, Path::to_path_buf))
            .map_err(CliSummarizeError::TempDir)?;
        let system = input.system.to_owned();
        // The worker owns the directory throughout setup. Dropping the
        // request during a write still cleans it up after the write finishes.
        let dir = tokio::task::spawn_blocking(move || {
            let base = if dedicated_base {
                let process_dir = base.join(process_dir_name());
                match std::fs::create_dir(&process_dir) {
                    Ok(()) => {}
                    Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
                        if !std::fs::symlink_metadata(&process_dir)?.is_dir() {
                            return Err(error);
                        }
                    }
                    Err(error) => return Err(error),
                }
                process_dir
            } else {
                base
            };
            let dir = tempfile::Builder::new()
                .prefix(DIR_PREFIX)
                .tempdir_in(base)?;
            std::fs::write(dir.path().join(SYSTEM_FILE), system)?;
            Ok::<_, io::Error>(Arc::new(WorkingDir(Some(dir))))
        })
        .await
        .map_err(|error| CliSummarizeError::TempDir(io::Error::other(error)))?
        .map_err(CliSummarizeError::TempDir)?;
        let require_success_envelope = herdr.is_some();
        let stdout = if let Some(connection) = herdr {
            run_herdr_attempt(backend, connection, binary, &input, dir.path(), timeout).await?
        } else {
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
            result.stdout
        };

        let text = match stdout {
            CliStdout::Claude(stdout) => {
                let value: serde_json::Value = serde_json::from_slice(&stdout)
                    .map_err(|e| CliSummarizeError::ParseOutput(backend, e.to_string()))?;
                if require_success_envelope
                    && value.get("is_error").and_then(|v| v.as_bool()) != Some(false)
                {
                    return Err(CliSummarizeError::ParseOutput(
                        backend,
                        "unsuccessful result envelope".into(),
                    ));
                }
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
    let provider_args = cli_args(backend, dir.path(), input.model, compat);
    let mut process = platform::Process::spawn(binary, &provider_args, Some(dir))
        .map_err(|e| launch_error(backend, e))?;
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
        read_stdout(backend, &mut process.stdout, false),
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

async fn run_herdr_attempt(
    backend: CliBackend,
    connection: &mut HerdrRpc,
    binary: &Path,
    input: &CliInput<'_>,
    dir: &Path,
    timeout: Duration,
) -> Result<CliStdout, CliSummarizeError> {
    let args = cli_args(backend, dir, input.model, false);
    let result = connection
        .exec(binary, &args, dir, Some(input.user), timeout)
        .await
        .map_err(|error| CliSummarizeError::ReadOutput(backend, error))?;
    if result.timed_out {
        return Err(CliSummarizeError::Timeout(backend));
    }
    if result.code != Some(0) || result.truncated {
        return Err(CliSummarizeError::ParseOutput(
            backend,
            "Herdr execution failed or output was truncated".into(),
        ));
    }
    let stdout = base64::engine::general_purpose::STANDARD
        .decode(result.stdout)
        .map_err(|_| {
            CliSummarizeError::ParseOutput(backend, "invalid Herdr output encoding".into())
        })?;
    read_stdout(backend, stdout.as_slice(), true).await
}

async fn read_stdout(
    backend: CliBackend,
    mut stdout: impl AsyncRead + Unpin,
    require_completed: bool,
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
    let mut completed = false;
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
            Ok(event) => match event.get("type").and_then(|v| v.as_str()) {
                Some("turn.completed") => completed = true,
                Some("turn.failed" | "turn.started") => completed = false,
                _ => {}
            },
            Err(_) if line.iter().all(u8::is_ascii_whitespace) => {}
            Err(_) => malformed = true,
        }
        line.clear();
    }
    if malformed || (require_completed && !completed) {
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
