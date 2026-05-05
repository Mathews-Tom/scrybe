// Copyright 2026 Mathews Tom
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//     https://www.apache.org/licenses/LICENSE-2.0

//! Stereo channel-split interleaver.
//!
//! When `--source mic+system` is active, mic frames (`FrameSource::Mic`)
//! and system frames (`FrameSource::System`) arrive on separate streams
//! at the same native sample rate but with independent timing. The
//! encoder needs interleaved stereo PCM where the left channel carries
//! mic samples and the right carries system samples.
//!
//! `StereoInterleaver` buffers each source's mono samples in a ring,
//! emits aligned L/R pairs as soon as both sides have data, and zero-
//! fills the lagging side when one source lags beyond `max_lag_samples`.
//! This keeps the encoder's wall-clock granulepos in lockstep with the
//! session's wall-clock duration — the v1.0.4 bug where `audio.opus`
//! ran ~2× the session length stems from pushing each source's mono
//! samples serially into a mono encoder.
//!
//! Sample-rate normalization is out of scope: both `MicCapture` and
//! `MacCapture` deliver 48 kHz today. A future device emitting a
//! different rate fails fast at `push` rather than resampling silently.

use std::collections::VecDeque;

use crate::error::PipelineError;
use crate::types::{AudioFrame, FrameSource};

/// Minimum number of samples to drain in a single interleave pass.
/// 480 samples = 10 ms at 48 kHz, which is half of one Opus 20-ms
/// frame so the encoder can still buffer to its packet boundary.
const MIN_DRAIN_SAMPLES: usize = 480;

/// Interleaves per-source mono PCM into stereo (L=mic, R=system).
pub struct StereoInterleaver {
    sample_rate: u32,
    mic_ring: VecDeque<f32>,
    sys_ring: VecDeque<f32>,
    max_lag_samples: usize,
}

impl StereoInterleaver {
    /// Construct a new interleaver for `sample_rate` Hz with bounded
    /// per-side buffering. `max_lag_ms` caps how far one source may
    /// lead the other before the lagging side is zero-filled.
    #[must_use]
    pub fn new(sample_rate: u32, max_lag_ms: u32) -> Self {
        let max_lag_samples = (sample_rate as usize / 1000) * max_lag_ms as usize;
        Self {
            sample_rate,
            mic_ring: VecDeque::with_capacity(max_lag_samples * 2),
            sys_ring: VecDeque::with_capacity(max_lag_samples * 2),
            max_lag_samples,
        }
    }

    /// Push a mono frame from either source. Routes by `frame.source`.
    /// `FrameSource::Mixed` is rejected — the interleaver expects
    /// per-source mono streams, not a pre-mixed feed.
    ///
    /// # Errors
    ///
    /// `PipelineError::InvalidFrame` when the frame's `sample_rate` or
    /// `channels` does not match the interleaver's contract, or when a
    /// `Mixed` frame is received.
    pub fn push(&mut self, frame: &AudioFrame) -> Result<(), PipelineError> {
        if frame.sample_rate != self.sample_rate {
            return Err(PipelineError::InvalidFrame(format!(
                "interleaver configured for {} Hz, frame is {} Hz",
                self.sample_rate, frame.sample_rate
            )));
        }
        if frame.channels != 1 {
            return Err(PipelineError::InvalidFrame(format!(
                "interleaver expects mono per-source frames, got {} channels",
                frame.channels
            )));
        }
        let ring = match frame.source {
            FrameSource::Mic => &mut self.mic_ring,
            FrameSource::System => &mut self.sys_ring,
            FrameSource::Mixed => {
                return Err(PipelineError::InvalidFrame(
                    "Mixed source frames are not valid input to StereoInterleaver".to_string(),
                ));
            }
        };
        ring.extend(frame.samples.iter().copied());
        Ok(())
    }

    /// Drain whatever aligned stereo samples are currently available.
    /// Returns interleaved `[L, R, L, R, …]` PCM. Empty when neither
    /// side has at least `MIN_DRAIN_SAMPLES` and lag is within bounds.
    #[must_use]
    pub fn drain(&mut self) -> Vec<f32> {
        let aligned = self.mic_ring.len().min(self.sys_ring.len());
        let lead = self.mic_ring.len().max(self.sys_ring.len());
        let lag = lead - aligned;

        let take = if aligned >= MIN_DRAIN_SAMPLES {
            // Both sides have enough — drain the smaller side's worth.
            aligned
        } else if lag > self.max_lag_samples {
            // One side is lagging too far. Zero-fill the shorter side
            // up to the longer side's length and drain everything so
            // wall-clock keeps moving.
            self.zero_fill_to(lead);
            lead
        } else {
            // Both sides still within tolerance, wait for more.
            return Vec::new();
        };

        let mut out = Vec::with_capacity(take * 2);
        for _ in 0..take {
            // VecDeque::pop_front returns Option but the loop bound
            // ensures both rings hold at least `take` samples.
            let l = self.mic_ring.pop_front().unwrap_or(0.0);
            let r = self.sys_ring.pop_front().unwrap_or(0.0);
            out.push(l);
            out.push(r);
        }
        out
    }

    /// Drain everything remaining, zero-filling the shorter side so
    /// the final stereo length is `max(mic_ring, sys_ring)`. Called
    /// once at session end before encoder finalize.
    #[must_use]
    pub fn finish(&mut self) -> Vec<f32> {
        let lead = self.mic_ring.len().max(self.sys_ring.len());
        if lead == 0 {
            return Vec::new();
        }
        self.zero_fill_to(lead);
        let mut out = Vec::with_capacity(lead * 2);
        for _ in 0..lead {
            let l = self.mic_ring.pop_front().unwrap_or(0.0);
            let r = self.sys_ring.pop_front().unwrap_or(0.0);
            out.push(l);
            out.push(r);
        }
        out
    }

    fn zero_fill_to(&mut self, target_len: usize) {
        if self.mic_ring.len() < target_len {
            let pad = target_len - self.mic_ring.len();
            self.mic_ring.extend(std::iter::repeat_n(0.0_f32, pad));
        }
        if self.sys_ring.len() < target_len {
            let pad = target_len - self.sys_ring.len();
            self.sys_ring.extend(std::iter::repeat_n(0.0_f32, pad));
        }
    }
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::cast_precision_loss
)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    fn frame(samples: &[f32], source: FrameSource) -> AudioFrame {
        AudioFrame::from_slice(samples, 1, 48_000, 0, source)
    }

    #[test]
    fn test_interleaver_aligned_frames_emit_interleaved_lr_pairs() {
        let mut iv = StereoInterleaver::new(48_000, 200);
        let mic_samples: Vec<f32> = (0..480).map(|i| i as f32 * 0.001).collect();
        let sys_samples: Vec<f32> = (0..480).map(|i| -(i as f32) * 0.001).collect();

        iv.push(&frame(&mic_samples, FrameSource::Mic)).unwrap();
        iv.push(&frame(&sys_samples, FrameSource::System)).unwrap();

        let out = iv.drain();

        assert_eq!(out.len(), 960);
        for i in 0..480 {
            assert!((out[i * 2] - mic_samples[i]).abs() < 1e-9);
            assert!((out[i * 2 + 1] - sys_samples[i]).abs() < 1e-9);
        }
    }

    #[test]
    fn test_interleaver_below_min_drain_samples_returns_empty() {
        let mut iv = StereoInterleaver::new(48_000, 200);
        iv.push(&frame(&[0.5_f32; 100], FrameSource::Mic)).unwrap();
        iv.push(&frame(&[0.5_f32; 100], FrameSource::System))
            .unwrap();

        assert!(iv.drain().is_empty());
        assert_eq!(iv.mic_ring.len(), 100);
        assert_eq!(iv.sys_ring.len(), 100);
    }

    #[test]
    fn test_interleaver_one_side_lag_within_tolerance_waits() {
        let mut iv = StereoInterleaver::new(48_000, 200);
        iv.push(&frame(&[1.0_f32; 480], FrameSource::Mic)).unwrap();
        // System ring empty; lag = 480 samples = 10 ms < 200 ms tolerance.
        let out = iv.drain();

        assert!(out.is_empty());
        assert_eq!(iv.mic_ring.len(), 480);
    }

    #[test]
    fn test_interleaver_one_side_lag_over_tolerance_zero_fills() {
        let mut iv = StereoInterleaver::new(48_000, 5);
        // 5 ms tolerance = 240 samples at 48 kHz. Push 480 mic samples
        // and zero system samples — lag exceeds tolerance, drain must
        // emit 480 stereo frames with R=0.
        iv.push(&frame(&[0.7_f32; 480], FrameSource::Mic)).unwrap();
        let out = iv.drain();

        assert_eq!(out.len(), 960);
        for i in 0..480 {
            assert!((out[i * 2] - 0.7).abs() < 1e-9);
            assert!(out[i * 2 + 1].abs() < 1e-9);
        }
        assert_eq!(iv.mic_ring.len(), 0);
        assert_eq!(iv.sys_ring.len(), 0);
    }

    #[test]
    fn test_interleaver_finish_zero_fills_shorter_side() {
        let mut iv = StereoInterleaver::new(48_000, 200);
        iv.push(&frame(&[0.3_f32; 1000], FrameSource::Mic)).unwrap();
        iv.push(&frame(&[0.4_f32; 600], FrameSource::System))
            .unwrap();

        let out = iv.finish();

        assert_eq!(out.len(), 2000);
        for i in 0..600 {
            assert!((out[i * 2] - 0.3).abs() < 1e-9);
            assert!((out[i * 2 + 1] - 0.4).abs() < 1e-9);
        }
        for i in 600..1000 {
            assert!((out[i * 2] - 0.3).abs() < 1e-9);
            assert!(out[i * 2 + 1].abs() < 1e-9);
        }
    }

    #[test]
    fn test_interleaver_finish_with_empty_rings_returns_empty() {
        let mut iv = StereoInterleaver::new(48_000, 200);
        assert!(iv.finish().is_empty());
    }

    #[test]
    fn test_interleaver_rejects_mixed_source_frames() {
        let mut iv = StereoInterleaver::new(48_000, 200);
        let err = iv
            .push(&frame(&[0.0_f32; 100], FrameSource::Mixed))
            .unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("Mixed"), "expected Mixed rejection, got {msg}");
    }

    #[test]
    fn test_interleaver_rejects_sample_rate_mismatch() {
        let mut iv = StereoInterleaver::new(48_000, 200);
        let bad = AudioFrame::from_slice(&[0.0_f32; 100], 1, 44_100, 0, FrameSource::Mic);
        let err = iv.push(&bad).unwrap_err();
        assert!(err.to_string().contains("44100"));
    }

    #[test]
    fn test_interleaver_rejects_non_mono_frames() {
        let mut iv = StereoInterleaver::new(48_000, 200);
        let bad = AudioFrame::from_slice(&[0.0_f32; 100], 2, 48_000, 0, FrameSource::Mic);
        let err = iv.push(&bad).unwrap_err();
        assert!(err.to_string().contains("2 channels"));
    }

    #[test]
    fn test_interleaver_drain_empty_when_no_frames_pushed() {
        let mut iv = StereoInterleaver::new(48_000, 200);
        assert!(iv.drain().is_empty());
    }
}
