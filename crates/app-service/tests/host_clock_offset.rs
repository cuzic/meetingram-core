//! Real capture backends stamp frames with a host clock that counts from boot
//! (QPC on Windows, mach time on macOS). The recorded session length must be the
//! span of the frames, not the absolute end time on that clock.

use app_service::pseudo_source::{generate_frames, nominal_frame_interval_ns, PseudoSourceConfig};
use app_service::run_pipeline;
use chrono::Utc;
use recorder_domain::{
    AudioManifest, CaptureManifest, ConsentManifest, RemoteSourceKind, SessionId, SessionManifest,
    TrackKind,
};
use session_store::SessionStore;
use std::sync::Arc;
use std::time::Duration;
use upload_client::mock_server::{spawn_test_server, FaultConfig};
use upload_client::{HttpUploadClient, StaticTokenProvider};

const SAMPLE_RATE: u32 = 48_000;
const DURATION_SECS: u32 = 30;
/// The VM this was found on had been up for about 48.7 hours.
const BOOT_OFFSET_NS: u64 = 48 * 3_600 * 1_000_000_000;

#[tokio::test]
async fn session_length_ignores_how_long_the_host_has_been_up() {
    let (base_url, _server_state) = spawn_test_server(FaultConfig {
        pre_process_fault_probability: 0.0,
        post_process_fault_probability: 0.0,
        timeout_simulation_probability: 0.0,
        timeout_sleep: Duration::from_millis(0),
    })
    .await;
    let adapter = HttpUploadClient::new(
        base_url,
        Duration::from_secs(5),
        Arc::new(StaticTokenProvider("test-token".to_string())),
    );
    let store = SessionStore::open_in_memory().unwrap();
    let session_dir = tempfile::tempdir().unwrap();
    let session_id = SessionId::new();
    let manifest = SessionManifest {
        schema_version: 1,
        session_id,
        started_at: Utc::now(),
        ended_at: None,
        platform: "linux".to_string(),
        app_version: "0.1.0".to_string(),
        capture: CaptureManifest {
            microphone_device_id: "pseudo-mic".to_string(),
            remote_source_id: "pseudo-speaker".to_string(),
            remote_source_kind: RemoteSourceKind::EndpointLoopback,
        },
        audio: AudioManifest {
            sample_rate: SAMPLE_RATE,
            segment_duration_ms: 30_000,
            tracks: vec![TrackKind::SelfMic, TrackKind::RemoteAudio],
        },
        consent: ConsentManifest {
            confirmed_by_user: true,
            confirmed_at: Utc::now(),
        },
    };
    let config = PseudoSourceConfig {
        duration_secs: DURATION_SECS,
        frame_interval_ms: 20,
        sample_rate: SAMPLE_RATE,
        channels: 1,
        tone_freq_hz: 440.0,
    };
    let shift = |track| {
        let mut frames = generate_frames(track, &config);
        for frame in &mut frames {
            frame.host_time_ns += BOOT_OFFSET_NS;
        }
        frames
    };
    let self_frames = shift(TrackKind::SelfMic);
    let remote_frames = shift(TrackKind::RemoteAudio);
    let interval = nominal_frame_interval_ns(&config);
    let end_ns = BOOT_OFFSET_NS + DURATION_SECS as u64 * 1_000_000_000;

    let summary = run_pipeline(
        &manifest,
        &self_frames,
        &remote_frames,
        interval,
        interval,
        end_ns,
        session_dir.path(),
        32_000,
        &store,
        &adapter,
    )
    .await
    .expect("pipeline should complete without error");

    assert_eq!(summary.total_duration_ms, DURATION_SECS as u64 * 1000);
    for track in [TrackKind::SelfMic, TrackKind::RemoteAudio] {
        assert_eq!(summary.segment_counts_by_track.get(&track), Some(&1));
    }
}
