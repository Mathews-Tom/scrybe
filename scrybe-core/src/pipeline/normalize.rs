// Copyright 2026 Mathews Tom
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//     https://www.apache.org/licenses/LICENSE-2.0

//! The single live capture-to-STT boundary, per source.
//!
//! Every conversion from a native capture frame to STT input goes
//! through this module: format validation, downmix to mono, and the one
//! resample to [`STT_SAMPLE_RATE`]. `docs/development-plan.md` §20.2
//! records the anti-pattern this closes — more than one resample stage
//! between capture and the recognizer.
//!
//! Two callers, one boundary each:
//!
//! - [`SourceNormalizer`] is the live streaming boundary. One per
//!   source, held for the whole session, so the anti-alias kernel keeps
//!   its context across capture frames instead of being restarted (and
//!   edge-clamped) on every frame.
//! - `session::build_audio_chunk` is the batch boundary used when no
//!   streaming capability is wired: it converts one already-completed
//!   `EmittedChunk` through [`validate_frame_format`] and
//!   [`downmix_to_mono`] plus a single stateless resample.
//!
//! A session uses exactly one of the two per source: with a streaming
//! provider the finalized chunk's audio is never converted or
//! transcribed a second time.

use crate::error::PipelineError;
use crate::pipeline::resample::StreamingResampler;
use crate::types::{AudioFrame, FrameSource};

/// Target rate for STT input. Whisper and the streaming Zipformer both
/// expect 16 kHz mono.
pub const STT_SAMPLE_RATE: u32 = 16_000;

/// Reject a frame whose declared format disagrees with the source's
/// established format, or is degenerate.
///
/// `index` identifies the offending frame for the operator: the frame's
/// position inside a batch chunk, or the per-source frame counter on the
/// live path.
///
/// # Errors
///
/// `PipelineError::InvalidFrame` when either expected value is zero or
/// the frame disagrees with them.
pub fn validate_frame_format(
    index: usize,
    frame_channels: u16,
    frame_rate: u32,
    channels: u16,
    sample_rate: u32,
) -> Result<(), PipelineError> {
    if sample_rate == 0 || channels == 0 || frame_rate != sample_rate || frame_channels != channels
    {
        return Err(PipelineError::InvalidFrame(format!(
            "STT input frame {index} has {frame_channels} channels at {frame_rate} Hz; \
             expected nonzero uniform {channels} channels at {sample_rate} Hz"
        )));
    }
    Ok(())
}

/// Average interleaved channels into mono. Returns an empty buffer for a
/// zero channel count; callers reject that format before calling.
#[allow(clippy::cast_precision_loss)]
#[must_use]
pub fn downmix_to_mono(interleaved: &[f32], channels: u16) -> Vec<f32> {
    let chans = usize::from(channels);
    if chans == 0 {
        return Vec::new();
    }
    if chans == 1 {
        return interleaved.to_vec();
    }
    let frames = interleaved.len() / chans;
    let mut out = Vec::with_capacity(frames);
    for f in 0..frames {
        let base = f * chans;
        let mut sum = 0.0_f32;
        for c in 0..chans {
            sum += interleaved[base + c];
        }
        out.push(sum / chans as f32);
    }
    out
}

/// Live per-source normalizer: native frames in, 16 kHz mono f32 out.
///
/// The first frame fixes the source's format; every later frame must
/// match it. The resampler is stateful, so the kernel context survives
/// across frames and the output equals a single stateless conversion of
/// the concatenated capture.
pub struct SourceNormalizer {
    source: FrameSource,
    /// Format plus its resampler, established by the first frame. One
    /// field so a validated format can never exist without the matching
    /// stateful resampler.
    live: Option<LiveFormat>,
    frames_seen: usize,
    emitted_samples: u64,
}

struct LiveFormat {
    sample_rate: u32,
    channels: u16,
    resampler: StreamingResampler,
}

impl SourceNormalizer {
    /// Create a normalizer for `source`. The format is learned from the
    /// first frame.
    #[must_use]
    pub const fn new(source: FrameSource) -> Self {
        Self {
            source,
            live: None,
            frames_seen: 0,
            emitted_samples: 0,
        }
    }

    /// Source this normalizer belongs to.
    #[must_use]
    pub const fn source(&self) -> FrameSource {
        self.source
    }

    /// Offset from the first normalized sample, in 16 kHz samples, of
    /// the next sample this normalizer will emit.
    #[must_use]
    pub const fn emitted_samples(&self) -> u64 {
        self.emitted_samples
    }

    /// Normalize one native capture frame.
    ///
    /// Returns the 16 kHz mono samples that became available; an empty
    /// vector means the resampler is still accumulating kernel context
    /// and is not an error.
    ///
    /// # Errors
    ///
    /// `PipelineError::InvalidFrame` for a degenerate or inconsistent
    /// frame format, `PipelineError::Resample` when the source rate
    /// cannot be converted.
    #[allow(clippy::cast_possible_truncation)]
    pub fn push(&mut self, frame: &AudioFrame) -> Result<Vec<f32>, PipelineError> {
        let (sample_rate, channels) = self
            .live
            .as_ref()
            .map_or((frame.sample_rate, frame.channels), |live| {
                (live.sample_rate, live.channels)
            });
        validate_frame_format(
            self.frames_seen,
            frame.channels,
            frame.sample_rate,
            channels,
            sample_rate,
        )?;
        self.frames_seen += 1;

        let mut live = match self.live.take() {
            Some(live) => live,
            None => LiveFormat {
                sample_rate,
                channels,
                resampler: StreamingResampler::new(sample_rate, STT_SAMPLE_RATE)?,
            },
        };
        let mono = downmix_to_mono(&frame.samples, channels);
        let out = live.resampler.push(&mono);
        self.live = Some(live);
        self.emitted_samples = self.emitted_samples.saturating_add(out.len() as u64);
        Ok(out)
    }

    /// Drain the resampler tail once capture has ended.
    #[allow(clippy::cast_possible_truncation)]
    pub fn finish(&mut self) -> Vec<f32> {
        let out = self
            .live
            .as_mut()
            .map(|live| live.resampler.finish())
            .unwrap_or_default();
        self.emitted_samples = self.emitted_samples.saturating_add(out.len() as u64);
        out
    }
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::cast_precision_loss,
    clippy::float_cmp
)]
mod tests {
    use super::*;
    use crate::pipeline::resample::resample_linear;
    use pretty_assertions::assert_eq;

    fn frame(samples: &[f32], channels: u16, sample_rate: u32) -> AudioFrame {
        AudioFrame::from_slice(samples, channels, sample_rate, 0, FrameSource::Mic)
    }

    fn sine(rate: u32, freq: f32, count: usize) -> Vec<f32> {
        (0..count)
            .map(|i| {
                let t = i as f32 / rate as f32;
                (t * freq * std::f32::consts::TAU).sin()
            })
            .collect()
    }

    #[test]
    fn test_normalizer_streams_stereo_48khz_into_16khz_mono() {
        let mut normalizer = SourceNormalizer::new(FrameSource::Mic);
        let interleaved: Vec<f32> = (0..480).flat_map(|_| [0.25_f32, 0.75_f32]).collect();

        let out = normalizer.push(&frame(&interleaved, 2, 48_000)).unwrap();
        let tail = normalizer.finish();

        assert_eq!(out.len() + tail.len(), 160);
        for sample in out.iter().chain(tail.iter()) {
            assert!((sample - 0.5).abs() < 1e-4, "downmix must average channels");
        }
    }

    #[test]
    fn test_normalizer_retains_filter_context_across_frames() {
        // A per-frame stateless resample clamps the anti-alias kernel at
        // every frame edge. Chunked streaming must instead reproduce one
        // stateless conversion of the whole capture, sample for sample.
        let full = sine(48_000, 3_000.0, 4_800);
        let expected = resample_linear(&full, 48_000, STT_SAMPLE_RATE).unwrap();

        let mut normalizer = SourceNormalizer::new(FrameSource::Mic);
        let mut streamed = Vec::new();
        for block in full.chunks(137) {
            streamed.extend(normalizer.push(&frame(block, 1, 48_000)).unwrap());
        }
        streamed.extend(normalizer.finish());

        assert_eq!(streamed.len(), expected.len());
        assert_eq!(streamed, expected);
    }

    #[test]
    fn test_per_frame_stateless_resample_differs_from_streamed_output() {
        // Guards the reason this type exists: the naive per-frame call
        // this replaces does not produce the streamed result.
        let full = sine(48_000, 3_000.0, 4_800);
        let expected = resample_linear(&full, 48_000, STT_SAMPLE_RATE).unwrap();

        let mut per_frame = Vec::new();
        for block in full.chunks(137) {
            per_frame.extend(resample_linear(block, 48_000, STT_SAMPLE_RATE).unwrap());
        }

        assert_ne!(per_frame, expected);
    }

    #[test]
    fn test_normalizer_rejects_channel_change_mid_stream() {
        let mut normalizer = SourceNormalizer::new(FrameSource::Mic);
        normalizer.push(&frame(&[0.0_f32; 48], 1, 48_000)).unwrap();

        let error = normalizer
            .push(&frame(&[0.0_f32; 96], 2, 48_000))
            .unwrap_err()
            .to_string();

        assert!(
            error.contains("frame 1 has 2 channels"),
            "unexpected error: {error}"
        );
    }

    #[test]
    fn test_normalizer_rejects_rate_change_mid_stream() {
        let mut normalizer = SourceNormalizer::new(FrameSource::Mic);
        normalizer.push(&frame(&[0.0_f32; 48], 1, 48_000)).unwrap();

        let error = normalizer
            .push(&frame(&[0.0_f32; 48], 1, 44_100))
            .unwrap_err()
            .to_string();

        assert!(error.contains("at 44100 Hz"), "unexpected error: {error}");
    }

    #[test]
    fn test_normalizer_rejects_degenerate_first_frame() {
        let mut normalizer = SourceNormalizer::new(FrameSource::System);

        let error = normalizer
            .push(&frame(&[0.0_f32; 48], 0, 48_000))
            .unwrap_err()
            .to_string();

        assert!(
            error.contains("expected nonzero"),
            "unexpected error: {error}"
        );
    }

    #[test]
    fn test_normalizer_tracks_emitted_sample_offsets() {
        let mut normalizer = SourceNormalizer::new(FrameSource::Mic);
        let mut total = 0_u64;
        for _ in 0..10 {
            total += normalizer
                .push(&frame(&[0.1_f32; 480], 1, 48_000))
                .unwrap()
                .len() as u64;
        }
        total += normalizer.finish().len() as u64;

        assert_eq!(normalizer.emitted_samples(), total);
        assert_eq!(total, 1_600);
    }
}
