// Copyright 2026 Mathews Tom
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//     https://www.apache.org/licenses/LICENSE-2.0

//! `Diarizer` — speaker-attribution strategy (`docs/system-design.md` §4.4).
//!
//! Tier-2 stability — the trait shape may evolve through v0.5 as the
//! neural fallback's input shape settles. Frozen at v1.0.
//!
//! Two implementations:
//!
//! - `BinaryChannelDiarizer` (v0.1 default) — energy-on-mic ⇒ `Me`,
//!   energy-on-system ⇒ `Them`, both ⇒ both. Correct for 1:1 remote calls.
//! - `PyannoteOnnxDiarizer` (v0.5, behind `--features diarize-pyannote`) —
//!   wraps `pyannote-onnx` 3.1+. Activated by config or auto-activated
//!   when `Capabilities::supports_system_audio == false` or when
//!   `MeetingContext.attendees.len() >= 3`.

use async_trait::async_trait;

use crate::context::MeetingContext;
use crate::error::CoreError;
use crate::types::{AttributedChunk, Capabilities, TranscriptChunk};

/// Speaker-attribution implementation. Pipeline calls `diarize` once
/// per session after all per-channel STT chunks are collected.
#[async_trait]
pub trait Diarizer: Send + Sync {
    /// Attribute each transcript segment to a speaker label.
    ///
    /// # Errors
    ///
    /// `CoreError::Pipeline` for compute-time failures (e.g. ONNX model
    /// not loaded); `CoreError::Storage` if the implementation reads
    /// embedding tables from disk and the read fails.
    async fn diarize(
        &self,
        mic_chunks: &[TranscriptChunk],
        sys_chunks: &[TranscriptChunk],
        ctx: &MeetingContext,
    ) -> Result<Vec<AttributedChunk>, CoreError>;

    /// Stable identifier surfaced in `meta.toml`'s `[providers]` table.
    fn name(&self) -> &str;
}

/// Decide whether the binary-channel heuristic is sufficient for the call.
///
/// Returns `true` when the pipeline must route through a neural
/// diarizer instead — multi-party remote calls (≥3 attendees) or
/// single-channel inputs (system audio unavailable).
#[must_use]
pub fn requires_neural(capabilities: &Capabilities, ctx: &MeetingContext) -> bool {
    !capabilities.supports_system_audio || ctx.attendees.len() >= 3
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::unnecessary_literal_bound
)]
mod tests {
    use super::*;
    use crate::types::{FrameSource, PermissionModel, SpeakerLabel};
    use pretty_assertions::assert_eq;

    fn caps_with_system_audio(supports: bool) -> Capabilities {
        Capabilities {
            supports_system_audio: supports,
            supports_per_app_capture: false,
            native_sample_rates: vec![48_000],
            permission_model: PermissionModel::CoreAudioTap,
        }
    }

    struct StubDiarizer;

    #[async_trait]
    impl Diarizer for StubDiarizer {
        async fn diarize(
            &self,
            mic_chunks: &[TranscriptChunk],
            sys_chunks: &[TranscriptChunk],
            _ctx: &MeetingContext,
        ) -> Result<Vec<AttributedChunk>, CoreError> {
            let mut out = Vec::new();
            for c in mic_chunks {
                out.push(AttributedChunk {
                    chunk: c.clone(),
                    speaker: SpeakerLabel::Me,
                });
            }
            for c in sys_chunks {
                out.push(AttributedChunk {
                    chunk: c.clone(),
                    speaker: SpeakerLabel::Them,
                });
            }
            Ok(out)
        }

        fn name(&self) -> &str {
            "stub"
        }
    }

    fn t(text: &str, source: FrameSource, start_ms: u64) -> TranscriptChunk {
        TranscriptChunk {
            text: text.into(),
            source,
            start_ms,
            duration_ms: 1_000,
            language: None,
        }
    }

    #[tokio::test]
    async fn test_stub_diarizer_assigns_me_to_mic_and_them_to_system() {
        let result = StubDiarizer
            .diarize(
                &[t("Hi.", FrameSource::Mic, 0)],
                &[t("Of course.", FrameSource::System, 1_000)],
                &MeetingContext::default(),
            )
            .await
            .unwrap();

        assert_eq!(result.len(), 2);
        assert_eq!(result[0].speaker, SpeakerLabel::Me);
        assert_eq!(result[1].speaker, SpeakerLabel::Them);
    }

    #[test]
    fn test_requires_neural_true_when_system_audio_unsupported() {
        let caps = caps_with_system_audio(false);
        let ctx = MeetingContext::default();

        assert!(requires_neural(&caps, &ctx));
    }

    #[test]
    fn test_requires_neural_true_when_attendees_three_or_more() {
        let caps = caps_with_system_audio(true);
        let ctx = MeetingContext {
            attendees: vec!["a".into(), "b".into(), "c".into()],
            ..MeetingContext::default()
        };

        assert!(requires_neural(&caps, &ctx));
    }

    #[test]
    fn test_requires_neural_false_for_one_on_one_remote_call() {
        let caps = caps_with_system_audio(true);
        let ctx = MeetingContext {
            attendees: vec!["a".into(), "b".into()],
            ..MeetingContext::default()
        };

        assert!(!requires_neural(&caps, &ctx));
    }

    #[test]
    fn test_requires_neural_false_for_solo_session() {
        let caps = caps_with_system_audio(true);
        let ctx = MeetingContext::default();

        assert!(!requires_neural(&caps, &ctx));
    }
}
