// Copyright 2026 Mathews Tom
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//     https://www.apache.org/licenses/LICENSE-2.0

//! `LlmProvider` — language-model contract (`docs/system-design.md` §4.3).

use async_trait::async_trait;

use crate::error::LlmError;

/// Tier-2 trait. Implementations: `OllamaLlmProvider`, `LmStudioLlmProvider`,
/// `OpenAiCompatLlmProvider`. The pipeline calls `complete` exactly once
/// per session, at `SessionEnd`, with the rendered prompt.
#[async_trait]
pub trait LlmProvider: Send + Sync {
    /// Render a single completion. Streaming responses are flattened to
    /// a single `String` by the implementation; the pipeline does not
    /// observe partial output.
    ///
    /// # Errors
    ///
    /// `LlmError::ProviderStatus` for HTTP non-success,
    /// `LlmError::Transport` for I/O, `LlmError::PromptRendering` if
    /// the implementation rejects the prompt shape,
    /// `LlmError::RetriesExhausted` once the configured budget is spent.
    async fn complete(&self, prompt: &str) -> Result<String, LlmError>;

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
    use super::*;
    use pretty_assertions::assert_eq;

    struct CannedProvider {
        response: &'static str,
    }

    #[async_trait]
    impl LlmProvider for CannedProvider {
        async fn complete(&self, _prompt: &str) -> Result<String, LlmError> {
            Ok(self.response.to_string())
        }

        fn name(&self) -> &str {
            "canned"
        }
    }

    #[tokio::test]
    async fn test_llm_provider_complete_returns_canned_response() {
        let provider = CannedProvider {
            response: "TL;DR: shipped.",
        };

        let out = provider.complete("notes prompt").await.unwrap();

        assert_eq!(out, "TL;DR: shipped.");
    }

    #[tokio::test]
    async fn test_llm_provider_propagates_provider_status_error() {
        struct FailingProvider;
        #[async_trait]
        impl LlmProvider for FailingProvider {
            async fn complete(&self, _prompt: &str) -> Result<String, LlmError> {
                Err(LlmError::ProviderStatus { status: 503 })
            }
            fn name(&self) -> &str {
                "failing"
            }
        }

        let err = FailingProvider.complete("anything").await.unwrap_err();

        assert!(matches!(err, LlmError::ProviderStatus { status: 503 }));
    }
}
