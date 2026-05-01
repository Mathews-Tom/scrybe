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
use crate::types::{AttributedChunk, Capabilities, FrameSource, SpeakerLabel, TranscriptChunk};

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
pub const fn requires_neural(capabilities: &Capabilities, ctx: &MeetingContext) -> bool {
    !capabilities.supports_system_audio || ctx.attendees.len() >= 3
}

/// v0.1-default `Diarizer`.
///
/// Mic-channel transcripts are attributed to `SpeakerLabel::Me`,
/// system-channel transcripts to `SpeakerLabel::Them`, and the merged
/// stream is sorted by `start_ms` so the renderer sees a single
/// timeline. Correct for 1:1 remote calls; falls back to a single
/// `Them` label per non-self speaker on multi-party calls — see
/// `requires_neural` for the auto-routing rule that hands those off to
/// the v0.5 neural fallback.
pub struct BinaryChannelDiarizer;

impl BinaryChannelDiarizer {
    /// Stable name surfaced in `meta.toml`'s `[providers]` table.
    pub const NAME: &'static str = "binary-channel";
}

#[async_trait]
impl Diarizer for BinaryChannelDiarizer {
    async fn diarize(
        &self,
        mic_chunks: &[TranscriptChunk],
        sys_chunks: &[TranscriptChunk],
        _ctx: &MeetingContext,
    ) -> Result<Vec<AttributedChunk>, CoreError> {
        let mut merged: Vec<AttributedChunk> =
            Vec::with_capacity(mic_chunks.len() + sys_chunks.len());

        for chunk in mic_chunks {
            merged.push(AttributedChunk {
                chunk: enforce_source(chunk.clone(), FrameSource::Mic),
                speaker: SpeakerLabel::Me,
            });
        }
        for chunk in sys_chunks {
            merged.push(AttributedChunk {
                chunk: enforce_source(chunk.clone(), FrameSource::System),
                speaker: SpeakerLabel::Them,
            });
        }

        merged.sort_by_key(|a| (a.chunk.start_ms, source_order(a.chunk.source)));
        Ok(merged)
    }

    fn name(&self) -> &str {
        Self::NAME
    }
}

fn enforce_source(chunk: TranscriptChunk, source: FrameSource) -> TranscriptChunk {
    TranscriptChunk { source, ..chunk }
}

const fn source_order(source: FrameSource) -> u8 {
    match source {
        FrameSource::Mic => 0,
        FrameSource::System => 1,
        FrameSource::Mixed => 2,
    }
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

    #[tokio::test]
    async fn test_binary_channel_diarizer_assigns_me_to_mic_and_them_to_system() {
        let result = BinaryChannelDiarizer
            .diarize(
                &[t("Hi.", FrameSource::Mic, 0)],
                &[t("Of course.", FrameSource::System, 1_000)],
                &MeetingContext::default(),
            )
            .await
            .unwrap();

        assert_eq!(result.len(), 2);
        assert_eq!(result[0].speaker, SpeakerLabel::Me);
        assert_eq!(result[0].chunk.text, "Hi.");
        assert_eq!(result[1].speaker, SpeakerLabel::Them);
        assert_eq!(result[1].chunk.text, "Of course.");
    }

    #[tokio::test]
    async fn test_binary_channel_diarizer_sorts_merged_chunks_by_start_ms() {
        let result = BinaryChannelDiarizer
            .diarize(
                &[
                    t("Mic late.", FrameSource::Mic, 5_000),
                    t("Mic early.", FrameSource::Mic, 1_000),
                ],
                &[
                    t("Sys early.", FrameSource::System, 0),
                    t("Sys mid.", FrameSource::System, 3_000),
                ],
                &MeetingContext::default(),
            )
            .await
            .unwrap();

        let texts: Vec<&str> = result.iter().map(|a| a.chunk.text.as_str()).collect();
        assert_eq!(
            texts,
            vec!["Sys early.", "Mic early.", "Sys mid.", "Mic late."]
        );
    }

    #[tokio::test]
    async fn test_binary_channel_diarizer_simultaneous_chunks_orders_mic_then_system() {
        let result = BinaryChannelDiarizer
            .diarize(
                &[t("Me here.", FrameSource::Mic, 2_000)],
                &[t("Them here.", FrameSource::System, 2_000)],
                &MeetingContext::default(),
            )
            .await
            .unwrap();

        assert_eq!(result.len(), 2);
        assert_eq!(result[0].speaker, SpeakerLabel::Me);
        assert_eq!(result[1].speaker, SpeakerLabel::Them);
    }

    #[tokio::test]
    async fn test_binary_channel_diarizer_overrides_source_field_on_misrouted_chunks() {
        let mic_with_wrong_source = TranscriptChunk {
            text: "Came in via mic stream.".into(),
            source: FrameSource::Mixed,
            start_ms: 0,
            duration_ms: 1_000,
            language: None,
        };

        let result = BinaryChannelDiarizer
            .diarize(&[mic_with_wrong_source], &[], &MeetingContext::default())
            .await
            .unwrap();

        assert_eq!(result[0].chunk.source, FrameSource::Mic);
        assert_eq!(result[0].speaker, SpeakerLabel::Me);
    }

    #[tokio::test]
    async fn test_binary_channel_diarizer_empty_inputs_yield_empty_output() {
        let result = BinaryChannelDiarizer
            .diarize(&[], &[], &MeetingContext::default())
            .await
            .unwrap();

        assert!(result.is_empty());
    }

    #[test]
    fn test_binary_channel_diarizer_name_matches_meta_toml_provider_string() {
        assert_eq!(BinaryChannelDiarizer.name(), "binary-channel");
    }
}
