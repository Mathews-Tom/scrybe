// Copyright 2026 Mathews Tom
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//     https://www.apache.org/licenses/LICENSE-2.0

//! `OpenAiCompatSttProvider` — speech-to-text via any endpoint speaking
//! the `/audio/transcriptions` contract.
//!
//! Compatible upstreams: Groq, `OpenAI`, Together, vLLM, … (`docs/system-design.md`
//! §4.3, §8.2). The provider delegates retry budget to
//! [`retry_with_policy`]; an HTTP 429 / 5xx / network-timeout response
//! is a transient retry, anything else (401, 400, malformed JSON)
//! short-circuits as a permanent failure. The resampler
//! (`pipeline::resample`) hands this provider an `f32` PCM chunk at
//! 16 kHz; we wrap the samples in a single-channel WAV in memory and
//! POST as `multipart/form-data` — the format every
//! `OpenAI`-compatible endpoint accepts.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use reqwest::multipart::{Form, Part};
use reqwest::{Client, ClientBuilder, StatusCode};
use serde::Deserialize;

use crate::error::SttError;
use crate::providers::retry::{retry_with_policy, RetryFailure, RetryOutcome, RetryPolicy};
use crate::providers::stt::SttProvider;
use crate::types::{AudioChunk, TranscriptChunk};

/// Configuration for [`OpenAiCompatSttProvider`].
#[derive(Clone, Debug)]
pub struct OpenAiCompatSttConfig {
    /// Base URL up to and including `/v1` (e.g. `https://api.groq.com/openai/v1`).
    /// The provider appends `/audio/transcriptions`.
    pub base_url: String,
    /// API key. Empty string disables the `Authorization` header (used
    /// by self-hosted vLLM / Ollama in OpenAI-compat mode).
    pub api_key: String,
    /// Model name passed in the multipart `model` field.
    pub model: String,
    /// `auto` for autodetection, otherwise an ISO-639-1 code.
    pub language: String,
    /// Provider-name override surfaced in `meta.toml`. Default is
    /// `"openai-compat"`; set to `"groq"`, `"together"`, etc., for
    /// stable identification in audit-log lines.
    pub display_name: String,
    /// Retry policy. Defaults to [`RetryPolicy::default`] (3 attempts,
    /// 500 ms initial, 8 s ceiling).
    pub retry: RetryPolicy,
    /// Per-request HTTP timeout including connect + body + read.
    /// Defaults to 60 s — Whisper-large transcription on Groq routinely
    /// approaches that ceiling for 30 s chunks.
    pub timeout: Duration,
}

impl Default for OpenAiCompatSttConfig {
    fn default() -> Self {
        Self {
            base_url: String::new(),
            api_key: String::new(),
            model: String::new(),
            language: "auto".to_string(),
            display_name: "openai-compat".to_string(),
            retry: RetryPolicy::default(),
            timeout: Duration::from_secs(60),
        }
    }
}

/// HTTP transport-and-retry wrapper around any OpenAI-compatible
/// `/audio/transcriptions` endpoint.
pub struct OpenAiCompatSttProvider {
    config: OpenAiCompatSttConfig,
    client: Client,
}

#[derive(Debug, Deserialize)]
struct TranscriptionResponse {
    text: String,
    #[serde(default)]
    language: Option<String>,
}

impl OpenAiCompatSttProvider {
    /// Construct from a config. Builds a single `reqwest::Client` so
    /// connection-pool reuse spans the session.
    ///
    /// # Errors
    ///
    /// `SttError::Transport` when `reqwest` cannot build a TLS-capable
    /// client (extremely rare; usually a misconfigured rustls platform
    /// verifier).
    pub fn new(config: OpenAiCompatSttConfig) -> Result<Self, SttError> {
        let client = ClientBuilder::new()
            .timeout(config.timeout)
            .build()
            .map_err(|e| SttError::Transport(Box::new(e)))?;
        Ok(Self { config, client })
    }

    fn endpoint(&self) -> String {
        let trimmed = self.config.base_url.trim_end_matches('/');
        format!("{trimmed}/audio/transcriptions")
    }

    async fn try_transcribe(&self, chunk: &AudioChunk) -> RetryOutcome<TranscriptChunk, SttError> {
        let wav = encode_pcm_to_wav(&chunk.samples);
        let mut form = Form::new()
            .text("model", self.config.model.clone())
            .text("response_format", "json");
        if self.config.language != "auto" {
            form = form.text("language", self.config.language.clone());
        }
        let part = match Part::bytes(wav)
            .file_name("audio.wav")
            .mime_str("audio/wav")
        {
            Ok(p) => p,
            Err(e) => return RetryOutcome::Permanent(SttError::Transport(Box::new(e))),
        };
        form = form.part("file", part);

        let mut request = self.client.post(self.endpoint()).multipart(form);
        if !self.config.api_key.is_empty() {
            request = request.bearer_auth(&self.config.api_key);
        }

        let response = match request.send().await {
            Ok(r) => r,
            Err(e) => {
                if e.is_timeout() || e.is_connect() {
                    return RetryOutcome::Transient(SttError::Transport(Box::new(e)));
                }
                return RetryOutcome::Permanent(SttError::Transport(Box::new(e)));
            }
        };

        let status = response.status();
        if status.is_success() {
            let body: TranscriptionResponse = match response.json().await {
                Ok(b) => b,
                Err(e) => return RetryOutcome::Permanent(SttError::Decoding(Box::new(e))),
            };
            return RetryOutcome::Ok(TranscriptChunk {
                text: body.text,
                source: chunk.source,
                start_ms: u64::try_from(chunk.start.as_millis()).unwrap_or(u64::MAX),
                duration_ms: u64::try_from(chunk.duration.as_millis()).unwrap_or(u64::MAX),
                language: body.language,
            });
        }

        if is_transient_status(status) {
            RetryOutcome::Transient(SttError::ProviderStatus {
                status: status.as_u16(),
            })
        } else {
            RetryOutcome::Permanent(SttError::ProviderStatus {
                status: status.as_u16(),
            })
        }
    }
}

const fn is_transient_status(status: StatusCode) -> bool {
    matches!(status.as_u16(), 408 | 425 | 429 | 500..=599)
}

#[async_trait]
impl SttProvider for OpenAiCompatSttProvider {
    async fn transcribe(&self, chunk: AudioChunk) -> Result<TranscriptChunk, SttError> {
        let chunk_for_op = Arc::new(chunk);
        match retry_with_policy(self.config.retry, |_attempt| {
            let chunk = Arc::clone(&chunk_for_op);
            let provider = self;
            async move { provider.try_transcribe(&chunk).await }
        })
        .await
        {
            Ok(t) => Ok(t),
            Err(RetryFailure {
                attempts,
                last_error,
                permanent,
            }) => {
                if permanent {
                    Err(last_error)
                } else {
                    Err(SttError::RetriesExhausted { attempts })
                }
            }
        }
    }

    fn name(&self) -> &str {
        &self.config.display_name
    }
}

/// Encode a `f32` PCM slice as a single-channel 16 kHz 16-bit-PCM WAV.
/// Whisper-compatible endpoints accept wider formats but mono 16 kHz
/// is universal.
#[allow(clippy::cast_possible_truncation)]
fn encode_pcm_to_wav(samples: &[f32]) -> Vec<u8> {
    const SAMPLE_RATE: u32 = 16_000;
    const CHANNELS: u16 = 1;
    const BITS_PER_SAMPLE: u16 = 16;

    let byte_rate = SAMPLE_RATE * u32::from(CHANNELS) * u32::from(BITS_PER_SAMPLE) / 8;
    let block_align = CHANNELS * BITS_PER_SAMPLE / 8;
    let data_len = u32::try_from(samples.len() * 2).unwrap_or(u32::MAX);
    let chunk_size = 36_u32.saturating_add(data_len);

    let mut out = Vec::with_capacity(44 + samples.len() * 2);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&chunk_size.to_le_bytes());
    out.extend_from_slice(b"WAVE");
    out.extend_from_slice(b"fmt ");
    out.extend_from_slice(&16_u32.to_le_bytes());
    out.extend_from_slice(&1_u16.to_le_bytes());
    out.extend_from_slice(&CHANNELS.to_le_bytes());
    out.extend_from_slice(&SAMPLE_RATE.to_le_bytes());
    out.extend_from_slice(&byte_rate.to_le_bytes());
    out.extend_from_slice(&block_align.to_le_bytes());
    out.extend_from_slice(&BITS_PER_SAMPLE.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&data_len.to_le_bytes());

    for sample in samples {
        let clamped = sample.clamp(-1.0, 1.0);
        let scaled = (clamped * f32::from(i16::MAX)) as i16;
        out.extend_from_slice(&scaled.to_le_bytes());
    }

    out
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use crate::types::FrameSource;
    use pretty_assertions::assert_eq;
    use wiremock::matchers::{header_exists, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn pcm_chunk() -> AudioChunk {
        AudioChunk {
            samples: Arc::from(vec![0.1_f32; 16_000]),
            source: FrameSource::Mic,
            start: Duration::from_secs(0),
            duration: Duration::from_secs(1),
        }
    }

    fn config_for(server: &MockServer) -> OpenAiCompatSttConfig {
        OpenAiCompatSttConfig {
            base_url: server.uri(),
            api_key: "test-key".into(),
            model: "whisper-large-v3".into(),
            display_name: "test-stt".into(),
            // Tight backoffs so the retry tests run quickly.
            retry: RetryPolicy {
                max_attempts: 3,
                initial_backoff_ms: 1,
                max_backoff_ms: 4,
            },
            timeout: Duration::from_secs(5),
            ..OpenAiCompatSttConfig::default()
        }
    }

    #[test]
    fn test_encode_pcm_to_wav_emits_riff_header_and_correct_data_length() {
        let samples = vec![0.0_f32; 16_000];

        let wav = encode_pcm_to_wav(&samples);

        assert_eq!(&wav[0..4], b"RIFF");
        assert_eq!(&wav[8..12], b"WAVE");
        let data_len = u32::from_le_bytes([wav[40], wav[41], wav[42], wav[43]]);
        assert_eq!(data_len as usize, samples.len() * 2);
        assert_eq!(wav.len(), 44 + samples.len() * 2);
    }

    #[tokio::test]
    async fn test_openai_compat_stt_returns_transcript_on_success() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/audio/transcriptions"))
            .and(header_exists("authorization"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "text": "Hi, thanks for taking the call.",
                "language": "en"
            })))
            .mount(&server)
            .await;

        let provider = OpenAiCompatSttProvider::new(config_for(&server)).unwrap();

        let result = provider.transcribe(pcm_chunk()).await.unwrap();

        assert_eq!(result.text, "Hi, thanks for taking the call.");
        assert_eq!(result.language.as_deref(), Some("en"));
        assert_eq!(result.source, FrameSource::Mic);
        assert_eq!(result.duration_ms, 1_000);
    }

    #[tokio::test]
    async fn test_openai_compat_stt_401_short_circuits_without_retries() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/audio/transcriptions"))
            .respond_with(ResponseTemplate::new(401))
            .expect(1)
            .mount(&server)
            .await;

        let provider = OpenAiCompatSttProvider::new(config_for(&server)).unwrap();

        let err = provider.transcribe(pcm_chunk()).await.unwrap_err();

        assert!(matches!(err, SttError::ProviderStatus { status: 401 }));
    }

    #[tokio::test]
    async fn test_openai_compat_stt_429_then_200_succeeds_after_retry() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/audio/transcriptions"))
            .respond_with(ResponseTemplate::new(429))
            .up_to_n_times(1)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/audio/transcriptions"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "text": "ok"
            })))
            .mount(&server)
            .await;

        let provider = OpenAiCompatSttProvider::new(config_for(&server)).unwrap();

        let result = provider.transcribe(pcm_chunk()).await.unwrap();

        assert_eq!(result.text, "ok");
    }

    #[tokio::test]
    async fn test_openai_compat_stt_500_exhausts_retry_budget() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/audio/transcriptions"))
            .respond_with(ResponseTemplate::new(503))
            .expect(3)
            .mount(&server)
            .await;

        let provider = OpenAiCompatSttProvider::new(config_for(&server)).unwrap();

        let err = provider.transcribe(pcm_chunk()).await.unwrap_err();

        assert!(matches!(err, SttError::RetriesExhausted { attempts: 3 }));
    }

    #[tokio::test]
    async fn test_openai_compat_stt_malformed_json_returns_decoding_error() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/audio/transcriptions"))
            .respond_with(ResponseTemplate::new(200).set_body_string("{not valid json"))
            .mount(&server)
            .await;

        let provider = OpenAiCompatSttProvider::new(config_for(&server)).unwrap();

        let err = provider.transcribe(pcm_chunk()).await.unwrap_err();

        assert!(matches!(err, SttError::Decoding(_)));
    }

    #[tokio::test]
    async fn test_openai_compat_stt_omits_authorization_when_api_key_empty() {
        // wiremock has no built-in 'header absent' matcher; we layer a
        // narrow "with auth" mock returning 500 in front of a permissive
        // 200 mock. If the request-under-test sends `Authorization`, the
        // narrow mock matches first and the test fails on a 500.
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/audio/transcriptions"))
            .and(header_exists("authorization"))
            .respond_with(ResponseTemplate::new(500))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/audio/transcriptions"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "text": "no auth header"
            })))
            .mount(&server)
            .await;

        let mut config = config_for(&server);
        config.api_key.clear();
        let provider = OpenAiCompatSttProvider::new(config).unwrap();

        let result = provider.transcribe(pcm_chunk()).await.unwrap();

        assert_eq!(result.text, "no auth header");
    }

    #[tokio::test]
    async fn test_openai_compat_stt_name_returns_configured_display_name() {
        let server = MockServer::start().await;
        let provider = OpenAiCompatSttProvider::new(config_for(&server)).unwrap();

        assert_eq!(provider.name(), "test-stt");
    }
}
