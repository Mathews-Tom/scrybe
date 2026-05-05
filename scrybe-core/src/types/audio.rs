// Copyright 2026 Mathews Tom
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//     https://www.apache.org/licenses/LICENSE-2.0

//! Tier-1 audio frame and capture-capability types.
//!
//! `AudioFrame::samples` is `Arc<[f32]>` so frames fan out to the channel
//! splitter, the encoder, and the VAD/chunker without per-clone allocation
//! (`docs/system-design.md` §4.1).

use std::sync::Arc;

use serde::{Deserialize, Serialize};

/// Where a captured frame originated. Adapters set this once per stream.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FrameSource {
    Mic,
    System,
    Mixed,
}

/// One block of interleaved PCM samples emitted by an [`AudioCapture`]
/// implementation.
///
/// [`AudioCapture`]: crate::capture::AudioCapture
#[derive(Clone, Debug)]
pub struct AudioFrame {
    /// Interleaved PCM samples in `[-1.0, 1.0]`. `Arc<[f32]>` is cheap to
    /// clone for fan-out across the channel splitter, encoder, and VAD.
    pub samples: Arc<[f32]>,
    /// Native channel count of the originating capture stream. Per-
    /// source streams are routed by `source`, not by `channels`:
    /// a `Mic` frame at `channels = 2` is a stereo microphone, and a
    /// `System` frame at `channels = 2` is the macOS Core Audio Tap
    /// (which is created stereo by default). The
    /// `StereoInterleaver` down-mixes any multi-channel per-source
    /// frame to mono before pairing it with the other source for the
    /// final stereo `audio.opus`.
    pub channels: u16,
    /// Native rate from the platform; the pipeline resamples to 16 kHz
    /// before STT.
    pub sample_rate: u32,
    /// Monotonic timestamp in nanoseconds since session start. Capture
    /// adapters MUST emit non-decreasing timestamps for the chunker's
    /// contiguity invariant to hold.
    pub timestamp_ns: u64,
    pub source: FrameSource,
}

impl AudioFrame {
    /// Construct a frame from a slice. Allocates one `Arc<[f32]>`.
    #[must_use]
    pub fn from_slice(
        samples: &[f32],
        channels: u16,
        sample_rate: u32,
        timestamp_ns: u64,
        source: FrameSource,
    ) -> Self {
        Self {
            samples: Arc::from(samples),
            channels,
            sample_rate,
            timestamp_ns,
            source,
        }
    }

    /// Number of samples per channel in this frame.
    #[must_use]
    pub fn frames_per_channel(&self) -> usize {
        if self.channels == 0 {
            0
        } else {
            self.samples.len() / usize::from(self.channels)
        }
    }
}

/// Permission semantics declared by an `AudioCapture` adapter so the CLI
/// can render onboarding copy without `cfg(target_os)`.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PermissionModel {
    /// macOS Core Audio Taps (14.4+): tap-creation prompt, no orange dot.
    CoreAudioTap,
    /// macOS `ScreenCaptureKit` fallback (13.0–14.3): Screen Recording
    /// permission and orange recording indicator.
    ScreenRecording,
    /// Windows WASAPI loopback: no permission prompt for system audio;
    /// microphone access prompt is OS-managed.
    WasapiLoopback,
    /// Linux `PipeWire` / `PulseAudio`: portal-managed where applicable.
    PipeWirePortal,
    /// Android `MediaProjection`: runtime permission and a foreground
    /// service notification are required.
    MediaProjection,
}

/// Static metadata about what an `AudioCapture` adapter can do. Drives
/// runtime fallbacks in the pipeline (e.g., `Diarizer::requires_neural`).
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct Capabilities {
    pub supports_system_audio: bool,
    /// True only on Windows 10 build 2004+ (`AUDIOCLIENT_PROCESS_LOOPBACK_PARAMS`).
    pub supports_per_app_capture: bool,
    pub native_sample_rates: Vec<u32>,
    pub permission_model: PermissionModel,
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::cast_precision_loss,
    clippy::redundant_clone
)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    #[test]
    fn test_audio_frame_from_slice_clones_samples_into_arc() {
        let pcm: Vec<f32> = (0..8).map(|i| i as f32 * 0.1).collect();

        let frame = AudioFrame::from_slice(&pcm, 1, 48_000, 1_000_000, FrameSource::Mic);

        assert_eq!(frame.samples.len(), 8);
        assert_eq!(frame.channels, 1);
        assert_eq!(frame.sample_rate, 48_000);
        assert_eq!(frame.source, FrameSource::Mic);
        assert!((frame.samples[3] - 0.3).abs() < 1e-6);
    }

    #[test]
    fn test_audio_frame_clone_shares_arc_samples_without_reallocating() {
        let frame = AudioFrame::from_slice(&[0.0_f32; 1024], 2, 48_000, 0, FrameSource::Mixed);

        let clone = frame.clone();

        assert!(Arc::ptr_eq(&frame.samples, &clone.samples));
    }

    #[test]
    fn test_audio_frame_frames_per_channel_divides_samples_by_channels() {
        let pcm = vec![0.0_f32; 480];

        let stereo = AudioFrame::from_slice(&pcm, 2, 48_000, 0, FrameSource::Mixed);
        let mono = AudioFrame::from_slice(&pcm, 1, 48_000, 0, FrameSource::Mic);

        assert_eq!(stereo.frames_per_channel(), 240);
        assert_eq!(mono.frames_per_channel(), 480);
    }

    #[test]
    fn test_audio_frame_frames_per_channel_zero_channels_returns_zero() {
        let frame = AudioFrame::from_slice(&[0.0_f32; 16], 0, 48_000, 0, FrameSource::Mic);

        assert_eq!(frame.frames_per_channel(), 0);
    }

    #[test]
    fn test_frame_source_serializes_to_lowercase_tag() {
        let json = serde_json::to_string(&FrameSource::System).unwrap();

        assert_eq!(json, "\"system\"");
    }

    #[test]
    fn test_capabilities_round_trips_through_json() {
        let caps = Capabilities {
            supports_system_audio: true,
            supports_per_app_capture: false,
            native_sample_rates: vec![48_000, 44_100],
            permission_model: PermissionModel::CoreAudioTap,
        };

        let encoded = serde_json::to_string(&caps).unwrap();
        let decoded: Capabilities = serde_json::from_str(&encoded).unwrap();

        assert_eq!(decoded, caps);
    }

    #[test]
    fn test_permission_model_serializes_kebab_case() {
        let json = serde_json::to_string(&PermissionModel::CoreAudioTap).unwrap();

        assert_eq!(json, "\"core-audio-tap\"");
    }
}
