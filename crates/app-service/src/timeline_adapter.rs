//! Converts one track's `CapturedFrame` sequence into a single continuous, aligned
//! `Vec<f32>` at a fixed nominal rate — the bridge between a capture backend's raw
//! frame stream and `audio-timeline`'s alignment policy.

use audio_timeline::{AudioPacket, TimelineAligner};
use recorder_domain::CapturedFrame;

use crate::normalize::normalize_to_mono;

/// Feeds every frame through `normalize_to_mono` and `TimelineAligner::ingest`, then
/// pads to `timeline_end_ns`. Both tracks of a session get the same
/// `timeline_start_ns` and `timeline_end_ns` (host clock), so they cover the same
/// span even if one started late, lost its final frames, or got no frames at all.
pub fn align_track(
    frames: &[CapturedFrame],
    nominal_rate_hz: u32,
    nominal_frame_interval_ns: u64,
    timeline_start_ns: u64,
    timeline_end_ns: u64,
) -> Vec<f32> {
    let mut aligner = TimelineAligner::starting_at(nominal_rate_hz, timeline_start_ns);
    for frame in frames {
        let mono = normalize_to_mono(frame, nominal_rate_hz);
        let packet = AudioPacket {
            host_time_ns: frame.host_time_ns,
            nominal_duration_ns: nominal_frame_interval_ns,
            samples: mono,
            discontinuity: frame.discontinuity,
        };
        aligner.ingest(&packet);
    }
    aligner.finalize_up_to(timeline_end_ns);
    aligner.into_output()
}

#[cfg(test)]
mod tests {
    use super::*;
    use recorder_domain::TrackKind;

    const RATE: u32 = 1_000;
    const INTERVAL_NS: u64 = 20_000_000; // 20 frames at 1 kHz
    const BOOT_NS: u64 = 48 * 3_600 * 1_000_000_000;

    fn frames(track: TrackKind, start_ns: u64, count: u64) -> Vec<CapturedFrame> {
        (0..count)
            .map(|i| CapturedFrame {
                track,
                host_time_ns: start_ns + i * INTERVAL_NS,
                source_time_ns: None,
                sample_rate: RATE,
                channels: 1,
                samples: vec![1.0; 20],
                discontinuity: false,
            })
            .collect()
    }

    #[test]
    fn a_late_track_gets_leading_silence_and_matches_the_other_track() {
        let start = BOOT_NS;
        let end = start + 50 * INTERVAL_NS; // 1 s
        let early = align_track(
            &frames(TrackKind::SelfMic, start, 50),
            RATE,
            INTERVAL_NS,
            start,
            end,
        );
        // Starts 0.5 s later.
        let late = align_track(
            &frames(TrackKind::RemoteAudio, start + 25 * INTERVAL_NS, 25),
            RATE,
            INTERVAL_NS,
            start,
            end,
        );
        assert_eq!(early.len(), 1_000);
        assert_eq!(late.len(), 1_000);
        assert!(late[..500].iter().all(|&s| s == 0.0));
        assert!(late[500..].iter().all(|&s| s == 1.0));
    }

    #[test]
    fn an_empty_track_is_padded_over_the_session_not_from_boot() {
        let start = BOOT_NS;
        let end = start + 50 * INTERVAL_NS;
        let empty = align_track(&[], RATE, INTERVAL_NS, start, end);
        assert_eq!(empty.len(), 1_000);
    }
}
