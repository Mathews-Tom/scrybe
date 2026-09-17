// Copyright 2026 Mathews Tom
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//     https://www.apache.org/licenses/LICENSE-2.0

//! The one error type every application service returns.
//!
//! A failure carries two things with different audiences. [`ErrorCode`]
//! is the stable, serializable discriminant a frontend branches on; it
//! never changes meaning once shipped. The originating error is retained
//! as a private [`std::error::Error`] source for logs and `--verbose`
//! CLI output, and is deliberately absent from [`ErrorPayload`], the
//! wire form — so a provider URL, filesystem path, or parser excerpt
//! cannot reach a frontend through an error channel.

use std::error::Error as StdError;
use std::fmt;

use serde::{Deserialize, Serialize};

use crate::identity::IdentityRejection;

/// Stable discriminant for an application-service failure.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    /// A supplied session identity broke a confinement rule.
    InvalidSessionId,
    /// No session under the configured root matches the identity.
    SessionNotFound,
    /// The identity is a fragment matching more than one session.
    AmbiguousSessionId,
    /// The configured storage root does not exist.
    StorageRootMissing,
    /// The storage root or a session folder could not be read.
    StorageUnavailable,
    /// A session's durable metadata exists but could not be parsed.
    MetadataUnreadable,
    /// The caller cancelled the operation before it completed.
    Cancelled,
    /// The operation does not apply to the session in its current state.
    NotApplicable,
    /// The configuration file exists but could not be read or parsed.
    ConfigUnreadable,
    /// The candidate configuration is not a valid complete document.
    ConfigInvalid,
    /// A validated configuration could not be durably replaced.
    ConfigWriteFailed,
    /// A diagnostic probe could not be completed.
    DiagnosticsUnavailable,
    /// An explicitly requested repair failed.
    RepairFailed,
    /// Notes regeneration failed in the caller-supplied generator.
    NotesGenerationFailed,
    /// A recording transition was requested from a state that forbids it.
    RecordingStateConflict,
    /// Recording preflight failed; no session was created.
    PreflightFailed,
    /// A provider the plan names could not be built. Distinct from
    /// [`Self::PreflightFailed`]: preflight refuses before anything is
    /// written, and this is a provider that passed every check and then
    /// failed to load.
    ProviderUnavailable,
    /// A capture device could not be opened.
    CaptureUnavailable,
    /// A recording that started did not complete. Durable state may
    /// exist under the session folder and may be repairable, which is
    /// what distinguishes it from a preflight refusal.
    RecordingFailed,
    /// The checked-in model catalog does not describe a usable model.
    ModelManifestInvalid,
    /// No catalog entry carries the requested identity.
    ModelUnknown,
    /// A download was requested without the user's explicit
    /// confirmation of the artifact they would be fetching.
    ModelConfirmationRequired,
    /// The models directory could not be read, created, or measured.
    ModelStorageUnavailable,
    /// This build carries no model transport, so nothing can be
    /// fetched through it.
    ModelDownloadUnavailable,
}

impl ErrorCode {
    /// The code's stable wire spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::InvalidSessionId => "invalid_session_id",
            Self::SessionNotFound => "session_not_found",
            Self::AmbiguousSessionId => "ambiguous_session_id",
            Self::StorageRootMissing => "storage_root_missing",
            Self::StorageUnavailable => "storage_unavailable",
            Self::MetadataUnreadable => "metadata_unreadable",
            Self::Cancelled => "cancelled",
            Self::NotApplicable => "not_applicable",
            Self::ConfigUnreadable => "config_unreadable",
            Self::ConfigInvalid => "config_invalid",
            Self::ConfigWriteFailed => "config_write_failed",
            Self::DiagnosticsUnavailable => "diagnostics_unavailable",
            Self::RepairFailed => "repair_failed",
            Self::NotesGenerationFailed => "notes_generation_failed",
            Self::RecordingStateConflict => "recording_state_conflict",
            Self::PreflightFailed => "preflight_failed",
            Self::ProviderUnavailable => "provider_unavailable",
            Self::CaptureUnavailable => "capture_unavailable",
            Self::RecordingFailed => "recording_failed",
            Self::ModelManifestInvalid => "model_manifest_invalid",
            Self::ModelUnknown => "model_unknown",
            Self::ModelConfirmationRequired => "model_confirmation_required",
            Self::ModelStorageUnavailable => "model_storage_unavailable",
            Self::ModelDownloadUnavailable => "model_download_unavailable",
        }
    }
}

impl fmt::Display for ErrorCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// The serializable projection of an [`ApplicationError`].
///
/// Deliberately has no source, path, or cause field: everything a
/// frontend receives is a stable code plus a message the service layer
/// wrote itself.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ErrorPayload {
    pub code: ErrorCode,
    pub message: String,
}

/// A failure from any application service.
#[derive(Debug)]
pub struct ApplicationError {
    code: ErrorCode,
    message: String,
    source: Option<Box<dyn StdError + Send + Sync>>,
}

impl ApplicationError {
    /// A failure with no underlying cause.
    #[must_use]
    pub fn new(code: ErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            source: None,
        }
    }

    /// Attaches the originating error. It stays internal: logs and
    /// `Error::source` see it, [`Self::payload`] does not.
    #[must_use]
    pub fn with_source(mut self, source: impl StdError + Send + Sync + 'static) -> Self {
        self.source = Some(Box::new(source));
        self
    }

    /// The stable discriminant.
    #[must_use]
    pub const fn code(&self) -> ErrorCode {
        self.code
    }

    /// The user-safe message.
    #[must_use]
    pub fn message(&self) -> &str {
        &self.message
    }

    /// The wire form handed to a frontend.
    #[must_use]
    pub fn payload(&self) -> ErrorPayload {
        ErrorPayload {
            code: self.code,
            message: self.message.clone(),
        }
    }
}

impl fmt::Display for ApplicationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}

impl StdError for ApplicationError {
    fn source(&self) -> Option<&(dyn StdError + 'static)> {
        self.source
            .as_ref()
            .map(|boxed| boxed.as_ref() as &(dyn StdError + 'static))
    }
}

impl From<IdentityRejection> for ApplicationError {
    fn from(value: IdentityRejection) -> Self {
        Self::new(ErrorCode::InvalidSessionId, value.reason()).with_source(value)
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    #[test]
    fn test_payload_omits_the_internal_source_error() {
        let error = ApplicationError::new(ErrorCode::StorageUnavailable, "storage is unavailable")
            .with_source(std::io::Error::other("/private/var/secret-root denied"));

        let payload = error.payload();
        let encoded = serde_json::to_string(&payload).unwrap();

        assert!(error.source().is_some());
        assert!(!encoded.contains("secret-root"));
        assert_eq!(
            encoded,
            r#"{"code":"storage_unavailable","message":"storage is unavailable"}"#
        );
    }

    #[test]
    fn test_rejected_identity_maps_to_the_invalid_session_id_code() {
        let error = ApplicationError::from(IdentityRejection::Traversal);

        assert_eq!(error.code(), ErrorCode::InvalidSessionId);
    }

    #[test]
    fn test_every_code_serializes_to_its_documented_wire_spelling() {
        for code in [
            ErrorCode::InvalidSessionId,
            ErrorCode::SessionNotFound,
            ErrorCode::AmbiguousSessionId,
            ErrorCode::StorageRootMissing,
            ErrorCode::StorageUnavailable,
            ErrorCode::MetadataUnreadable,
            ErrorCode::Cancelled,
            ErrorCode::NotApplicable,
            ErrorCode::ConfigUnreadable,
            ErrorCode::ConfigInvalid,
            ErrorCode::ConfigWriteFailed,
            ErrorCode::DiagnosticsUnavailable,
            ErrorCode::RepairFailed,
            ErrorCode::NotesGenerationFailed,
            ErrorCode::RecordingStateConflict,
            ErrorCode::PreflightFailed,
            ErrorCode::ProviderUnavailable,
            ErrorCode::CaptureUnavailable,
            ErrorCode::RecordingFailed,
        ] {
            let encoded = serde_json::to_string(&code).unwrap();

            assert_eq!(encoded, format!("\"{}\"", code.as_str()));
        }
    }
}
