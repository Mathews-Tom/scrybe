// Copyright 2026 Mathews Tom
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//     https://www.apache.org/licenses/LICENSE-2.0

//! Stereo channel-split interleaver with timestamp-aware alignment.
//!
//! When `--source mic+system` is active, mic frames (`FrameSource::Mic`)
//! and system frames (`FrameSource::System`) arrive on separate streams
//! at the same native sample rate but with independent timing. The
//! encoder needs interleaved stereo PCM where the left channel carries
//! mic samples and the right carries system samples.
//!
//! Each side has a ring buffer of pending mono samples plus a
//! `head_ns` cursor that records the wall-clock timestamp (nanoseconds
//! since session start) of the next-to-emit sample. `drain` walks the
//! two cursors forward together, dropping samples from whichever side
//! is earlier so the L/R pairs it emits represent the same wall-clock
//! instant. The lagging side is zero-filled when its head trails the
//! leading head by more than `max_lag_samples`.
//!
//! The earlier arrival-order pairing strategy worked for the common
//! case (both sources start at T=0 and deliver at exactly the same
//! effective rate), but produced glitchy stereo whenever the two
//! adapters diverged: different startup wall-clock, frame-cadence
//! jitter, lost frames, or physical-clock drift between cpal and
//! `CoreAudio`. The encoder then wrote audio whose L and R channels
//! each looked clean in isolation but carried a slowly-accumulating
//! temporal offset, which Whisper hallucinated as `(fast forwarding)`
//! and similar non-speech events on the Them channel.
//!
//! Sample-rate normalization is out of scope: both adapters must
//! deliver at the rate the interleaver was constructed for. Frames
//! at the wrong rate fail fast at `push` (the orchestrator is
//! responsible for resampling on the encoder side).

use std::collections::VecDeque;

use crate::error::PipelineError;
use crate::types::{AudioFrame, FrameSource};

/// Minimum number of samples to drain in a single interleave pass.
/// 480 samples = 10 ms at 48 kHz, which is half of one Opus 20-ms
/// frame so the encoder can still buffer to its packet boundary.
const MIN_DRAIN_SAMPLES: usize = 480;

/// Per-source state tracked alongside its sample ring.
///
/// `head_ns` is the wall-clock timestamp (in nanoseconds since session
/// start) of the next-to-emit sample in `ring`. When `ring` empties,
/// `head_ns` retains the timestamp the next sample would have had so
/// the next push can validate continuity. `head_ns` is `None` until
/// the first frame from this source arrives.
struct Channel {
    ring: VecDeque<f32>,
    head_ns: Option<u64>,
}

impl Channel {
    fn with_capacity(cap: usize) -> Self {
        Self {
            ring: VecDeque::with_capacity(cap),
            head_ns: None,
        }
    }

    fn len(&self) -> usize {
        self.ring.len()
    }
}

/// Interleaves per-source mono PCM into stereo (L=mic, R=system).
pub struct StereoInterleaver {
    sample_rate: u32,
    mic: Channel,
    sys: Channel,
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
            mic: Channel::with_capacity(max_lag_samples * 2),
            sys: Channel::with_capacity(max_lag_samples * 2),
            max_lag_samples,
        }
    }

    /// Convert a sample count to a nanosecond offset at this
    /// interleaver's sample rate, using exact fractional arithmetic
    /// so a steady-state 10 ms / 480 samples conversion lands on
    /// `10_000_000` ns rather than `9_999_840` ns (the value a
    /// truncated `step_ns` constant would give).
    fn samples_to_ns(&self, samples: u64) -> u64 {
        if self.sample_rate == 0 {
            return 0;
        }
        samples.saturating_mul(1_000_000_000) / u64::from(self.sample_rate)
    }

    /// Convert a nanosecond offset to a sample count at this
    /// interleaver's sample rate, rounded down. Inverse of
    /// `samples_to_ns` modulo sub-sample remainder.
    fn ns_to_samples(&self, ns: u64) -> u64 {
        if self.sample_rate == 0 {
            return 0;
        }
        ns.saturating_mul(u64::from(self.sample_rate)) / 1_000_000_000
    }

    /// Push a frame from either source. Routes by `frame.source`,
    /// down-mixes multi-channel frames to mono, and reconciles the
    /// frame's `timestamp_ns` against the channel's wall-clock cursor
    /// so any inter-frame gap or overlap shows up as zero-padding or
    /// overlap-skipping instead of silent drift. Without this, mic
    /// and system rings drift relative to each other over the course
    /// of a recording and produce temporally-misaligned stereo at
    /// the encoder.
    ///
    /// The macOS Core Audio Tap delivers stereo natively
    /// (`initStereoGlobalTapButExcludeProcesses`), and a stereo USB
    /// mic on the cpal path produces 2-channel input. The interleaver
    /// averages those channels per sample-time so each side of the
    /// L/R output stays a single mono signal.
    ///
    /// `FrameSource::Mixed` is rejected — the interleaver's input
    /// contract is the unmixed per-source split. A pre-mixed feed has
    /// already lost the channel attribution this stage exists to
    /// preserve.
    ///
    /// # Errors
    ///
    /// `PipelineError::InvalidFrame` when the frame's `sample_rate`
    /// does not match the interleaver's contract, when `channels`
    /// is zero, or when a `Mixed` frame is received.
    #[allow(clippy::cast_possible_truncation)]
    pub fn push(&mut self, frame: &AudioFrame) -> Result<(), PipelineError> {
        if frame.sample_rate != self.sample_rate {
            return Err(PipelineError::InvalidFrame(format!(
                "interleaver configured for {} Hz, frame is {} Hz",
                self.sample_rate, frame.sample_rate
            )));
        }
        if frame.channels == 0 {
            return Err(PipelineError::InvalidFrame(
                "interleaver received frame with zero channels".to_string(),
            ));
        }
        let mut mono = if frame.channels == 1 {
            frame.samples.to_vec()
        } else {
            let channels = usize::from(frame.channels);
            let inv = 1.0_f32 / f32::from(frame.channels);
            frame
                .samples
                .chunks_exact(channels)
                .map(|chunk| chunk.iter().sum::<f32>() * inv)
                .collect::<Vec<f32>>()
        };

        // Pre-compute the buffered nanoseconds before borrowing the
        // channel mutably (samples_to_ns needs `&self`).
        let pre_push_buffered_samples = match frame.source {
            FrameSource::Mic => self.mic.ring.len() as u64,
            FrameSource::System => self.sys.ring.len() as u64,
            FrameSource::Mixed => {
                return Err(PipelineError::InvalidFrame(
                    "Mixed source frames are not valid input to StereoInterleaver".to_string(),
                ));
            }
        };
        let buffered_ns = self.samples_to_ns(pre_push_buffered_samples);

        let max_lag = self.max_lag_samples;
        let channel = match frame.source {
            FrameSource::Mic => &mut self.mic,
            FrameSource::System => &mut self.sys,
            FrameSource::Mixed => unreachable!("rejected above"),
        };

        if let Some(head_ns) = channel.head_ns {
            // Where the next sample logically belongs given what's
            // already buffered.
            let expected_ns = head_ns.saturating_add(buffered_ns);
            let actual_ns = frame.timestamp_ns;
            if actual_ns > expected_ns {
                // Gap: source skipped wall-clock time between frames.
                // Bridge with zero samples so subsequent draining
                // emits the silence the gap actually represents.
                // Bounded by `max_lag_samples` to prevent runaway
                // buffering on a clock that rolls over or restarts.
                let gap_ns = actual_ns - expected_ns;
                // ns_to_samples is inlined here because we cannot
                // borrow `self` while the mut-ref `channel` is alive.
                let gap_samples = if self.sample_rate == 0 {
                    0
                } else {
                    (gap_ns.saturating_mul(u64::from(self.sample_rate)) / 1_000_000_000) as usize
                };
                let gap_samples = gap_samples.min(max_lag);
                channel
                    .ring
                    .extend(std::iter::repeat_n(0.0_f32, gap_samples));
            } else if actual_ns < expected_ns {
                // Overlap: new frame's wall-clock starts before the
                // ring's tail. The source re-delivered samples we
                // already accounted for. Drop the overlapping
                // prefix of the new frame.
                let overlap_ns = expected_ns - actual_ns;
                let overlap_samples = if self.sample_rate == 0 {
                    0
                } else {
                    (overlap_ns.saturating_mul(u64::from(self.sample_rate)) / 1_000_000_000)
                        as usize
                };
                let drop = overlap_samples.min(mono.len());
                mono.drain(..drop);
            }
        } else {
            // First frame for this source: anchor the head cursor at
            // the frame's wall-clock timestamp.
            channel.head_ns = Some(frame.timestamp_ns);
        }

        channel.ring.extend(mono);
        Ok(())
    }

    /// Drain whatever timestamp-aligned stereo samples are currently
    /// available. Before pairing, the channel whose head trails the
    /// other (i.e. holds older wall-clock samples than the partner)
    /// drops its head-leading samples until both heads are aligned;
    /// this cleans up an initial wall-clock skew between the two
    /// adapters' first frames. Returns interleaved `[L, R, L, R, …]`
    /// PCM. Empty when neither side has at least `MIN_DRAIN_SAMPLES`
    /// of post-alignment audio AND the lag is within tolerance.
    #[must_use]
    #[allow(clippy::cast_possible_truncation)]
    pub fn drain(&mut self) -> Vec<f32> {
        self.align_heads();

        let aligned = self.mic.len().min(self.sys.len());
        let lead = self.mic.len().max(self.sys.len());
        let lag = lead - aligned;

        let take = if aligned >= MIN_DRAIN_SAMPLES {
            // Both sides have post-alignment audio — drain the
            // smaller side's worth.
            aligned
        } else if lag > self.max_lag_samples {
            // One side is lagging too far. Zero-fill the shorter
            // side up to the longer side's length so wall-clock
            // keeps moving even though the lagging adapter has gone
            // quiet.
            self.zero_fill_to(lead);
            lead
        } else {
            // Both sides still within tolerance, wait for more.
            return Vec::new();
        };

        let mut out = Vec::with_capacity(take * 2);
        for _ in 0..take {
            let l = self.mic.ring.pop_front().unwrap_or(0.0);
            let r = self.sys.ring.pop_front().unwrap_or(0.0);
            out.push(l);
            out.push(r);
        }
        // Advance head cursors so a subsequent push correctly detects
        // discontinuity against the post-drain ring tail.
        let advance_ns = self.samples_to_ns(take as u64);
        if let Some(h) = self.mic.head_ns {
            self.mic.head_ns = Some(h.saturating_add(advance_ns));
        }
        if let Some(h) = self.sys.head_ns {
            self.sys.head_ns = Some(h.saturating_add(advance_ns));
        }
        out
    }

    /// Drain everything remaining, zero-filling the shorter side so
    /// the final stereo length is `max(mic.ring, sys.ring)`. Called
    /// once at session end before encoder finalize. Skips head
    /// alignment because at session end, dropping head-leading
    /// samples would lose audio that's already final.
    #[must_use]
    #[allow(clippy::cast_possible_truncation)]
    pub fn finish(&mut self) -> Vec<f32> {
        let lead = self.mic.len().max(self.sys.len());
        if lead == 0 {
            return Vec::new();
        }
        self.zero_fill_to(lead);
        let mut out = Vec::with_capacity(lead * 2);
        for _ in 0..lead {
            let l = self.mic.ring.pop_front().unwrap_or(0.0);
            let r = self.sys.ring.pop_front().unwrap_or(0.0);
            out.push(l);
            out.push(r);
        }
        let advance_ns = self.samples_to_ns(lead as u64);
        if let Some(h) = self.mic.head_ns {
            self.mic.head_ns = Some(h.saturating_add(advance_ns));
        }
        if let Some(h) = self.sys.head_ns {
            self.sys.head_ns = Some(h.saturating_add(advance_ns));
        }
        out
    }

    /// Drop samples from whichever channel's head is earlier so the
    /// remaining samples on both sides start at the same wall-clock
    /// timestamp. No-op when either channel has not yet seen a
    /// frame, or when the heads already match.
    #[allow(clippy::cast_possible_truncation)]
    fn align_heads(&mut self) {
        let (Some(mic_head), Some(sys_head)) = (self.mic.head_ns, self.sys.head_ns) else {
            return;
        };
        if mic_head == sys_head {
            return;
        }
        if mic_head < sys_head {
            // Mic samples represent earlier wall-clock than sys's
            // earliest sample. Drop mic's head-leading samples so
            // the next pair represents matched wall-clock instants.
            let drop_ns = sys_head - mic_head;
            let drop_samples = (self.ns_to_samples(drop_ns) as usize).min(self.mic.ring.len());
            self.mic.ring.drain(..drop_samples);
            let advanced_ns = self.samples_to_ns(drop_samples as u64);
            self.mic.head_ns = Some(mic_head.saturating_add(advanced_ns));
        } else {
            let drop_ns = mic_head - sys_head;
            let drop_samples = (self.ns_to_samples(drop_ns) as usize).min(self.sys.ring.len());
            self.sys.ring.drain(..drop_samples);
            let advanced_ns = self.samples_to_ns(drop_samples as u64);
            self.sys.head_ns = Some(sys_head.saturating_add(advanced_ns));
        }
    }

    fn zero_fill_to(&mut self, target_len: usize) {
        if self.mic.len() < target_len {
            let pad = target_len - self.mic.len();
            self.mic.ring.extend(std::iter::repeat_n(0.0_f32, pad));
            // The lagging side may have no head yet (no frames
            // received). Anchor it to the partner's head so future
            // frames reconcile against the right wall-clock.
            if self.mic.head_ns.is_none() {
                self.mic.head_ns = self.sys.head_ns;
            }
        }
        if self.sys.len() < target_len {
            let pad = target_len - self.sys.len();
            self.sys.ring.extend(std::iter::repeat_n(0.0_f32, pad));
            if self.sys.head_ns.is_none() {
                self.sys.head_ns = self.mic.head_ns;
            }
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

    fn frame_at(samples: &[f32], source: FrameSource, timestamp_ns: u64) -> AudioFrame {
        AudioFrame::from_slice(samples, 1, 48_000, timestamp_ns, source)
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
        assert_eq!(iv.mic.len(), 100);
        assert_eq!(iv.sys.len(), 100);
    }

    #[test]
    fn test_interleaver_one_side_lag_within_tolerance_waits() {
        let mut iv = StereoInterleaver::new(48_000, 200);
        iv.push(&frame(&[1.0_f32; 480], FrameSource::Mic)).unwrap();
        // System ring empty; lag = 480 samples = 10 ms < 200 ms tolerance.
        let out = iv.drain();

        assert!(out.is_empty());
        assert_eq!(iv.mic.len(), 480);
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
        assert_eq!(iv.mic.len(), 0);
        assert_eq!(iv.sys.len(), 0);
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
    fn test_interleaver_rejects_zero_channel_frames() {
        let mut iv = StereoInterleaver::new(48_000, 200);
        let bad = AudioFrame::from_slice(&[0.0_f32; 16], 0, 48_000, 0, FrameSource::Mic);
        let err = iv.push(&bad).unwrap_err();
        assert!(err.to_string().contains("zero channels"));
    }

    #[test]
    fn test_interleaver_downmixes_stereo_per_source_frames_to_mono() {
        let mut iv = StereoInterleaver::new(48_000, 200);
        // Stereo mic frame: L=1.0, R=0.0 throughout. Average = 0.5.
        // 480 stereo frames = 960 interleaved samples, downmix to 480
        // mono samples per side.
        let stereo_samples: Vec<f32> = (0..480).flat_map(|_| [1.0_f32, 0.0_f32]).collect();
        let mic = AudioFrame::from_slice(&stereo_samples, 2, 48_000, 0, FrameSource::Mic);
        // Stereo system frame: L=-0.5, R=-0.5. Average = -0.5.
        let sys_samples: Vec<f32> = (0..480).flat_map(|_| [-0.5_f32, -0.5_f32]).collect();
        let sys = AudioFrame::from_slice(&sys_samples, 2, 48_000, 0, FrameSource::System);

        iv.push(&mic).unwrap();
        iv.push(&sys).unwrap();
        let out = iv.drain();

        assert_eq!(out.len(), 960, "expected 480 stereo pairs after downmix");
        for pair in out.chunks_exact(2) {
            assert!(
                (pair[0] - 0.5).abs() < 1e-6,
                "L channel should carry mic downmix 0.5, got {}",
                pair[0]
            );
            assert!(
                (pair[1] + 0.5).abs() < 1e-6,
                "R channel should carry system downmix -0.5, got {}",
                pair[1]
            );
        }
    }

    #[test]
    fn test_interleaver_drain_empty_when_no_frames_pushed() {
        let mut iv = StereoInterleaver::new(48_000, 200);
        assert!(iv.drain().is_empty());
    }

    #[test]
    fn test_interleaver_drops_mic_head_samples_when_mic_starts_before_sys() {
        // 1 ms = 48 mic samples that should be dropped because
        // sys started 1 ms after mic. After alignment, the next pair
        // should pop sys[0] alongside mic[48], not mic[0].
        let mut iv = StereoInterleaver::new(48_000, 200);
        let mic_samples: Vec<f32> = (0..600).map(|i| i as f32 * 0.001).collect();
        let sys_samples: Vec<f32> = (0..600).map(|i| -(i as f32) * 0.001).collect();

        iv.push(&frame_at(&mic_samples, FrameSource::Mic, 0))
            .unwrap();
        iv.push(&frame_at(&sys_samples, FrameSource::System, 1_000_000))
            .unwrap();

        let out = iv.drain();

        // After alignment: mic head advanced by ~48 samples,
        // remaining mic = 600-48 = 552, sys still at 600. aligned=552.
        assert_eq!(out.len(), 552 * 2);
        // First L sample should be mic[48] (or thereabouts).
        let first_l = out[0];
        let expected_l = mic_samples[48];
        assert!(
            (first_l - expected_l).abs() < 1e-6,
            "L[0] should be mic[48]={expected_l}, got {first_l}"
        );
        // First R sample should be sys[0].
        let first_r = out[1];
        let expected_r = sys_samples[0];
        assert!(
            (first_r - expected_r).abs() < 1e-6,
            "R[0] should be sys[0]={expected_r}, got {first_r}"
        );
    }

    #[test]
    fn test_interleaver_drops_sys_head_samples_when_sys_starts_before_mic() {
        let mut iv = StereoInterleaver::new(48_000, 200);
        let mic_samples: Vec<f32> = (0..600).map(|i| i as f32 * 0.001).collect();
        let sys_samples: Vec<f32> = (0..600).map(|i| -(i as f32) * 0.001).collect();

        iv.push(&frame_at(&sys_samples, FrameSource::System, 0))
            .unwrap();
        iv.push(&frame_at(&mic_samples, FrameSource::Mic, 1_000_000))
            .unwrap();

        let out = iv.drain();

        assert_eq!(out.len(), 552 * 2);
        let first_l = out[0];
        let expected_l = mic_samples[0];
        assert!(
            (first_l - expected_l).abs() < 1e-6,
            "L[0] should be mic[0]={expected_l}, got {first_l}"
        );
        let first_r = out[1];
        let expected_r = sys_samples[48];
        assert!(
            (first_r - expected_r).abs() < 1e-6,
            "R[0] should be sys[48]={expected_r}, got {first_r}"
        );
    }

    #[test]
    fn test_interleaver_bridges_intra_source_gap_with_zero_padding() {
        // First mic frame at T=0 with 480 samples covers 0..10 ms.
        // Second mic frame at T=20 ms (10 ms gap) means the source
        // skipped 10 ms of audio. The interleaver should bridge with
        // zeros so subsequent draining preserves wall-clock layout.
        let mut iv = StereoInterleaver::new(48_000, 200);
        iv.push(&frame_at(&[1.0_f32; 480], FrameSource::Mic, 0))
            .unwrap();
        iv.push(&frame_at(&[1.0_f32; 480], FrameSource::Mic, 20_000_000))
            .unwrap();

        // Mic ring should now hold:  480 ones, 480 zeros, 480 ones.
        // (480 zeros = 10 ms gap bridged.)
        assert_eq!(
            iv.mic.len(),
            480 + 480 + 480,
            "expected 1440 samples (480 frame + 480 zeros + 480 frame), got {}",
            iv.mic.len()
        );
        // Verify the gap section is exactly zeros.
        let mid_zeros: Vec<f32> = iv.mic.ring.iter().skip(480).take(480).copied().collect();
        assert!(mid_zeros.iter().all(|&s| s == 0.0));
    }

    #[test]
    fn test_interleaver_skips_overlapping_prefix_when_frame_predates_ring_tail() {
        // Mic frame 1 at T=0 with 480 samples covers 0..10 ms.
        // Mic frame 2 at T=5 ms with 480 samples overlaps the first
        // by 5 ms (240 samples). The first 240 samples of frame 2
        // are duplicates of the second half of frame 1 and must be
        // discarded so the ring tail doesn't double-count.
        let mut iv = StereoInterleaver::new(48_000, 200);
        iv.push(&frame_at(&[0.1_f32; 480], FrameSource::Mic, 0))
            .unwrap();
        iv.push(&frame_at(&[0.9_f32; 480], FrameSource::Mic, 5_000_000))
            .unwrap();

        // Expected ring: 480 of 0.1, then 240 of 0.9 (the non-
        // overlapping tail of frame 2).
        assert_eq!(iv.mic.len(), 480 + 240);
        let last_240: Vec<f32> = iv.mic.ring.iter().skip(480).copied().collect();
        assert!(last_240.iter().all(|&s| (s - 0.9).abs() < 1e-9));
    }

    #[test]
    fn test_interleaver_steady_state_pairing_matches_wall_clock() {
        // Both sources start at T=0 and deliver 480 samples per 10 ms
        // cleanly. Pairing should advance heads by exactly 480 samples
        // per drain pass.
        let mut iv = StereoInterleaver::new(48_000, 200);
        iv.push(&frame_at(&[0.5_f32; 480], FrameSource::Mic, 0))
            .unwrap();
        iv.push(&frame_at(&[-0.5_f32; 480], FrameSource::System, 0))
            .unwrap();
        let _ = iv.drain();

        assert_eq!(iv.mic.head_ns, Some(10_000_000));
        assert_eq!(iv.sys.head_ns, Some(10_000_000));

        iv.push(&frame_at(&[0.5_f32; 480], FrameSource::Mic, 10_000_000))
            .unwrap();
        iv.push(&frame_at(&[-0.5_f32; 480], FrameSource::System, 10_000_000))
            .unwrap();
        let out2 = iv.drain();

        assert_eq!(out2.len(), 960);
        assert_eq!(iv.mic.head_ns, Some(20_000_000));
        assert_eq!(iv.sys.head_ns, Some(20_000_000));
    }

    #[test]
    fn test_interleaver_zero_fill_anchors_missing_head_to_partner() {
        // When sys never pushes a frame and max_lag triggers a
        // zero-fill, sys.head_ns must adopt the mic's head so that a
        // future sys frame can reconcile against the right wall-clock.
        let mut iv = StereoInterleaver::new(48_000, 5);
        iv.push(&frame_at(&[0.7_f32; 480], FrameSource::Mic, 0))
            .unwrap();
        let _ = iv.drain();

        assert_eq!(iv.sys.head_ns, Some(10_000_000));
        assert_eq!(iv.mic.head_ns, Some(10_000_000));
    }
}
