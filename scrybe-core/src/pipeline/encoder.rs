// Copyright 2026 Mathews Tom
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//     https://www.apache.org/licenses/LICENSE-2.0

//! Opus encoder seam.
//!
//! The pipeline writes audio to disk through this trait so the storage
//! layer never touches a codec directly. `NullEncoder` is the
//! deterministic test-only impl that emits PCM-shaped bytes for
//! assertions about page boundaries; `OggOpusEncoder` ships behind the
//! `encoder-opus` feature flag in the CLI binary.
//!
//! Page flushing is the contract: the implementation MUST commit a
//! recoverable boundary every `EncoderConfig::page_interval`. The
//! storage layer's `append_durable` calls fsync on each returned page
//! so a crash mid-session loses at most the most recent partial page.

use std::time::Duration;

use crate::error::PipelineError;

/// Tunable encoder parameters. Defaults match `system-design.md` §9 —
/// Opus 32 kbps, 1-second pages.
#[derive(Clone, Copy, Debug)]
pub struct EncoderConfig {
    pub sample_rate: u32,
    pub channels: u16,
    pub bitrate_bps: u32,
    pub page_interval: Duration,
}

impl Default for EncoderConfig {
    fn default() -> Self {
        Self {
            sample_rate: 48_000,
            channels: 1,
            bitrate_bps: 32_000,
            page_interval: Duration::from_secs(1),
        }
    }
}

/// Opus encoder seam. Implementations buffer frames and emit byte
/// blobs at page boundaries.
pub trait Encoder: Send {
    /// Feed PCM samples into the encoder. Returns any byte payload that
    /// completed a page boundary; an empty `Vec` means the encoder is
    /// still buffering.
    ///
    /// # Errors
    ///
    /// Implementations return `PipelineError::OpusEncode` for codec
    /// failures.
    fn push_pcm(&mut self, samples: &[f32]) -> Result<Vec<u8>, PipelineError>;

    /// Flush the encoder's tail buffer at session end. After calling
    /// `finish`, further calls to `push_pcm` are undefined; callers
    /// drop the encoder and finalize the audio file.
    ///
    /// # Errors
    ///
    /// `PipelineError::OpusEncode` when the codec rejects the flush.
    fn finish(&mut self) -> Result<Vec<u8>, PipelineError>;
}

/// Test-only encoder.
///
/// Buffers PCM as little-endian f32 bytes and emits a "page" every
/// `page_interval` of audio. Useful for asserting page-flush behavior
/// in pipeline tests without pulling in an Opus dependency.
pub struct NullEncoder {
    config: EncoderConfig,
    pending: Vec<f32>,
    samples_per_page: usize,
}

impl NullEncoder {
    #[must_use]
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    pub fn new(config: EncoderConfig) -> Self {
        let samples_per_page = (f64::from(config.sample_rate) * config.page_interval.as_secs_f64())
            .round() as usize
            * usize::from(config.channels.max(1));
        Self {
            config,
            pending: Vec::with_capacity(samples_per_page * 2),
            samples_per_page: samples_per_page.max(1),
        }
    }

    #[must_use]
    pub const fn config(&self) -> EncoderConfig {
        self.config
    }
}

impl Encoder for NullEncoder {
    fn push_pcm(&mut self, samples: &[f32]) -> Result<Vec<u8>, PipelineError> {
        self.pending.extend_from_slice(samples);
        if self.pending.len() < self.samples_per_page {
            return Ok(Vec::new());
        }
        let take = self.samples_per_page;
        let drained: Vec<f32> = self.pending.drain(..take).collect();
        Ok(pcm_to_bytes(&drained))
    }

    fn finish(&mut self) -> Result<Vec<u8>, PipelineError> {
        if self.pending.is_empty() {
            return Ok(Vec::new());
        }
        let drained = std::mem::take(&mut self.pending);
        Ok(pcm_to_bytes(&drained))
    }
}

fn pcm_to_bytes(samples: &[f32]) -> Vec<u8> {
    let mut out = Vec::with_capacity(samples.len() * 4);
    for sample in samples {
        out.extend_from_slice(&sample.to_le_bytes());
    }
    out
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
    use pretty_assertions::assert_eq;

    const fn cfg() -> EncoderConfig {
        EncoderConfig {
            sample_rate: 16_000,
            channels: 1,
            bitrate_bps: 32_000,
            page_interval: Duration::from_millis(100),
        }
    }

    #[test]
    fn test_null_encoder_emits_page_when_buffer_reaches_page_size() {
        let mut enc = NullEncoder::new(cfg());
        let samples_per_page = 1_600;
        let pcm = vec![0.5_f32; samples_per_page];

        let out = enc.push_pcm(&pcm).unwrap();

        assert_eq!(out.len(), samples_per_page * 4);
    }

    #[test]
    fn test_null_encoder_buffers_below_page_size_without_emitting() {
        let mut enc = NullEncoder::new(cfg());
        let pcm = vec![0.5_f32; 800];

        let out = enc.push_pcm(&pcm).unwrap();

        assert!(out.is_empty());
    }

    #[test]
    fn test_null_encoder_finish_returns_tail_below_page_threshold() {
        let mut enc = NullEncoder::new(cfg());
        enc.push_pcm(&vec![0.25_f32; 200]).unwrap();

        let tail = enc.finish().unwrap();

        assert_eq!(tail.len(), 200 * 4);
    }

    #[test]
    fn test_null_encoder_finish_returns_empty_when_no_buffer() {
        let mut enc = NullEncoder::new(cfg());

        let tail = enc.finish().unwrap();

        assert!(tail.is_empty());
    }

    #[test]
    fn test_null_encoder_emits_multiple_pages_for_large_burst() {
        let mut enc = NullEncoder::new(cfg());

        let first = enc.push_pcm(&vec![0.1_f32; 1_600]).unwrap();
        let second = enc.push_pcm(&vec![0.2_f32; 1_600]).unwrap();

        assert_eq!(first.len(), 1_600 * 4);
        assert_eq!(second.len(), 1_600 * 4);
    }

    #[test]
    fn test_null_encoder_round_trip_decodes_to_within_5_percent_rms() {
        let mut enc = NullEncoder::new(cfg());
        let pcm = vec![0.5_f32; 1_600];

        let bytes = enc.push_pcm(&pcm).unwrap();

        let decoded: Vec<f32> = bytes
            .chunks_exact(4)
            .map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]]))
            .collect();
        let in_rms = (pcm.iter().map(|s| s * s).sum::<f32>() / pcm.len() as f32).sqrt();
        let out_rms = (decoded.iter().map(|s| s * s).sum::<f32>() / decoded.len() as f32).sqrt();
        let ratio = out_rms / in_rms;
        assert!(
            (ratio - 1.0).abs() < 0.05,
            "round-trip RMS deviates by more than 5%: {ratio}"
        );
    }

    #[test]
    fn test_encoder_config_default_uses_documented_v0_1_settings() {
        let c = EncoderConfig::default();

        assert_eq!(c.sample_rate, 48_000);
        assert_eq!(c.channels, 1);
        assert_eq!(c.bitrate_bps, 32_000);
        assert_eq!(c.page_interval, Duration::from_secs(1));
    }
}
