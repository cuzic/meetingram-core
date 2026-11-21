//! Orchestrates `capture -> align -> segment -> encode -> commit -> upload ->
//! finalize` for the meeting recorder. Stage 1 (task #7): an OS-independent pipeline
//! driven by `pseudo_source`, proving the wiring between `audio-timeline`,
//! `segment-store`, `session-store`, and `upload-client` without any real capture
//! backend. Stage 2 (task #10) adds a Windows supervisor and the frame-conversion
//! layer that feeds it into stage 1's pipeline. Stage 3 (task #11) adds a standing
//! upload worker and `CaptureState`/`UploadState` lifecycle management
//! (`upload_worker`, `session_lifecycle`), including force-quit crash recovery.

pub mod capture_health;
mod error;
mod normalize;
mod pipeline;
#[cfg(feature = "live-transcription")]
mod resample;
// Task #91: needs `stt-api`'s `BatchSttProvider` and `stt-deepgram`'s batch
// adapter, both optional deps only pulled in by `live-transcription` (see this
// crate's `Cargo.toml`) — unlike `live_transcription` itself, there is no
// feature-disabled stub, so the module doesn't exist at all without it (see
// `retranscribe`'s own doc comment).
#[cfg(feature = "live-transcription")]
mod retranscribe;
mod segmenter;
mod session_lifecycle;
mod silence_gate;
mod stt_provider_kind;
mod timeline_adapter;
mod upload_worker;

// `pub` (rather than a private `mod` re-exported piecemeal like the others above):
// nothing in this crate wires `TimestampMapper` into `live_transcription` yet (a
// separate task), so keeping it a private `mod` would make every item in it dead
// code under `-D warnings`. Making the module `pub` marks it as public API instead,
// which is also how callers will reach it once wiring lands:
// `app_service::timestamp_mapper::TimestampMapper`.
pub mod timestamp_mapper;

pub mod pseudo_source;

/// Settings for live transcription during a capture session.
#[derive(Default)]
#[non_exhaustive]
pub struct LiveTranscriptionOptions {
    pub silence_gate_enabled: bool,
    #[cfg(feature = "live-transcription")]
    pub stt_override: Option<SttOverride>,
}

impl LiveTranscriptionOptions {
    pub fn with_silence_gate(mut self, enabled: bool) -> Self {
        self.silence_gate_enabled = enabled;
        self
    }

    #[cfg(feature = "live-transcription")]
    pub fn with_stt_override(mut self, stt_override: SttOverride) -> Self {
        self.stt_override = Some(stt_override);
        self
    }
}

/// Caller-supplied providers for the Self and Remote live transcription tracks.
#[cfg(feature = "live-transcription")]
pub struct SttOverride {
    pub self_track: std::sync::Arc<dyn stt_api::SttProvider>,
    pub remote_track: std::sync::Arc<dyn stt_api::SttProvider>,
}

#[cfg(feature = "windows-supervisor")]
pub mod live_transcription;
#[cfg(all(windows, feature = "windows-supervisor"))]
pub mod windows_device_watch;
#[cfg(all(windows, feature = "windows-supervisor"))]
pub mod windows_frame_collector;
#[cfg(all(windows, feature = "windows-supervisor"))]
pub mod windows_session;
#[cfg(all(windows, feature = "windows-supervisor"))]
pub mod windows_supervisor;

// Only re-exported when `macos-supervisor` is off: both this and
// `macos_frame_collector::LevelSnapshot` are structurally identical but distinct
// types, so re-exporting both under the same top-level name would collide if a
// caller ever enabled both features together (e.g. `cargo build --all-features`,
// which nothing stops even though the two are only ever meaningful on their own
// respective OS). Callers building with `macos-supervisor` reach the macOS type via
// `app_service::macos_frame_collector::LevelSnapshot` instead of a root re-export.
#[cfg(all(
    windows,
    feature = "windows-supervisor",
    not(feature = "macos-supervisor")
))]
pub use windows_frame_collector::LevelSnapshot;
#[cfg(all(windows, feature = "windows-supervisor"))]
pub use windows_session::run_windows_capture_session;
// Same collision rationale as `LevelSnapshot` above: `capture_windows`/
// `capture_macos`'s `device_select::DeviceInfo` are structurally identical (both
// wrap `capture_api::rebinding::DeviceRole`) but distinct types, so only one is
// re-exported at the root when both features happen to be enabled together.
// `apps/desktop`'s settings screen calls these to list mics/speakers for its
// device picker without depending on `capture-windows`/`capture-macos` directly.
#[cfg(feature = "macos-supervisor")]
pub use capture_macos::device_select::{
    enumerate_capture_devices, enumerate_render_devices, DeviceInfo,
};
#[cfg(all(
    windows,
    feature = "windows-supervisor",
    not(feature = "macos-supervisor")
))]
pub use capture_windows::device_select::{
    enumerate_capture_devices, enumerate_render_devices, DeviceInfo,
};
// Same collision rationale as `DeviceInfo` above: a session-independent
// device-change watcher for `apps/desktop`'s Settings screen to auto-refresh its
// device list (see `windows_device_watch`/`macos_device_watch`'s doc comments).
#[cfg(feature = "macos-supervisor")]
pub use macos_device_watch::DeviceChangeWatcher;
#[cfg(all(
    windows,
    feature = "windows-supervisor",
    not(feature = "macos-supervisor")
))]
pub use windows_device_watch::DeviceChangeWatcher;
// `live_transcription` (unlike `LevelSnapshot`) has no macOS-side equivalent type
// to collide with (see that module's doc comment), so this re-export doesn't need
// the same `not(feature = "macos-supervisor")` guard.
#[cfg(feature = "windows-supervisor")]
pub use live_transcription::{TrackTranscriptionStatus, TranscriptionStatus};
#[cfg(feature = "live-transcription")]
pub use retranscribe::{retranscribe_gap, supports_batch_retranscription, RetranscribeError};

#[cfg(feature = "macos-supervisor")]
pub mod macos_device_watch;
#[cfg(feature = "macos-supervisor")]
pub mod macos_frame_collector;
#[cfg(feature = "macos-supervisor")]
pub mod macos_session;
#[cfg(feature = "macos-supervisor")]
pub mod macos_supervisor;

#[cfg(feature = "macos-supervisor")]
pub use macos_session::run_macos_capture_session;

pub use capture_health::{CaptureHealth, TrackHealth};
pub use error::AppServiceError;
pub use normalize::normalize_to_mono;
pub use pipeline::run_pipeline;
pub use segmenter::{segment_pcm, PendingSegment};
pub use session_lifecycle::{begin_session, end_session, recover_incomplete_sessions};
// Not yet wired into `live_transcription` (a separate task) — exported so it can be
// used and tested independently in the meantime.
pub use silence_gate::{GateAction, GateConfig, SilenceGate};
// Unconditional (no `windows-supervisor`/`live-transcription` gate, unlike
// `live_transcription`'s own re-export below): `apps/desktop`'s settings screen
// (task #49) needs to save/load the user's selected STT provider on every
// platform, not just where a real live-transcription session can actually open.
pub use stt_provider_kind::{SttProviderKind, CREDENTIAL_SERVICE, SELECTED_STT_PROVIDER_ACCOUNT};
pub use timeline_adapter::align_track;
pub use upload_worker::{run_until_drained, upload_pending_once, UploadPassSummary};

#[cfg(test)]
mod tests {
    #[test]
    fn live_transcription_silence_gate_is_disabled_by_default() {
        assert!(!super::LiveTranscriptionOptions::default().silence_gate_enabled);
    }
}
