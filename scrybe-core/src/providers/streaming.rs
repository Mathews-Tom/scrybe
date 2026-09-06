// Copyright 2026 Mathews Tom
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//     https://www.apache.org/licenses/LICENSE-2.0

//! `StreamingSttProvider` — additive Tier-2 streaming capability.
//!
//! `SttProvider::transcribe` receives an already-completed chunk, so it
//! structurally cannot expose a growing hypothesis while capture is
//! still running. Rather than change that frozen contract, a provider
//! that supports incremental decoding also implements this capability
//! and the session takes it as a separate optional input.
//!
//! Contract:
//!
//! - [`StreamingSttProvider::accept`] receives boundary-normalized
//!   16 kHz mono f32 audio (see [`crate::pipeline::normalize`]) as it
//!   arrives, and returns the current hypothesis whenever it changed.
//!   Those updates are [`StreamingStage::Partial`].
//! - [`StreamingSttProvider::finalize`] closes the segment the VAD
//!   chunker just ended and returns its [`StreamingStage::Final`]
//!   update exactly once. A second `finalize` for the same segment
//!   returns `None`.
//! - The session never re-converts or re-transcribes finalized audio
//!   through [`SttProvider`]: the final update *is* the transcript
//!   chunk.
//!
//! [`SttProvider`]: crate::providers::SttProvider

use std::time::Duration;

use async_trait::async_trait;

use crate::error::SttError;
use crate::types::{AudioChunk, FrameSource, TokenTiming, TranscriptChunk};

/// Whether an update is a growing hypothesis or the closed segment.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StreamingStage {
    /// Hypothesis for a segment still receiving audio. Written to the
    /// crash-recovery WAL as a partial record; never rendered into
    /// `transcript.md`.
    Partial,
    /// Closed segment. Flows through the same diarization, transcript
    /// and hook path as a batch transcription.
    Final,
}

/// One streaming recognition update.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StreamingUpdate {
    pub stage: StreamingStage,
    pub chunk: TranscriptChunk,
}

/// Tier-2 capability for providers that decode incrementally.
///
/// Implementations: `SherpaStreamingProvider` behind
/// `--features stt-sherpa`.
#[async_trait]
pub trait StreamingSttProvider: Send + Sync {
    /// Feed normalized live audio for one source.
    ///
    /// `audio.start` is the offset of its first sample from session
    /// start; `audio.duration` covers `audio.samples`. Returns `Some`
    /// only when the hypothesis changed, so an unchanged stream writes
    /// nothing.
    ///
    /// # Errors
    ///
    /// `SttError::Decoding` when the recognizer rejects the audio or
    /// returns an inconsistent result, `SttError::ModelNotLoaded` when
    /// the provider was built without its backing feature.
    async fn accept(&self, audio: AudioChunk) -> Result<Option<StreamingUpdate>, SttError>;

    /// Close `source`'s current segment at the boundary the chunker
    /// chose, using the chunker's own `start` and `duration`.
    ///
    /// Returns `None` when the source has no open segment — including
    /// every call after the first for the same segment.
    ///
    /// # Errors
    ///
    /// As [`Self::accept`].
    async fn finalize(
        &self,
        source: FrameSource,
        start: Duration,
        duration: Duration,
    ) -> Result<Option<StreamingUpdate>, SttError>;
}

/// Convert a recognizer's stream-relative token timestamps into
/// session-absolute [`TokenTiming`] values.
///
/// `segment_start_ms` is the session offset at which the recognition
/// stream began; `timestamps` are seconds from that point.
///
/// # Errors
///
/// `SttError::Decoding` when the recognizer produced tokens without a
/// matching timestamp for each one. Persisted token timings are a
/// milestone requirement, so a cardinality mismatch is a loud failure
/// rather than a silently untimed transcript.
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
pub fn token_timings(
    tokens: &[String],
    timestamps: Option<&[f32]>,
    segment_start_ms: u64,
) -> Result<Vec<TokenTiming>, SttError> {
    let timestamps = match timestamps {
        Some(values) => values,
        None if tokens.is_empty() => return Ok(Vec::new()),
        None => {
            return Err(SttError::Decoding(Box::new(std::io::Error::other(
                format!(
                    "recognizer returned {} tokens with no timestamps",
                    tokens.len()
                ),
            ))))
        }
    };
    if timestamps.len() != tokens.len() {
        return Err(SttError::Decoding(Box::new(std::io::Error::other(
            format!(
                "recognizer returned {} tokens but {} timestamps",
                tokens.len(),
                timestamps.len()
            ),
        ))));
    }
    Ok(tokens
        .iter()
        .zip(timestamps)
        .map(|(token, seconds)| TokenTiming {
            token: token.clone(),
            timestamp_ms: segment_start_ms
                .saturating_add((f64::from(*seconds).max(0.0) * 1000.0).round() as u64),
        })
        .collect())
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    fn tokens(values: &[&str]) -> Vec<String> {
        values.iter().map(|v| (*v).to_string()).collect()
    }

    #[test]
    fn test_token_timings_offsets_stream_relative_seconds_to_session_absolute() {
        let timings = token_timings(
            &tokens(&["▁hel", "lo", "▁there"]),
            Some(&[0.08, 0.24, 0.6]),
            12_000,
        )
        .unwrap();

        assert_eq!(
            timings,
            vec![
                TokenTiming {
                    token: "▁hel".to_string(),
                    timestamp_ms: 12_080,
                },
                TokenTiming {
                    token: "lo".to_string(),
                    timestamp_ms: 12_240,
                },
                TokenTiming {
                    token: "▁there".to_string(),
                    timestamp_ms: 12_600,
                },
            ]
        );
    }

    #[test]
    fn test_token_timings_rejects_cardinality_mismatch() {
        let error = token_timings(&tokens(&["a", "b", "c"]), Some(&[0.1, 0.2]), 0)
            .unwrap_err()
            .to_string();

        assert!(
            error.contains("3 tokens but 2 timestamps"),
            "unexpected error: {error}"
        );
    }

    #[test]
    fn test_token_timings_rejects_tokens_without_timestamps() {
        let error = token_timings(&tokens(&["a"]), None, 0)
            .unwrap_err()
            .to_string();

        assert!(
            error.contains("1 tokens with no timestamps"),
            "unexpected error: {error}"
        );
    }

    #[test]
    fn test_token_timings_allows_empty_result_without_timestamps() {
        let timings = token_timings(&[], None, 5_000).unwrap();

        assert_eq!(timings, Vec::new());
    }
}
