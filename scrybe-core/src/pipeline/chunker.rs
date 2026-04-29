// Copyright 2026 Mathews Tom
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//     https://www.apache.org/licenses/LICENSE-2.0

//! VAD-aware chunker. Emits an `EmittedChunk` whenever either:
//!
//! 1. 30 s of audio have accumulated, OR
//! 2. ≥ 5 s of silence follow ≥ 5 s of speech.
//!
//! Five-second silence aligns with the `WhisperX` / `faster-whisper`
//! merge window. Two-second silence (the original draft in
//! `.docs/development-plan.md` §2) produced too-short chunks and hurt
//! the long-context decoder.

use std::time::Duration;

use crate::pipeline::vad::{Vad, VadDecision};
use crate::types::{AudioFrame, FrameSource};

/// Tunable thresholds for the chunker. Defaults match `system-design.md`
/// §5; tests override to keep fixtures under one second.
#[derive(Clone, Copy, Debug)]
pub struct ChunkerConfig {
    pub max_chunk: Duration,
    pub min_speech_before_silence_split: Duration,
    pub silence_split_after: Duration,
}

impl Default for ChunkerConfig {
    fn default() -> Self {
        Self {
            max_chunk: Duration::from_secs(30),
            min_speech_before_silence_split: Duration::from_secs(5),
            silence_split_after: Duration::from_secs(5),
        }
    }
}

/// Output of the chunker — a contiguous run of frames at the source's
/// native rate, plus the chunker's classification of how the chunk ended.
#[derive(Clone, Debug)]
pub struct EmittedChunk {
    pub frames: Vec<AudioFrame>,
    pub start: Duration,
    pub duration: Duration,
    pub source: FrameSource,
    pub ended_on: ChunkBoundary,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ChunkBoundary {
    MaxDuration,
    SilenceAfterSpeech,
    EndOfStream,
}

/// Sink that receives chunks.
///
/// The pipeline's session orchestrator implements this to forward
/// chunks to the resampler and STT provider; tests use
/// `Vec<EmittedChunk>` directly via the closure adapter from
/// [`Chunker::push`].
pub trait ChunkSink {
    fn accept(&mut self, chunk: EmittedChunk);
}

impl<F: FnMut(EmittedChunk)> ChunkSink for F {
    fn accept(&mut self, chunk: EmittedChunk) {
        self(chunk);
    }
}

/// VAD-aware chunker. Stateless across sessions; one `Chunker` per
/// channel per session.
pub struct Chunker<V: Vad> {
    config: ChunkerConfig,
    vad: V,
    source: FrameSource,
    pending: Vec<AudioFrame>,
    pending_duration: Duration,
    speech_duration: Duration,
    trailing_silence: Duration,
    chunk_start: Option<Duration>,
}

impl<V: Vad> Chunker<V> {
    #[must_use]
    pub const fn new(config: ChunkerConfig, vad: V, source: FrameSource) -> Self {
        Self {
            config,
            vad,
            source,
            pending: Vec::new(),
            pending_duration: Duration::ZERO,
            speech_duration: Duration::ZERO,
            trailing_silence: Duration::ZERO,
            chunk_start: None,
        }
    }

    /// Feed a frame through the VAD and the chunker; emit any boundary
    /// the new frame triggers via `sink`.
    pub fn push<S: ChunkSink>(&mut self, frame: AudioFrame, sink: &mut S) {
        let frame_duration = frame_duration(&frame);
        let decision = self.vad.decide(&frame);

        if self.chunk_start.is_none() {
            self.chunk_start = Some(timestamp_to_duration(frame.timestamp_ns));
        }

        match decision {
            VadDecision::Speech => {
                self.speech_duration = self.speech_duration.saturating_add(frame_duration);
                self.trailing_silence = Duration::ZERO;
            }
            VadDecision::Silence => {
                self.trailing_silence = self.trailing_silence.saturating_add(frame_duration);
            }
        }

        self.pending_duration = self.pending_duration.saturating_add(frame_duration);
        self.pending.push(frame);

        if self.pending_duration >= self.config.max_chunk {
            self.flush(sink, ChunkBoundary::MaxDuration);
            return;
        }

        let speech_long_enough =
            self.speech_duration >= self.config.min_speech_before_silence_split;
        let silence_long_enough = self.trailing_silence >= self.config.silence_split_after;
        if speech_long_enough && silence_long_enough {
            self.flush(sink, ChunkBoundary::SilenceAfterSpeech);
        }
    }

    /// Emit any buffered frames as a final chunk. Idempotent.
    pub fn finish<S: ChunkSink>(&mut self, sink: &mut S) {
        if !self.pending.is_empty() {
            self.flush(sink, ChunkBoundary::EndOfStream);
        }
    }

    fn flush<S: ChunkSink>(&mut self, sink: &mut S, boundary: ChunkBoundary) {
        if self.pending.is_empty() {
            return;
        }
        let frames = std::mem::take(&mut self.pending);
        let start = self.chunk_start.unwrap_or_default();
        let duration = self.pending_duration;
        sink.accept(EmittedChunk {
            frames,
            start,
            duration,
            source: self.source,
            ended_on: boundary,
        });
        self.pending_duration = Duration::ZERO;
        self.speech_duration = Duration::ZERO;
        self.trailing_silence = Duration::ZERO;
        self.chunk_start = None;
    }
}

#[allow(clippy::cast_precision_loss)]
fn frame_duration(frame: &AudioFrame) -> Duration {
    if frame.sample_rate == 0 || frame.channels == 0 {
        return Duration::ZERO;
    }
    let frames_per_channel = frame.samples.len() / usize::from(frame.channels);
    let secs = frames_per_channel as f64 / f64::from(frame.sample_rate);
    Duration::from_secs_f64(secs)
}

const fn timestamp_to_duration(ns: u64) -> Duration {
    Duration::from_nanos(ns)
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss
)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::pipeline::vad::EnergyVad;
    use crate::types::FrameSource;
    use pretty_assertions::assert_eq;

    const RATE: u32 = 16_000;

    const fn cfg_short() -> ChunkerConfig {
        ChunkerConfig {
            max_chunk: Duration::from_millis(300),
            min_speech_before_silence_split: Duration::from_millis(50),
            silence_split_after: Duration::from_millis(50),
        }
    }

    fn frame(samples: Vec<f32>, timestamp_ns: u64) -> AudioFrame {
        AudioFrame {
            samples: Arc::from(samples),
            channels: 1,
            sample_rate: RATE,
            timestamp_ns,
            source: FrameSource::Mic,
        }
    }

    fn speech_frame(timestamp_ns: u64, frame_size: usize) -> AudioFrame {
        let samples: Vec<f32> = (0..frame_size).map(|n| (n as f32 * 0.5).sin()).collect();
        frame(samples, timestamp_ns)
    }

    fn silence_frame(timestamp_ns: u64, frame_size: usize) -> AudioFrame {
        frame(vec![0.0_f32; frame_size], timestamp_ns)
    }

    fn ns_for_samples(samples: usize) -> u64 {
        u64::try_from((samples as u128 * 1_000_000_000) / u128::from(RATE)).unwrap_or(u64::MAX)
    }

    #[test]
    fn test_chunker_emits_chunk_at_max_duration() {
        let cfg = cfg_short();
        let mut chunker = Chunker::new(cfg, EnergyVad::default(), FrameSource::Mic);
        let mut chunks = Vec::new();
        let mut sink = |c: EmittedChunk| chunks.push(c);

        let frame_size = 1_600;
        let frames_needed =
            ((cfg.max_chunk.as_secs_f64() * f64::from(RATE)) / frame_size as f64).ceil() as usize;
        for i in 0..frames_needed {
            let ts = ns_for_samples(i * frame_size);
            chunker.push(speech_frame(ts, frame_size), &mut sink);
        }

        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0].ended_on, ChunkBoundary::MaxDuration);
        assert!(chunks[0].duration >= cfg.max_chunk);
    }

    #[test]
    fn test_chunker_emits_chunk_on_silence_after_sufficient_speech() {
        let cfg = cfg_short();
        let mut chunker = Chunker::new(cfg, EnergyVad::default(), FrameSource::Mic);
        let mut chunks = Vec::new();
        let mut sink = |c: EmittedChunk| chunks.push(c);

        let frame_size = 800;
        let speech_frames = 2;
        for i in 0..speech_frames {
            let ts = ns_for_samples(i * frame_size);
            chunker.push(speech_frame(ts, frame_size), &mut sink);
        }
        let silence_frames = 2;
        for i in 0..silence_frames {
            let ts = ns_for_samples((speech_frames + i) * frame_size);
            chunker.push(silence_frame(ts, frame_size), &mut sink);
        }

        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0].ended_on, ChunkBoundary::SilenceAfterSpeech);
    }

    #[test]
    fn test_chunker_does_not_emit_when_only_silence_present() {
        let cfg = cfg_short();
        let mut chunker = Chunker::new(cfg, EnergyVad::default(), FrameSource::Mic);
        let mut chunks = Vec::new();
        let mut sink = |c: EmittedChunk| chunks.push(c);

        for i in 0..3 {
            let ts = ns_for_samples(i * 800);
            chunker.push(silence_frame(ts, 800), &mut sink);
        }

        assert!(
            chunks.is_empty(),
            "pure silence must not trigger silence-after-speech boundary; got {chunks:?}"
        );
    }

    #[test]
    fn test_chunker_finish_emits_remaining_frames_as_end_of_stream_chunk() {
        let cfg = cfg_short();
        let mut chunker = Chunker::new(cfg, EnergyVad::default(), FrameSource::Mic);
        let mut chunks = Vec::new();
        let mut sink = |c: EmittedChunk| chunks.push(c);

        let frame_size = 800;
        chunker.push(speech_frame(0, frame_size), &mut sink);
        chunker.finish(&mut sink);

        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0].ended_on, ChunkBoundary::EndOfStream);
    }

    #[test]
    fn test_chunker_finish_is_idempotent_on_empty_buffer() {
        let cfg = cfg_short();
        let mut chunker = Chunker::new(cfg, EnergyVad::default(), FrameSource::Mic);
        let mut chunks = Vec::new();
        let mut sink = |c: EmittedChunk| chunks.push(c);

        chunker.finish(&mut sink);
        chunker.finish(&mut sink);

        assert!(chunks.is_empty());
    }

    #[test]
    fn test_chunker_resets_speech_counter_after_emitting_chunk() {
        let cfg = cfg_short();
        let mut chunker = Chunker::new(cfg, EnergyVad::default(), FrameSource::Mic);
        let mut chunks = Vec::new();
        let mut sink = |c: EmittedChunk| chunks.push(c);

        let frame_size = 800;
        for i in 0..2 {
            chunker.push(
                speech_frame(ns_for_samples(i * frame_size), frame_size),
                &mut sink,
            );
        }
        for i in 0..2 {
            chunker.push(
                silence_frame(ns_for_samples((2 + i) * frame_size), frame_size),
                &mut sink,
            );
        }
        for i in 0..2 {
            chunker.push(
                silence_frame(ns_for_samples((4 + i) * frame_size), frame_size),
                &mut sink,
            );
        }

        assert_eq!(chunks.len(), 1, "second silence run must not emit a chunk");
    }

    #[test]
    fn test_chunker_emits_chunks_with_contiguous_non_overlapping_starts() {
        let cfg = cfg_short();
        let mut chunker = Chunker::new(cfg, EnergyVad::default(), FrameSource::Mic);
        let mut chunks = Vec::new();
        let mut sink = |c: EmittedChunk| chunks.push(c);

        let frame_size = 1_600;
        for i in 0..7 {
            let ts = ns_for_samples(i * frame_size);
            chunker.push(speech_frame(ts, frame_size), &mut sink);
        }
        chunker.finish(&mut sink);

        let mut last_end = Duration::ZERO;
        for c in &chunks {
            assert!(c.start >= last_end, "overlap at chunk start={:?}", c.start);
            last_end = c.start + c.duration;
        }
        assert!(!chunks.is_empty());
    }

    #[test]
    fn test_chunker_records_source_on_emitted_chunk() {
        let cfg = cfg_short();
        let mut chunker = Chunker::new(cfg, EnergyVad::default(), FrameSource::System);
        let mut chunks = Vec::new();
        let mut sink = |c: EmittedChunk| chunks.push(c);

        chunker.push(speech_frame(0, 800), &mut sink);
        chunker.finish(&mut sink);

        assert_eq!(chunks[0].source, FrameSource::System);
    }
}
