// Copyright 2026 Mathews Tom
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//     https://www.apache.org/licenses/LICENSE-2.0

//! `SttProvider` — speech-to-text contract (`docs/system-design.md` §4.3).

use async_trait::async_trait;

use crate::error::SttError;
use crate::types::{AudioChunk, TranscriptChunk};

/// Tier-2 trait.
///
/// Implementations: `WhisperLocalProvider` (whisper.cpp via `whisper-rs`),
/// `OpenAiCompatSttProvider` (Groq, `OpenAI`, vLLM, `together.ai`), and an
/// optional `ParakeetLocalProvider` behind `--features parakeet-local`.
#[async_trait]
pub trait SttProvider: Send + Sync {
    /// Transcribe a single chunk. Implementations honor their own
    /// retry policy if applicable; pipeline code does not retry on
    /// behalf of providers.
    ///
    /// # Errors
    ///
    /// `SttError::ProviderStatus` for HTTP non-success, `SttError::Transport`
    /// for I/O, `SttError::Decoding` for response-shape mismatches,
    /// `SttError::RetriesExhausted` once the configured budget is spent.
    async fn transcribe(&self, chunk: AudioChunk) -> Result<TranscriptChunk, SttError>;

    /// Stable provider identifier surfaced in `meta.toml`'s
    /// `[providers]` table.
    fn name(&self) -> &str;
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::unnecessary_literal_bound
)]
mod tests {
    use std::sync::Arc;
    use std::time::Duration;

    use super::*;
    use crate::types::FrameSource;
    use pretty_assertions::assert_eq;

    struct EchoProvider {
        identifier: &'static str,
    }

    #[async_trait]
    impl SttProvider for EchoProvider {
        async fn transcribe(&self, chunk: AudioChunk) -> Result<TranscriptChunk, SttError> {
            Ok(TranscriptChunk {
                text: format!("samples={}", chunk.samples.len()),
                source: chunk.source,
                start_ms: u64::try_from(chunk.start.as_millis()).unwrap_or(u64::MAX),
                duration_ms: u64::try_from(chunk.duration.as_millis()).unwrap_or(u64::MAX),
                language: None,
            })
        }

        fn name(&self) -> &str {
            self.identifier
        }
    }

    #[tokio::test]
    async fn test_stt_provider_transcribe_returns_transcript_chunk() {
        let provider = EchoProvider { identifier: "echo" };
        let pcm: Arc<[f32]> = Arc::from(vec![0.0_f32; 16_000]);
        let chunk = AudioChunk {
            samples: pcm,
            source: FrameSource::Mic,
            start: Duration::from_secs(0),
            duration: Duration::from_secs(1),
        };

        let result = provider.transcribe(chunk).await.unwrap();

        assert_eq!(result.text, "samples=16000");
        assert_eq!(result.source, FrameSource::Mic);
        assert_eq!(result.start_ms, 0);
        assert_eq!(result.duration_ms, 1_000);
    }

    #[test]
    fn test_stt_provider_name_is_stable_across_calls() {
        let provider = EchoProvider { identifier: "echo" };

        assert_eq!(provider.name(), "echo");
        assert_eq!(provider.name(), "echo");
    }
}
