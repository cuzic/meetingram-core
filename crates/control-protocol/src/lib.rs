//! Wire types and local-socket naming for the desktop app's control server
//! (`apps/desktop/src/control_server.rs`) and the `1on1ctl` CLI
//! (`apps/desktop-cli`) to talk to each other. Deliberately depends on neither
//! side — `desktop` needs `interprocess`'s `tokio` feature, `desktop-cli` uses
//! the sync API, and both must resolve to the exact same socket name/path or
//! they'd silently never find each other.
//!
//! One connection = one request line + one response line (newline-delimited
//! JSON), then the connection closes — a CLI invocation is short-lived, so
//! there's no need for a persistent multiplexed session.

use std::io;
use std::path::PathBuf;

use interprocess::local_socket::{
    GenericFilePath, GenericNamespaced, Name, NameType, ToFsName, ToNsName,
};
use serde::{Deserialize, Serialize};

/// Same base `apps/desktop/src/main.rs::app_data_dir()` uses. Re-derived here
/// (not shared via a dependency on `desktop`, which has no `[lib]` target
/// anyway) rather than imported — `main.rs`'s version also does a one-time
/// migration from an old Tauri-era path, which is main.rs-specific and
/// irrelevant to where the control socket lives.
pub fn app_data_dir() -> PathBuf {
    dirs::data_dir()
        .unwrap_or_else(std::env::temp_dir)
        .join("meetingram")
}

pub fn control_socket_path() -> PathBuf {
    app_data_dir().join("control.sock")
}

/// `true` only on platforms where `control_socket_name()` falls back to a real
/// filesystem path (macOS today, since it lacks Linux's abstract socket
/// namespace) — i.e. the only platforms where a stale socket *file* can be left
/// behind after the owning process exits. Windows named pipes and Linux's
/// abstract namespace are both kernel-reclaimed on process exit, crash or not,
/// so `control_server::bind`'s stale-socket recovery path only needs to run
/// here.
pub fn uses_filesystem_path() -> bool {
    !GenericNamespaced::is_supported()
}

/// Resolves to a namespaced name (Windows named pipe / Linux abstract socket)
/// where supported, falling back to `control_socket_path()` only where it
/// isn't (macOS) — mirrors `interprocess`'s own recommended pattern for a name
/// that's valid on every platform this app targets.
pub fn control_socket_name() -> io::Result<Name<'static>> {
    if GenericNamespaced::is_supported() {
        "meetingram-control.sock".to_ns_name::<GenericNamespaced>()
    } else {
        control_socket_path().to_fs_name::<GenericFilePath>()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Command {
    Status,
    Start,
    Stop,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Response {
    Ok(StatusDto),
    /// Carries the current status alongside the error so a failed `start`/
    /// `stop` still lets the caller see real state without a second
    /// round-trip — every response confirms actual state, never just a bare
    /// pass/fail.
    Err {
        message: String,
        status: StatusDto,
    },
}

/// Serde-friendly mirror of `apps/desktop/src/status.rs`'s `Status` — kept as
/// a separate type (rather than reusing `Status` directly) so this crate never
/// depends on `desktop`; `desktop`'s `control_server.rs` converts between the
/// two with a `From` impl.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct StatusDto {
    pub recording: bool,
    pub elapsed_ms: u64,
    pub self_rms: f32,
    pub self_peak: f32,
    pub remote_rms: f32,
    pub remote_peak: f32,
    pub consent_confirmed: bool,
    pub uploaded_segments: usize,
    pub pending_segments: usize,
    pub last_error: Option<String>,
    pub last_session_id: Option<String>,
    pub last_total_duration_ms: Option<u64>,
    pub transcription_status: TranscriptionStatusDto,
    /// Serde-friendly mirror of `app_service::CaptureHealth` — lets an external
    /// observer (a device connect/disconnect E2E test driving `1on1ctl --json
    /// status`, or a future UI) see the rebinding FSM's per-track health without
    /// this crate depending on `app_service`'s supervisor features.
    pub capture_health: CaptureHealthDto,
    /// Graph status for the session reported by the server, when available.
    // Use the explicit default for older payloads whose graph field is absent.
    #[serde(default, deserialize_with = "Deserialize::deserialize")]
    pub graph: Option<GraphStatusDto>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct GraphStatusDto {
    pub session_id: String,
    pub rev: u64,
    pub enabled: bool,
    pub running: bool,
    pub pending: usize,
    /// Whether the session has a record of completed consolidation.
    pub consolidated: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
pub struct TranscriptionStatusDto {
    pub self_status: TrackStatusDto,
    pub remote_status: TrackStatusDto,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
pub enum TrackStatusDto {
    #[default]
    NotConfigured,
    Connecting,
    Connected,
    Error(String),
    Unavailable,
}

/// See `StatusDto::capture_health`'s doc comment.
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
pub struct CaptureHealthDto {
    pub self_health: TrackHealthDto,
    pub remote_health: TrackHealthDto,
}

/// Mirrors `app_service::TrackHealth`, which itself mirrors
/// `capture_api::rebinding::BindingHealth` — see those types' doc comments.
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
pub enum TrackHealthDto {
    #[default]
    Ok,
    Unavailable,
    Retrying {
        attempt: u32,
    },
    Failed {
        reason: String,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn command_roundtrips_through_json() {
        for cmd in [Command::Status, Command::Start, Command::Stop] {
            let json = serde_json::to_string(&cmd).unwrap();
            let back: Command = serde_json::from_str(&json).unwrap();
            assert_eq!(format!("{cmd:?}"), format!("{back:?}"));
        }
    }

    #[test]
    fn response_roundtrips_through_json() {
        let ok = Response::Ok(StatusDto {
            recording: true,
            elapsed_ms: 42,
            ..Default::default()
        });
        let json = serde_json::to_string(&ok).unwrap();
        let back: Response = serde_json::from_str(&json).unwrap();
        match back {
            Response::Ok(dto) => {
                assert!(dto.recording);
                assert_eq!(dto.elapsed_ms, 42);
            }
            Response::Err { .. } => panic!("expected Ok"),
        }

        let err = Response::Err {
            message: "not recording".to_string(),
            status: StatusDto::default(),
        };
        let json = serde_json::to_string(&err).unwrap();
        let back: Response = serde_json::from_str(&json).unwrap();
        match back {
            Response::Err { message, .. } => assert_eq!(message, "not recording"),
            Response::Ok(_) => panic!("expected Err"),
        }
    }

    #[test]
    fn status_reads_legacy_json_without_graph() {
        let legacy = serde_json::json!({
            "recording": true,
            "elapsed_ms": 42,
            "self_rms": 0.0,
            "self_peak": 0.0,
            "remote_rms": 0.0,
            "remote_peak": 0.0,
            "consent_confirmed": true,
            "uploaded_segments": 3,
            "pending_segments": 1,
            "last_error": null,
            "last_session_id": "session-1",
            "last_total_duration_ms": 42,
            "transcription_status": {
                "self_status": "Connected",
                "remote_status": "NotConfigured"
            },
            "capture_health": { "self_health": "Ok", "remote_health": "Ok" }
        });
        let status: StatusDto = serde_json::from_value(legacy).unwrap();
        assert!(status.recording);
        assert!(status.graph.is_none());
    }

    #[test]
    fn graph_status_roundtrips_through_json() {
        let graph = GraphStatusDto {
            session_id: "session-1".into(),
            rev: 12,
            enabled: true,
            running: false,
            pending: 0,
            consolidated: true,
        };
        let status = StatusDto {
            graph: Some(graph.clone()),
            ..Default::default()
        };
        let json = serde_json::to_value(status).unwrap();
        assert_eq!(json["graph"]["rev"], 12);
        let decoded: StatusDto = serde_json::from_value(json).unwrap();
        assert_eq!(decoded.graph, Some(graph));
    }

    #[test]
    fn socket_name_resolves_on_this_platform() {
        // On Linux (this sandbox) `GenericNamespaced` is supported, so this
        // exercises the real namespaced branch rather than the macOS-only
        // filesystem-path fallback.
        assert!(!uses_filesystem_path());
        control_socket_name().expect("namespaced name should resolve on Linux");
    }
}
