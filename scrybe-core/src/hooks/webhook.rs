// Copyright 2026 Mathews Tom
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//     https://www.apache.org/licenses/LICENSE-2.0

//! `Hook::Webhook` — async POST to a configured URL on terminal
//! lifecycle events, with HMAC-SHA256 body signing
//! (`docs/system-design.md` §4.5, `.docs/development-plan.md` §8.2).
//!
//! The hook intentionally only fires on `SessionEnd` and `NotesGenerated`;
//! per-chunk events would dwarf the meaningful events on a busy session
//! and webhook receivers care about completion, not progress. Failures
//! are surfaced to the dispatcher as `HookError::Hook`; the dispatcher
//! emits `LifecycleEvent::HookFailed` and `meta.toml` records the
//! disposition.

use std::time::Duration;

use async_trait::async_trait;
use hmac::{Hmac, Mac};
use reqwest::{Client, ClientBuilder};
use serde::Serialize;
use sha2::Sha256;

use super::{Hook, LifecycleEvent};
use crate::error::HookError;

/// Configuration for [`WebhookHook`].
#[derive(Clone, Debug)]
pub struct WebhookHookConfig {
    /// Endpoint URL the hook POSTs to.
    pub url: String,
    /// Optional HMAC-SHA256 secret. When `Some`, the hook adds an
    /// `X-Scrybe-Signature: sha256=<hex>` header computed over the
    /// JSON request body.
    pub secret: Option<String>,
    /// Per-request timeout. Defaults to 10 s.
    pub timeout: Duration,
    /// Stable hook identifier. Defaults to `"webhook"`.
    pub display_name: String,
}

impl Default for WebhookHookConfig {
    fn default() -> Self {
        Self {
            url: String::new(),
            secret: None,
            timeout: Duration::from_secs(10),
            display_name: "webhook".to_string(),
        }
    }
}

/// HTTP webhook hook. One POST per terminal lifecycle event.
pub struct WebhookHook {
    config: WebhookHookConfig,
    client: Client,
}

#[derive(Serialize)]
struct WebhookPayload<'a> {
    event: &'a str,
    session_id: String,
    notes_path: Option<String>,
    transcript_path: Option<String>,
    /// `Display`-rendered error chain on `SessionFailed`, omitted for
    /// success-path events. Receivers cannot reconstruct the original
    /// error type — the `LifecycleEvent::SessionFailed` variant carries
    /// `Arc<dyn Error + Send + Sync + 'static>` which has no JSON
    /// serialization — but the rendered string preserves the
    /// `#[source]` chain that the CLI also reports.
    error: Option<String>,
}

impl WebhookHook {
    /// Construct from a config.
    ///
    /// # Errors
    ///
    /// `HookError::Hook` if `reqwest` fails to build a TLS-capable
    /// client.
    pub fn new(config: WebhookHookConfig) -> Result<Self, HookError> {
        let client = ClientBuilder::new()
            .timeout(config.timeout)
            .build()
            .map_err(|e| HookError::Hook(Box::new(e)))?;
        Ok(Self { config, client })
    }

    fn payload_for(event: &LifecycleEvent) -> Option<WebhookPayload<'_>> {
        match event {
            LifecycleEvent::SessionEnd {
                id,
                transcript_path,
            } => Some(WebhookPayload {
                event: event.kind(),
                session_id: id.to_string_26(),
                notes_path: None,
                transcript_path: Some(transcript_path.display().to_string()),
                error: None,
            }),
            LifecycleEvent::NotesGenerated { id, notes_path } => Some(WebhookPayload {
                event: event.kind(),
                session_id: id.to_string_26(),
                notes_path: Some(notes_path.display().to_string()),
                transcript_path: None,
                error: None,
            }),
            // `SessionFailed` is the failure-path counterpart to
            // `SessionEnd`. Receivers configured for completion alerts
            // must see this; without it the webhook silently never
            // fires when the session crashed. Skipping `HookFailed` on
            // purpose — a webhook returning an error reentering
            // `dispatch_hooks` would loop.
            LifecycleEvent::SessionFailed { id, error } => Some(WebhookPayload {
                event: event.kind(),
                session_id: id.to_string_26(),
                notes_path: None,
                transcript_path: None,
                error: Some(format!("{error}")),
            }),
            _ => None,
        }
    }
}

#[async_trait]
impl Hook for WebhookHook {
    async fn on_event(&self, event: &LifecycleEvent) -> Result<(), HookError> {
        let Some(payload) = Self::payload_for(event) else {
            return Ok(());
        };

        let body = serde_json::to_vec(&payload).map_err(|e| HookError::Hook(Box::new(e)))?;

        let mut request = self
            .client
            .post(&self.config.url)
            .header("content-type", "application/json")
            .header("user-agent", concat!("scrybe/", env!("CARGO_PKG_VERSION")));
        if let Some(secret) = &self.config.secret {
            request = request.header("x-scrybe-signature", sign_body(secret, &body));
        }

        let response = request
            .body(body)
            .send()
            .await
            .map_err(|e| HookError::Hook(Box::new(e)))?;

        let status = response.status();
        if !status.is_success() {
            return Err(HookError::Hook(Box::new(WebhookStatusError {
                status: status.as_u16(),
            })));
        }
        Ok(())
    }

    fn name(&self) -> &str {
        &self.config.display_name
    }
}

#[derive(Debug, thiserror::Error)]
#[error("webhook returned non-success status {status}")]
struct WebhookStatusError {
    status: u16,
}

/// Compute the `sha256=<hex>` signature value used in the
/// `X-Scrybe-Signature` header.
///
/// Constant-time comparison on the receiving end (typically
/// `hmac.compare_digest` in Python or `crypto.timingSafeEqual` in
/// Node) keeps verification leak-free.
#[must_use]
pub fn sign_body(secret: &str, body: &[u8]) -> String {
    type HmacSha256 = Hmac<Sha256>;
    let mut mac = HmacSha256::new_from_slice(secret.as_bytes())
        .unwrap_or_else(|_| unreachable!("HmacSha256 accepts any-length keys"));
    mac.update(body);
    let digest = mac.finalize().into_bytes();
    format!("sha256={}", hex::encode(digest))
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use std::path::PathBuf;
    use std::sync::Arc;

    use super::*;
    use crate::context::MeetingContext;
    use crate::hooks::dispatch_hooks;
    use crate::types::SessionId;
    use pretty_assertions::assert_eq;
    use wiremock::matchers::{header, header_exists, method, path as wpath};
    use wiremock::{Mock, MockServer, Request, ResponseTemplate};

    fn config_for(server: &MockServer, secret: Option<&str>) -> WebhookHookConfig {
        WebhookHookConfig {
            url: format!("{}/scrybe/webhook", server.uri()),
            secret: secret.map(ToString::to_string),
            timeout: Duration::from_secs(2),
            display_name: "webhook".into(),
        }
    }

    #[test]
    fn test_sign_body_known_input_matches_hex_sha256() {
        // HMAC-SHA256("topsecret", "hello world"). Captured from the
        // first run; serves as a regression fixture so a future
        // hmac/sha2 bump that changes byte order surfaces immediately.
        let signed = sign_body("topsecret", b"hello world");

        assert_eq!(
            signed,
            "sha256=67a6479f7b6000f050577eea8b6b5e71d3c704e73a5f5d2aa09f607fce35cf1a"
        );
    }

    #[test]
    fn test_sign_body_empty_secret_still_produces_consistent_digest() {
        let a = sign_body("", b"abc");
        let b = sign_body("", b"abc");

        assert_eq!(a, b);
        assert!(a.starts_with("sha256="));
    }

    #[test]
    fn test_sign_body_different_secrets_produce_different_signatures() {
        let a = sign_body("alpha", b"payload");
        let b = sign_body("beta", b"payload");

        assert_ne!(a, b);
    }

    #[tokio::test]
    async fn test_webhook_hook_skips_non_terminal_events() {
        let server = MockServer::start().await;
        // No mock registered: any request would 404, which would fail
        // the test. Asserting the hook returns Ok on a non-terminal
        // event proves the early-out works.
        let hook = WebhookHook::new(config_for(&server, None)).unwrap();

        let event = LifecycleEvent::SessionStart {
            id: SessionId::new(),
            ctx: Arc::new(MeetingContext::default()),
        };

        hook.on_event(&event).await.unwrap();
    }

    #[tokio::test]
    async fn test_webhook_hook_posts_session_end_payload() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(wpath("/scrybe/webhook"))
            .and(header("content-type", "application/json"))
            .respond_with(ResponseTemplate::new(204))
            .mount(&server)
            .await;

        let hook = WebhookHook::new(config_for(&server, None)).unwrap();
        let id = SessionId::new();
        let event = LifecycleEvent::SessionEnd {
            id,
            transcript_path: PathBuf::from("/tmp/scrybe/transcript.md"),
        };

        hook.on_event(&event).await.unwrap();

        let received = server.received_requests().await.unwrap();
        assert_eq!(received.len(), 1);
        let body: serde_json::Value = serde_json::from_slice(&received[0].body).unwrap();
        assert_eq!(body["event"], "session_end");
        assert_eq!(body["session_id"], id.to_string_26());
        assert_eq!(body["transcript_path"], "/tmp/scrybe/transcript.md");
    }

    #[tokio::test]
    async fn test_webhook_hook_includes_hmac_signature_header_when_secret_configured() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(wpath("/scrybe/webhook"))
            .and(header_exists("x-scrybe-signature"))
            .respond_with(ResponseTemplate::new(200))
            .mount(&server)
            .await;

        let hook = WebhookHook::new(config_for(&server, Some("topsecret"))).unwrap();
        let event = LifecycleEvent::NotesGenerated {
            id: SessionId::new(),
            notes_path: PathBuf::from("/tmp/scrybe/notes.md"),
        };

        hook.on_event(&event).await.unwrap();

        let received = server.received_requests().await.unwrap();
        assert_eq!(received.len(), 1);
        let signature_header: &Request = &received[0];
        let header_value = signature_header
            .headers
            .get("x-scrybe-signature")
            .unwrap()
            .to_str()
            .unwrap();
        let expected = sign_body("topsecret", &signature_header.body);
        assert_eq!(header_value, expected);
    }

    #[tokio::test]
    async fn test_webhook_hook_non_success_status_propagates_as_hook_error() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(wpath("/scrybe/webhook"))
            .respond_with(ResponseTemplate::new(500))
            .mount(&server)
            .await;

        let hook = WebhookHook::new(config_for(&server, None)).unwrap();
        let event = LifecycleEvent::NotesGenerated {
            id: SessionId::new(),
            notes_path: PathBuf::from("/tmp/scrybe/notes.md"),
        };

        let err = hook.on_event(&event).await.unwrap_err();

        match err {
            HookError::Hook(_) => {}
            HookError::Timeout { .. } => panic!("expected HookError::Hook, got Timeout"),
        }
    }

    #[tokio::test]
    async fn test_webhook_hook_failure_surfaces_via_dispatcher_as_hook_failed_event() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(wpath("/scrybe/webhook"))
            .respond_with(ResponseTemplate::new(503))
            .mount(&server)
            .await;

        let hook: Box<dyn Hook> = Box::new(WebhookHook::new(config_for(&server, None)).unwrap());
        let id = SessionId::new();
        let event = LifecycleEvent::NotesGenerated {
            id,
            notes_path: PathBuf::from("/tmp/scrybe/notes.md"),
        };

        let outcome = dispatch_hooks(&[hook], &event).await;

        assert!(!outcome.all_ok());
        assert_eq!(outcome.failures.len(), 1);
        match &outcome.failures[0] {
            LifecycleEvent::HookFailed { hook_name, .. } => {
                assert_eq!(hook_name, "webhook");
            }
            other => panic!("expected HookFailed, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn test_webhook_hook_posts_session_failed_payload_with_error_chain() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(wpath("/scrybe/webhook"))
            .respond_with(ResponseTemplate::new(200))
            .mount(&server)
            .await;

        let hook = WebhookHook::new(config_for(&server, None)).unwrap();
        let id = SessionId::new();
        let event = LifecycleEvent::SessionFailed {
            id,
            error: Arc::new(std::io::Error::other("disk full at /var/scrybe")),
        };

        hook.on_event(&event).await.unwrap();

        let received = server.received_requests().await.unwrap();
        assert_eq!(received.len(), 1);
        let body: serde_json::Value = serde_json::from_slice(&received[0].body).unwrap();
        assert_eq!(body["event"], "session_failed");
        assert_eq!(body["session_id"], id.to_string_26());
        assert_eq!(body["error"], "disk full at /var/scrybe");
        assert!(body["transcript_path"].is_null());
        assert!(body["notes_path"].is_null());
    }

    #[tokio::test]
    async fn test_webhook_hook_skips_hook_failed_to_avoid_dispatcher_loop() {
        let server = MockServer::start().await;
        // No mock; any POST would fail. The hook must early-out on
        // HookFailed because emitting on it would let a webhook
        // failure re-enter dispatch_hooks via the synthetic
        // HookFailed event the dispatcher emits.
        let hook = WebhookHook::new(config_for(&server, None)).unwrap();
        let event = LifecycleEvent::HookFailed {
            id: SessionId::new(),
            hook_name: "another-hook".into(),
            error: Arc::new(std::io::Error::other("simulated")),
        };

        hook.on_event(&event).await.unwrap();
    }

    #[test]
    fn test_webhook_hook_name_returns_configured_display_name() {
        let config = WebhookHookConfig {
            display_name: "release-pipeline".into(),
            url: "http://localhost".into(),
            ..WebhookHookConfig::default()
        };
        let hook = WebhookHook::new(config).unwrap();

        assert_eq!(hook.name(), "release-pipeline");
    }
}
