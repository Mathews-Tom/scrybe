// Copyright 2026 Mathews Tom
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//     https://www.apache.org/licenses/LICENSE-2.0

//! Transcript and chunk types: audio chunks fed to STT, text chunks
//! returned, and the speaker-attributed chunks the renderer writes.

use std::sync::Arc;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use super::audio::FrameSource;

/// A 16-kHz, mono PCM chunk ready to hand to an `SttProvider`. The
/// chunker resamples and channel-splits before producing this.
#[derive(Clone, Debug)]
pub struct AudioChunk {
    /// Mono PCM at 16 kHz.
    pub samples: Arc<[f32]>,
    /// Channel of origin for binary diarization.
    pub source: FrameSource,
    /// Start of the chunk relative to session start.
    pub start: Duration,
    /// Duration covered by `samples`.
    pub duration: Duration,
}

/// Transcript text returned by an `SttProvider`. Speaker is unset until
/// the `Diarizer` populates it.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct TranscriptChunk {
    pub text: String,
    pub source: FrameSource,
    pub start_ms: u64,
    pub duration_ms: u64,
    pub language: Option<String>,
}

/// Coarse speaker label as seen on the transcript. Multi-party meetings
/// resolved by `PyannoteOnnxDiarizer` use `Named`; the binary-channel
/// diarizer only emits `Me` / `Them` / `Unknown`.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "value", rename_all = "lowercase")]
pub enum SpeakerLabel {
    Me,
    Them,
    Named(String),
    Unknown,
}

/// A transcript chunk after the `Diarizer` has assigned a speaker.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct AttributedChunk {
    pub chunk: TranscriptChunk,
    pub speaker: SpeakerLabel,
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    #[test]
    fn test_audio_chunk_holds_arc_samples_and_metadata() {
        let pcm: Arc<[f32]> = Arc::from(vec![0.0_f32; 16_000]);

        let chunk = AudioChunk {
            samples: pcm,
            source: FrameSource::Mic,
            start: Duration::from_secs(2),
            duration: Duration::from_secs(1),
        };

        assert_eq!(chunk.samples.len(), 16_000);
        assert_eq!(chunk.source, FrameSource::Mic);
        assert_eq!(chunk.start, Duration::from_secs(2));
        assert_eq!(chunk.duration, Duration::from_secs(1));
    }

    #[test]
    fn test_speaker_label_me_serializes_with_kind_tag() {
        let json = serde_json::to_string(&SpeakerLabel::Me).unwrap();

        assert_eq!(json, r#"{"kind":"me"}"#);
    }

    #[test]
    fn test_speaker_label_named_serializes_value_field() {
        let json = serde_json::to_string(&SpeakerLabel::Named("Alex".into())).unwrap();

        assert_eq!(json, r#"{"kind":"named","value":"Alex"}"#);
    }

    #[test]
    fn test_speaker_label_round_trips_through_json() {
        let inputs = vec![
            SpeakerLabel::Me,
            SpeakerLabel::Them,
            SpeakerLabel::Named("Alex".into()),
            SpeakerLabel::Unknown,
        ];

        for label in inputs {
            let encoded = serde_json::to_string(&label).unwrap();
            let decoded: SpeakerLabel = serde_json::from_str(&encoded).unwrap();
            assert_eq!(decoded, label);
        }
    }

    #[test]
    fn test_transcript_chunk_round_trips_through_json() {
        let original = TranscriptChunk {
            text: "Hi, thanks for taking the call.".into(),
            source: FrameSource::Mic,
            start_ms: 3_000,
            duration_ms: 1_800,
            language: Some("en".into()),
        };

        let encoded = serde_json::to_string(&original).unwrap();
        let decoded: TranscriptChunk = serde_json::from_str(&encoded).unwrap();

        assert_eq!(decoded, original);
    }

    #[test]
    fn test_attributed_chunk_carries_speaker_alongside_chunk() {
        let chunk = TranscriptChunk {
            text: "Of course, happy to.".into(),
            source: FrameSource::System,
            start_ms: 5_000,
            duration_ms: 1_200,
            language: None,
        };

        let attributed = AttributedChunk {
            chunk: chunk.clone(),
            speaker: SpeakerLabel::Them,
        };

        assert_eq!(attributed.speaker, SpeakerLabel::Them);
        assert_eq!(attributed.chunk, chunk);
    }
}
