// Copyright 2026 Mathews Tom
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//     https://www.apache.org/licenses/LICENSE-2.0

//! Mandatory pre-capture courtesy step (`docs/system-design.md` §5).
//!
//! `Quick` is the floor: the step is enforced inside the library so
//! library consumers (Android shell, future GUI) cannot bypass it.
//! `Notify` and `Announce` modes are wired in v0.2 — at v0.1 the
//! library accepts those modes but downgrades to `Quick` with a
//! diagnostic until the chat-paste / TTS adapters land.
//!
//! The prompter trait is the seam tests use to drive consent through
//! `quick` without standard input. The CLI wires a TTY-backed prompter
//! in `scrybe-cli`; the Android shell will provide its own.

use async_trait::async_trait;
use chrono::Utc;

use crate::error::ConsentError;
use crate::types::{ConsentAttestation, ConsentMode};

/// User-facing prompter abstraction.
#[async_trait]
pub trait ConsentPrompter: Send + Sync {
    /// Display the courtesy notice and return `Ok(())` if the user
    /// authorizes capture, `Err(ConsentError::UserAborted)` if they
    /// decline, or other `ConsentError` variants for transport failures
    /// (e.g. terminal closed mid-prompt).
    async fn prompt(&self, mode: ConsentMode) -> Result<(), ConsentError>;
}

/// Run the consent step and produce an attestation suitable for
/// persisting to `meta.toml`.
///
/// `prompter` is dependency-injected so tests do not depend on stdin.
///
/// # Errors
///
/// Propagates the prompter's error verbatim. The session orchestrator
/// MUST treat any error as session-fatal — proceeding without a
/// recorded attestation violates the §5 contract.
pub async fn run<P: ConsentPrompter>(
    mode: ConsentMode,
    by_user: impl Into<String> + Send,
    prompter: &P,
) -> Result<ConsentAttestation, ConsentError> {
    let effective_mode = downgrade_unsupported(mode);
    prompter.prompt(effective_mode).await?;

    let attestation = ConsentAttestation {
        mode: effective_mode,
        attested_at: Utc::now(),
        by_user: by_user.into(),
        chat_message_sent: false,
        chat_message_target: None,
        tts_announce_played: false,
    };

    Ok(attestation)
}

/// Downgrade `Notify` and `Announce` to `Quick` until the v0.2 chat-paste
/// and TTS adapters land. The library is the right place to enforce this:
/// `scrybe-core` cannot inject keystrokes into Zoom or play TTS through
/// the user's mic device, so accepting the higher modes silently would
/// produce a recorded attestation that overstates what scrybe did.
const fn downgrade_unsupported(mode: ConsentMode) -> ConsentMode {
    match mode {
        ConsentMode::Quick | ConsentMode::Notify | ConsentMode::Announce => ConsentMode::Quick,
    }
}

/// Test-only prompter that always authorizes. Useful in pipeline tests
/// that exercise the orchestrator end-to-end.
#[cfg(test)]
pub struct AcceptingPrompter;

#[cfg(test)]
#[async_trait]
impl ConsentPrompter for AcceptingPrompter {
    async fn prompt(&self, _mode: ConsentMode) -> Result<(), ConsentError> {
        Ok(())
    }
}

/// Test-only prompter that always declines. Drives the
/// `ConsentError::UserAborted` path in tests without stdin.
#[cfg(test)]
pub struct AbortingPrompter;

#[cfg(test)]
#[async_trait]
impl ConsentPrompter for AbortingPrompter {
    async fn prompt(&self, _mode: ConsentMode) -> Result<(), ConsentError> {
        Err(ConsentError::UserAborted)
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    #[tokio::test]
    async fn test_run_quick_mode_records_attestation_when_user_accepts() {
        let attestation = run(ConsentMode::Quick, "tom", &AcceptingPrompter)
            .await
            .unwrap();

        assert_eq!(attestation.mode, ConsentMode::Quick);
        assert_eq!(attestation.by_user, "tom");
        assert!(!attestation.chat_message_sent);
        assert!(attestation.chat_message_target.is_none());
        assert!(!attestation.tts_announce_played);
    }

    #[tokio::test]
    async fn test_run_returns_user_aborted_when_prompter_declines() {
        let err = run(ConsentMode::Quick, "tom", &AbortingPrompter)
            .await
            .unwrap_err();

        assert!(matches!(err, ConsentError::UserAborted));
    }

    #[tokio::test]
    async fn test_run_notify_mode_downgrades_to_quick_until_v0_2_adapters() {
        let attestation = run(ConsentMode::Notify, "tom", &AcceptingPrompter)
            .await
            .unwrap();

        assert_eq!(
            attestation.mode,
            ConsentMode::Quick,
            "notify must downgrade to quick at v0.1; chat-paste lands in v0.2"
        );
    }

    #[tokio::test]
    async fn test_run_announce_mode_downgrades_to_quick_until_v0_2_adapters() {
        let attestation = run(ConsentMode::Announce, "tom", &AcceptingPrompter)
            .await
            .unwrap();

        assert_eq!(attestation.mode, ConsentMode::Quick);
    }

    #[tokio::test]
    async fn test_run_attestation_timestamp_is_close_to_now() {
        let before = Utc::now();
        let attestation = run(ConsentMode::Quick, "tom", &AcceptingPrompter)
            .await
            .unwrap();
        let after = Utc::now();

        assert!(attestation.attested_at >= before);
        assert!(attestation.attested_at <= after);
    }
}
