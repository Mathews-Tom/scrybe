// Copyright 2026 Mathews Tom
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//     https://www.apache.org/licenses/LICENSE-2.0

//! Configuration, as a read-only settings view sees it.
//!
//! A deliberate narrowing of the service layer's `ConfigForm`: the
//! settings view reads, it does not edit, so it is handed the fields it
//! displays and nothing else. No credential, no
//! credential environment-variable name, and no hook or webhook target
//! appears here.

use scrybe_application::config::ConfigSnapshot;
use scrybe_application::diagnostics::Severity;
use serde::Serialize;
use ts_rs::TS;

/// One problem the service layer found in the configuration.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, TS)]
pub struct SettingsWarning {
    pub severity: WarningSeverity,
    pub message: String,
}

/// Mirrors `scrybe_application::config::Severity` through an exhaustive
/// match.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum WarningSeverity {
    Info,
    Warning,
    Error,
}

impl From<Severity> for WarningSeverity {
    fn from(severity: Severity) -> Self {
        match severity {
            Severity::Info => Self::Info,
            Severity::Warning => Self::Warning,
            Severity::Error => Self::Error,
        }
    }
}

/// What the settings view renders.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, TS)]
pub struct SettingsSummary {
    /// Where the configuration file is, or would be written.
    pub config_path: String,
    /// `false` when no file exists yet and the values below are the
    /// built-in defaults.
    pub config_exists: bool,
    pub storage_root: String,
    pub capture_source: String,
    pub transcription_provider: String,
    pub transcription_model: String,
    pub notes_provider: String,
    pub notes_model: String,
    /// Whether the configured transcription or notes provider is a
    /// hosted one that needs a credential. The frontend renders the
    /// local/offline indicator from this; the credential itself never
    /// leaves Rust.
    pub hosted_credential_required: bool,
    pub warnings: Vec<SettingsWarning>,
}

impl From<ConfigSnapshot> for SettingsSummary {
    fn from(snapshot: ConfigSnapshot) -> Self {
        let form = snapshot.form;
        Self {
            config_path: snapshot.path,
            config_exists: snapshot.exists,
            storage_root: form.storage_root,
            capture_source: form.record_source,
            transcription_provider: form.stt_provider,
            transcription_model: form.stt_model,
            notes_provider: form.llm_provider,
            notes_model: form.llm_model,
            hosted_credential_required: form.stt_requires_hosted_credential
                || form.llm_requires_hosted_credential,
            warnings: snapshot
                .diagnostics
                .into_iter()
                .map(|diagnostic| SettingsWarning {
                    severity: diagnostic.severity.into(),
                    message: diagnostic.message,
                })
                .collect(),
        }
    }
}
