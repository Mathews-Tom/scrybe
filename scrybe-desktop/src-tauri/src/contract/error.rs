// Copyright 2026 Mathews Tom
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//     https://www.apache.org/licenses/LICENSE-2.0

//! What a failed command returns.

use scrybe_application::{ApplicationError, ErrorCode, IdentityRejection};
use serde::Serialize;
use ts_rs::TS;

/// The wire spelling of a service failure.
///
/// Mirrors `scrybe_application::ErrorCode`. The conversion below is an
/// exhaustive match, so a new variant in the service layer breaks this
/// build rather than silently producing a frontend union that is
/// missing a case.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum FailureCode {
    InvalidSessionId,
    SessionNotFound,
    AmbiguousSessionId,
    StorageRootMissing,
    StorageUnavailable,
    MetadataUnreadable,
    Cancelled,
    NotApplicable,
    ConfigUnreadable,
    ConfigInvalid,
    ConfigWriteFailed,
    DiagnosticsUnavailable,
    RepairFailed,
    NotesGenerationFailed,
    RecordingStateConflict,
    PreflightFailed,
    ModelManifestInvalid,
    ModelUnknown,
    ModelConfirmationRequired,
    ModelStorageUnavailable,
    ModelDownloadUnavailable,
}

impl From<ErrorCode> for FailureCode {
    fn from(code: ErrorCode) -> Self {
        match code {
            ErrorCode::InvalidSessionId => Self::InvalidSessionId,
            ErrorCode::SessionNotFound => Self::SessionNotFound,
            ErrorCode::AmbiguousSessionId => Self::AmbiguousSessionId,
            ErrorCode::StorageRootMissing => Self::StorageRootMissing,
            ErrorCode::StorageUnavailable => Self::StorageUnavailable,
            ErrorCode::MetadataUnreadable => Self::MetadataUnreadable,
            ErrorCode::Cancelled => Self::Cancelled,
            ErrorCode::NotApplicable => Self::NotApplicable,
            ErrorCode::ConfigUnreadable => Self::ConfigUnreadable,
            ErrorCode::ConfigInvalid => Self::ConfigInvalid,
            ErrorCode::ConfigWriteFailed => Self::ConfigWriteFailed,
            ErrorCode::DiagnosticsUnavailable => Self::DiagnosticsUnavailable,
            ErrorCode::RepairFailed => Self::RepairFailed,
            ErrorCode::NotesGenerationFailed => Self::NotesGenerationFailed,
            ErrorCode::RecordingStateConflict => Self::RecordingStateConflict,
            ErrorCode::PreflightFailed => Self::PreflightFailed,
            ErrorCode::ModelManifestInvalid => Self::ModelManifestInvalid,
            ErrorCode::ModelUnknown => Self::ModelUnknown,
            ErrorCode::ModelConfirmationRequired => Self::ModelConfirmationRequired,
            ErrorCode::ModelStorageUnavailable => Self::ModelStorageUnavailable,
            ErrorCode::ModelDownloadUnavailable => Self::ModelDownloadUnavailable,
        }
    }
}

/// Everything the frontend learns about a failure.
///
/// Built from `ApplicationError::payload`, which is already proven to
/// drop the underlying source. The host adds no field of its own, so
/// there is no second place a path or a cause could leak from.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, TS)]
pub struct CommandFailure {
    pub code: FailureCode,
    pub message: String,
}

impl From<ApplicationError> for CommandFailure {
    fn from(error: ApplicationError) -> Self {
        let payload = error.payload();
        Self {
            code: payload.code.into(),
            message: payload.message,
        }
    }
}

impl From<IdentityRejection> for CommandFailure {
    fn from(rejection: IdentityRejection) -> Self {
        ApplicationError::from(rejection).into()
    }
}
