// Copyright 2026 Mathews Tom
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//     https://www.apache.org/licenses/LICENSE-2.0

//! `OpenAiCompatLlmProvider` — chat-completion via the
//! `/chat/completions` contract.
//!
//! Compatible upstreams: Groq, `OpenAI`, Together, local Ollama in
//! `OpenAI`-compat mode, vLLM, … (`docs/system-design.md` §4.3, §8.2).
//! The pipeline calls [`LlmProvider::complete`] exactly once per
//! session at `SessionEnd`. The provider sends a single user-role
//! message (`{ "role": "user", "content": <prompt> }`);
//! structured-output and tool-use are explicitly out of scope — the
//! prompt template is what shapes the response per
//! `docs/system-design.md` §6.

use std::time::Duration;

use async_trait::async_trait;
use reqwest::{Client, ClientBuilder, StatusCode};
use serde::Deserialize;

use crate::error::LlmError;
use crate::providers::llm::LlmProvider;
use crate::providers::retry::{retry_with_policy, RetryFailure, RetryOutcome, RetryPolicy};

/// Configuration for [`OpenAiCompatLlmProvider`].
#[derive(Clone, Debug)]
pub struct OpenAiCompatLlmConfig {
    /// Base URL up to and including `/v1` (e.g. `http://localhost:11434/v1`
    /// for Ollama's OpenAI-compat endpoint).
    pub base_url: String,
    /// API key. Empty disables the `Authorization` header (Ollama,
    /// self-hosted vLLM).
    pub api_key: String,
    /// Model identifier passed in the request body's `model` field.
    pub model: String,
    /// Display name surfaced in `meta.toml`'s `[providers]` table.
    pub display_name: String,
    /// Temperature passed through to the upstream.
    pub temperature: f32,
    /// Retry policy. Defaults to [`RetryPolicy::default`].
    pub retry: RetryPolicy,
    /// Per-request HTTP timeout. Defaults to 120 s — large summarization
    /// runs against a slow local model can approach this ceiling.
    pub timeout: Duration,
}

impl Default for OpenAiCompatLlmConfig {
    fn default() -> Self {
        Self {
            base_url: String::new(),
            api_key: String::new(),
            model: String::new(),
            display_name: "openai-compat".to_string(),
            temperature: 0.2,
            retry: RetryPolicy::default(),
            timeout: Duration::from_secs(120),
        }
    }
}

/// HTTP transport-and-retry wrapper around any OpenAI-compatible
/// `/chat/completions` endpoint.
pub struct OpenAiCompatLlmProvider {
    config: OpenAiCompatLlmConfig,
    client: Client,
}

#[derive(Debug, Deserialize)]
struct ChatCompletionResponse {
    choices: Vec<Choice>,
}

#[derive(Debug, Deserialize)]
struct Choice {
    message: Message,
}

#[derive(Debug, Deserialize)]
struct Message {
    content: String,
}

impl OpenAiCompatLlmProvider {
    /// Construct from a config.
    ///
    /// # Errors
    ///
    /// `LlmError::Transport` when `reqwest` cannot build the client.
    pub fn new(config: OpenAiCompatLlmConfig) -> Result<Self, LlmError> {
        let client = ClientBuilder::new()
            .timeout(config.timeout)
            .build()
            .map_err(|e| LlmError::Transport(Box::new(e)))?;
        Ok(Self { config, client })
    }

    fn endpoint(&self) -> String {
        let trimmed = self.config.base_url.trim_end_matches('/');
        format!("{trimmed}/chat/completions")
    }

    async fn try_complete(&self, prompt: &str) -> RetryOutcome<String, LlmError> {
        let body = serde_json::json!({
            "model": self.config.model,
            "temperature": self.config.temperature,
            "messages": [
                {"role": "user", "content": prompt}
            ],
        });

        let mut request = self.client.post(self.endpoint()).json(&body);
        if !self.config.api_key.is_empty() {
            request = request.bearer_auth(&self.config.api_key);
        }

        let response = match request.send().await {
            Ok(r) => r,
            Err(e) => {
                if e.is_timeout() || e.is_connect() {
                    return RetryOutcome::Transient(LlmError::Transport(Box::new(e)));
                }
                return RetryOutcome::Permanent(LlmError::Transport(Box::new(e)));
            }
        };

        let status = response.status();
        if status.is_success() {
            let parsed: ChatCompletionResponse = match response.json().await {
                Ok(b) => b,
                Err(e) => return RetryOutcome::Permanent(LlmError::Transport(Box::new(e))),
            };
            // Empty `choices` is an upstream-decoding failure (the
            // response shape is a 200 with no usable content) rather
            // than a prompt-rendering failure on our side.
            // `LlmError::PromptRendering` is documented for
            // "implementation rejects the prompt shape", so surface
            // the empty-choices case as a transport-class error
            // wrapping a synthetic `io::Error` until
            // `LlmError::Decoding` lands as a Tier-2 variant.
            return parsed
                .choices
                .into_iter()
                .next()
                .map(|c| c.message.content)
                .map_or_else(
                    || {
                        RetryOutcome::Permanent(LlmError::Transport(Box::new(
                            std::io::Error::other("upstream returned 200 with empty choices array"),
                        )))
                    },
                    RetryOutcome::Ok,
                );
        }

        if is_transient_status(status) {
            RetryOutcome::Transient(LlmError::ProviderStatus {
                status: status.as_u16(),
            })
        } else {
            RetryOutcome::Permanent(LlmError::ProviderStatus {
                status: status.as_u16(),
            })
        }
    }
}

const fn is_transient_status(status: StatusCode) -> bool {
    matches!(status.as_u16(), 408 | 425 | 429 | 500..=599)
}

#[async_trait]
impl LlmProvider for OpenAiCompatLlmProvider {
    async fn complete(&self, prompt: &str) -> Result<String, LlmError> {
        match retry_with_policy(self.config.retry, |_attempt| {
            let provider = self;
            async move { provider.try_complete(prompt).await }
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
                    Err(LlmError::RetriesExhausted { attempts })
                }
            }
        }
    }

    fn name(&self) -> &str {
        &self.config.display_name
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;
    use wiremock::matchers::{header_exists, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn config_for(server: &MockServer) -> OpenAiCompatLlmConfig {
        OpenAiCompatLlmConfig {
            base_url: server.uri(),
            api_key: "test-key".into(),
            model: "llama3.1:8b".into(),
            display_name: "test-llm".into(),
            retry: RetryPolicy {
                max_attempts: 3,
                initial_backoff_ms: 1,
                max_backoff_ms: 4,
            },
            timeout: Duration::from_secs(5),
            ..OpenAiCompatLlmConfig::default()
        }
    }

    fn ok_body(content: &str) -> serde_json::Value {
        serde_json::json!({
            "choices": [
                {"message": {"role": "assistant", "content": content}}
            ]
        })
    }

    #[tokio::test]
    async fn test_openai_compat_llm_returns_first_choice_content_on_success() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/chat/completions"))
            .and(header_exists("authorization"))
            .respond_with(ResponseTemplate::new(200).set_body_json(ok_body("TL;DR: shipped.")))
            .mount(&server)
            .await;

        let provider = OpenAiCompatLlmProvider::new(config_for(&server)).unwrap();

        let out = provider.complete("notes prompt").await.unwrap();

        assert_eq!(out, "TL;DR: shipped.");
    }

    #[tokio::test]
    async fn test_openai_compat_llm_401_short_circuits_without_retries() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/chat/completions"))
            .respond_with(ResponseTemplate::new(401))
            .expect(1)
            .mount(&server)
            .await;

        let provider = OpenAiCompatLlmProvider::new(config_for(&server)).unwrap();

        let err = provider.complete("p").await.unwrap_err();

        assert!(matches!(err, LlmError::ProviderStatus { status: 401 }));
    }

    #[tokio::test]
    async fn test_openai_compat_llm_429_then_200_succeeds_after_retry() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/chat/completions"))
            .respond_with(ResponseTemplate::new(429))
            .up_to_n_times(1)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/chat/completions"))
            .respond_with(ResponseTemplate::new(200).set_body_json(ok_body("ok")))
            .mount(&server)
            .await;

        let provider = OpenAiCompatLlmProvider::new(config_for(&server)).unwrap();

        let out = provider.complete("p").await.unwrap();

        assert_eq!(out, "ok");
    }

    #[tokio::test]
    async fn test_openai_compat_llm_503_exhausts_retry_budget() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/chat/completions"))
            .respond_with(ResponseTemplate::new(503))
            .expect(3)
            .mount(&server)
            .await;

        let provider = OpenAiCompatLlmProvider::new(config_for(&server)).unwrap();

        let err = provider.complete("p").await.unwrap_err();

        assert!(matches!(err, LlmError::RetriesExhausted { attempts: 3 }));
    }

    #[tokio::test]
    async fn test_openai_compat_llm_empty_choices_returns_transport_error() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/chat/completions"))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(serde_json::json!({"choices": []})),
            )
            .mount(&server)
            .await;

        let provider = OpenAiCompatLlmProvider::new(config_for(&server)).unwrap();

        let err = provider.complete("p").await.unwrap_err();

        match err {
            LlmError::Transport(source) => {
                assert!(
                    source.to_string().contains("empty choices"),
                    "expected empty-choices diagnostic in error chain: {source}"
                );
            }
            other => panic!("expected LlmError::Transport, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn test_openai_compat_llm_omits_authorization_when_api_key_empty() {
        let server = MockServer::start().await;
        // Same authorization-required-mock-fronted-by-permissive-200
        // pattern as the STT test. If the request sends Authorization,
        // the 500 mock matches first and the test fails.
        Mock::given(method("POST"))
            .and(path("/chat/completions"))
            .and(header_exists("authorization"))
            .respond_with(ResponseTemplate::new(500))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/chat/completions"))
            .respond_with(ResponseTemplate::new(200).set_body_json(ok_body("no auth")))
            .mount(&server)
            .await;

        let mut config = config_for(&server);
        config.api_key.clear();
        let provider = OpenAiCompatLlmProvider::new(config).unwrap();

        let out = provider.complete("p").await.unwrap();

        assert_eq!(out, "no auth");
    }

    #[tokio::test]
    async fn test_openai_compat_llm_name_returns_configured_display_name() {
        let server = MockServer::start().await;
        let provider = OpenAiCompatLlmProvider::new(config_for(&server)).unwrap();

        assert_eq!(provider.name(), "test-llm");
    }
}
