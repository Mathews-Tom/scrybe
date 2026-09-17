// Copyright 2026 Mathews Tom
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//     https://www.apache.org/licenses/LICENSE-2.0

//! Configuration, diagnosis, readiness, and model acquisition, as the
//! setup surface sees them.
//!
//! Narrowed the same way the rest of this module narrows: the service
//! layer's types are shared with the command-line tool and the agent
//! surface, and a `WebView` is handed the subset one screen renders.
//!
//! Two narrowings here are load-bearing rather than cosmetic. The
//! configuration form carries no credential and no credential
//! environment-variable name — only a boolean saying one is needed —
//! and an update names a field from a closed enumeration rather than a
//! TOML key, so a frontend cannot address a key the service layer does
//! not own. And a model plan carries the source, revision, licence,
//! exact size, digest, destination, and disk requirement together,
//! because those are exactly what must be on screen before the
//! confirmation that permits a request.

use scrybe_application::config::{ConfigField, ConfigForm, ConfigSnapshot, ConfigValueKind};
use scrybe_application::diagnostics::{
    Capability, DiagnosticFinding, DiagnosticReport, Facet, FacetState, Readiness, RecoveryAction,
    RepairApplication,
};
use scrybe_application::models::{InstallReport, ModelFailure, ModelPlan, ModelState};
use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// One field a settings form may write.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum SettingsField {
    StorageRoot,
    StorageAudioBitrateKbps,
    CaptureMicDevice,
    CaptureHotkey,
    RecordSource,
    RecordSystemBackend,
    RecordLlm,
    SttProvider,
    SttModel,
    SttLanguage,
    LlmProvider,
    LlmBaseUrl,
    LlmModel,
    ConsentDefaultMode,
    ShellIndicators,
    AgentAccessEnabled,
}

/// Mirrors `scrybe_application::config::ConfigField` through an
/// exhaustive match in both directions, so a field added to the service
/// layer's closed write surface breaks this build rather than becoming
/// silently unreachable — or, worse, silently reachable under a name
/// the frontend guessed.
impl From<ConfigField> for SettingsField {
    fn from(field: ConfigField) -> Self {
        match field {
            ConfigField::StorageRoot => Self::StorageRoot,
            ConfigField::StorageAudioBitrateKbps => Self::StorageAudioBitrateKbps,
            ConfigField::CaptureMicDevice => Self::CaptureMicDevice,
            ConfigField::CaptureHotkey => Self::CaptureHotkey,
            ConfigField::RecordSource => Self::RecordSource,
            ConfigField::RecordSystemBackend => Self::RecordSystemBackend,
            ConfigField::RecordLlm => Self::RecordLlm,
            ConfigField::SttProvider => Self::SttProvider,
            ConfigField::SttModel => Self::SttModel,
            ConfigField::SttLanguage => Self::SttLanguage,
            ConfigField::LlmProvider => Self::LlmProvider,
            ConfigField::LlmBaseUrl => Self::LlmBaseUrl,
            ConfigField::LlmModel => Self::LlmModel,
            ConfigField::ConsentDefaultMode => Self::ConsentDefaultMode,
            ConfigField::ShellIndicators => Self::ShellIndicators,
            ConfigField::AgentAccessEnabled => Self::AgentAccessEnabled,
        }
    }
}

impl From<SettingsField> for ConfigField {
    fn from(field: SettingsField) -> Self {
        match field {
            SettingsField::StorageRoot => Self::StorageRoot,
            SettingsField::StorageAudioBitrateKbps => Self::StorageAudioBitrateKbps,
            SettingsField::CaptureMicDevice => Self::CaptureMicDevice,
            SettingsField::CaptureHotkey => Self::CaptureHotkey,
            SettingsField::RecordSource => Self::RecordSource,
            SettingsField::RecordSystemBackend => Self::RecordSystemBackend,
            SettingsField::RecordLlm => Self::RecordLlm,
            SettingsField::SttProvider => Self::SttProvider,
            SettingsField::SttModel => Self::SttModel,
            SettingsField::SttLanguage => Self::SttLanguage,
            SettingsField::LlmProvider => Self::LlmProvider,
            SettingsField::LlmBaseUrl => Self::LlmBaseUrl,
            SettingsField::LlmModel => Self::LlmModel,
            SettingsField::ConsentDefaultMode => Self::ConsentDefaultMode,
            SettingsField::ShellIndicators => Self::ShellIndicators,
            SettingsField::AgentAccessEnabled => Self::AgentAccessEnabled,
        }
    }
}

/// What kind of control renders one field.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum SettingsFieldKind {
    Text,
    Integer,
    Boolean,
    TextList,
}

impl From<ConfigValueKind> for SettingsFieldKind {
    fn from(kind: ConfigValueKind) -> Self {
        match kind {
            ConfigValueKind::Text => Self::Text,
            ConfigValueKind::Integer => Self::Integer,
            ConfigValueKind::Boolean => Self::Boolean,
            ConfigValueKind::TextList => Self::TextList,
        }
    }
}

/// One editable field, and the control that edits it.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, TS)]
pub struct SettingsFieldSpec {
    pub field: SettingsField,
    pub kind: SettingsFieldKind,
}

/// A value the frontend is proposing for one field.
///
/// Untagged rather than discriminated because the four shapes are
/// already distinguishable in JSON, and a settings control that had to
/// name the kind alongside the value could name the wrong one. The
/// service layer checks the kind against the field regardless.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, TS)]
#[serde(untagged)]
pub enum SettingsValue {
    Boolean(bool),
    Integer(i64),
    Text(String),
    TextList(Vec<String>),
}

impl From<SettingsValue> for scrybe_application::config::ConfigValue {
    fn from(value: SettingsValue) -> Self {
        match value {
            SettingsValue::Boolean(flag) => Self::Boolean(flag),
            SettingsValue::Integer(number) => Self::Integer(number),
            SettingsValue::Text(text) => Self::Text(text),
            SettingsValue::TextList(items) => Self::TextList(items),
        }
    }
}

/// One proposed change, as it crosses the boundary.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, TS)]
pub struct SettingsChange {
    pub field: SettingsField,
    pub value: SettingsValue,
}

/// Everything a settings form renders, plus what is editable.
///
/// No credential and no credential environment-variable name appears.
/// `hosted_credential_required` is the whole of what the frontend
/// learns about one.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, TS)]
pub struct SettingsForm {
    pub config_path: String,
    pub config_exists: bool,
    pub schema_version: u32,
    pub storage_root: String,
    pub storage_audio_bitrate_kbps: u32,
    pub capture_mic_device: String,
    pub capture_hotkey: Option<String>,
    pub record_source: String,
    pub record_system_backend: String,
    pub record_llm: String,
    pub stt_provider: String,
    pub stt_model: String,
    pub stt_language: String,
    pub llm_provider: String,
    pub llm_base_url: String,
    pub llm_model: String,
    pub consent_default_mode: String,
    pub shell_indicators: Vec<String>,
    pub agent_access_enabled: bool,
    pub hosted_credential_required: bool,
    /// Every field this form may write, in the service layer's stable
    /// order, with the control kind each one takes.
    pub editable: Vec<SettingsFieldSpec>,
    pub warnings: Vec<super::SettingsWarning>,
}

impl From<ConfigSnapshot> for SettingsForm {
    fn from(snapshot: ConfigSnapshot) -> Self {
        let warnings = snapshot
            .diagnostics
            .into_iter()
            .map(|diagnostic| super::SettingsWarning {
                severity: diagnostic.severity.into(),
                message: diagnostic.message,
            })
            .collect();
        let form: ConfigForm = snapshot.form;
        Self {
            config_path: snapshot.path,
            config_exists: snapshot.exists,
            schema_version: form.schema_version,
            storage_root: form.storage_root,
            storage_audio_bitrate_kbps: form.storage_audio_bitrate_kbps,
            capture_mic_device: form.capture_mic_device,
            capture_hotkey: form.capture_hotkey,
            record_source: form.record_source,
            record_system_backend: form.record_system_backend,
            record_llm: form.record_llm,
            stt_provider: form.stt_provider,
            stt_model: form.stt_model,
            stt_language: form.stt_language,
            llm_provider: form.llm_provider,
            llm_base_url: form.llm_base_url,
            llm_model: form.llm_model,
            consent_default_mode: form.consent_default_mode,
            shell_indicators: form.shell_indicators,
            agent_access_enabled: form.agent_access_enabled,
            hosted_credential_required: form.stt_requires_hosted_credential
                || form.llm_requires_hosted_credential,
            editable: scrybe_application::config::EDITABLE_FIELDS
                .iter()
                .map(|field| SettingsFieldSpec {
                    field: (*field).into(),
                    kind: field.kind().into(),
                })
                .collect(),
            warnings,
        }
    }
}

/// How one facet of the install stands.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum ReadinessState {
    Ready,
    Blocked,
    NotConfigured,
    /// Nothing in the service layer checked this one. Mirrored rather
    /// than folded into `NotConfigured`, which means something else
    /// entirely — that the facet is deliberately not in use — and the
    /// difference is what a reader needs to tell "nothing is wrong"
    /// from "nobody looked".
    Unverified,
}

impl From<FacetState> for ReadinessState {
    fn from(state: FacetState) -> Self {
        match state {
            FacetState::Ready => Self::Ready,
            FacetState::Blocked => Self::Blocked,
            FacetState::NotConfigured => Self::NotConfigured,
            FacetState::Unverified => Self::Unverified,
        }
    }
}

/// One facet, with the line a user reads.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, TS)]
pub struct ReadinessFacet {
    pub state: ReadinessState,
    pub summary: String,
}

impl From<&Facet> for ReadinessFacet {
    fn from(facet: &Facet) -> Self {
        Self {
            state: facet.state.into(),
            summary: facet.summary.clone(),
        }
    }
}

/// The five answers a setup screen reports separately, and whether they
/// permit a recording.
///
/// `can_record` is carried rather than recomputed here: the rule lives
/// in the service layer, and a frontend that reassembled it from the
/// five facets would be a second definition free to drift.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, TS)]
pub struct ReadinessReport {
    pub capture: ReadinessFacet,
    pub transcription: ReadinessFacet,
    pub notes: ReadinessFacet,
    pub storage: ReadinessFacet,
    pub egress: ReadinessFacet,
    pub can_record: bool,
}

impl From<&Readiness> for ReadinessReport {
    fn from(readiness: &Readiness) -> Self {
        Self {
            capture: (&readiness.capture).into(),
            transcription: (&readiness.transcription).into(),
            notes: (&readiness.notes).into(),
            storage: (&readiness.storage).into(),
            egress: (&readiness.egress).into(),
            can_record: readiness.can_record(),
        }
    }
}

/// One diagnostic finding, as a diagnostics screen renders it.
///
/// The recovery action crosses as the JSON the service layer already
/// serializes it to, so the frontend hands back exactly what it was
/// given rather than reconstructing a value it might get wrong.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, TS)]
pub struct DiagnosticRow {
    pub code: String,
    pub severity: super::WarningSeverity,
    pub component: String,
    pub summary: String,
    /// `None` when nothing can be done about it.
    pub recovery_action: Option<RecoveryActionView>,
    /// Whether acting on it changes the system. Derived in the service
    /// layer from the action's own shape.
    pub mutation_required: bool,
}

/// A recovery action, in the form the frontend hands back unchanged.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, TS)]
pub struct RecoveryActionView {
    /// The action's tagged discriminant.
    pub action: String,
    /// The session the action names, when it names one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    /// The file the action names, when it names one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// The capability the action names, when it names one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub capability: Option<String>,
    /// What the platform calls that capability, and where it is
    /// granted. Present only for a permission recovery.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub capability_label: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub settings_url: Option<String>,
}

impl From<&RecoveryAction> for RecoveryActionView {
    fn from(action: &RecoveryAction) -> Self {
        let mut view = Self {
            action: discriminant(action).to_string(),
            id: None,
            name: None,
            capability: None,
            capability_label: None,
            settings_url: None,
        };
        match action {
            RecoveryAction::RepairSession { id }
            | RecoveryAction::RemoveStaleSessionLock { id } => {
                view.id = Some(id.as_str().to_string());
            }
            RecoveryAction::InstallTranscriptionModel { id } => view.id = Some(id.clone()),
            RecoveryAction::RemoveOrphanedPartial { name } => {
                view.name = Some(name.as_str().to_string());
            }
            RecoveryAction::RemoveModelPartial { name } => view.name = Some(name.clone()),
            RecoveryAction::OpenSystemSettings { capability } => {
                view.capability = Some(capability_slug(*capability).to_string());
                view.capability_label = Some(capability.label().to_string());
                view.settings_url = Some(capability.settings_url().to_string());
            }
            RecoveryAction::CreateStorageRoot
            | RecoveryAction::ReviewConfiguration
            | RecoveryAction::OpenAdvancedConfiguration => {}
        }
        view
    }
}

const fn discriminant(action: &RecoveryAction) -> &'static str {
    match action {
        RecoveryAction::CreateStorageRoot => "create_storage_root",
        RecoveryAction::RepairSession { .. } => "repair_session",
        RecoveryAction::RemoveStaleSessionLock { .. } => "remove_stale_session_lock",
        RecoveryAction::RemoveOrphanedPartial { .. } => "remove_orphaned_partial",
        RecoveryAction::ReviewConfiguration => "review_configuration",
        RecoveryAction::InstallTranscriptionModel { .. } => "install_transcription_model",
        RecoveryAction::RemoveModelPartial { .. } => "remove_model_partial",
        RecoveryAction::OpenSystemSettings { .. } => "open_system_settings",
        RecoveryAction::OpenAdvancedConfiguration => "open_advanced_configuration",
    }
}

const fn capability_slug(capability: Capability) -> &'static str {
    match capability {
        Capability::Microphone => "microphone",
        Capability::SystemAudioRecording => "system_audio_recording",
    }
}

impl From<&DiagnosticFinding> for DiagnosticRow {
    fn from(finding: &DiagnosticFinding) -> Self {
        Self {
            code: serde_json::to_value(finding.code)
                .ok()
                .and_then(|value| value.as_str().map(str::to_string))
                .unwrap_or_default(),
            severity: finding.severity.into(),
            component: serde_json::to_value(finding.component)
                .ok()
                .and_then(|value| value.as_str().map(str::to_string))
                .unwrap_or_default(),
            summary: finding.summary.clone(),
            recovery_action: finding.recovery_action.as_ref().map(Into::into),
            mutation_required: finding.mutation_required,
        }
    }
}

/// Everything one read-only diagnosis found.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, TS)]
pub struct DiagnosticRows {
    pub rows: Vec<DiagnosticRow>,
    pub warning_count: usize,
}

impl From<&DiagnosticReport> for DiagnosticRows {
    fn from(report: &DiagnosticReport) -> Self {
        Self {
            rows: report.findings.iter().map(Into::into).collect(),
            warning_count: report.warning_count(),
        }
    }
}

/// What one explicitly requested repair did.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, TS)]
pub struct RepairOutcome {
    /// `true` when the system was changed, `false` when the condition
    /// had already been resolved.
    pub applied: bool,
    pub summary: String,
}

impl From<RepairApplication> for RepairOutcome {
    fn from(application: RepairApplication) -> Self {
        Self {
            applied: application.status == scrybe_application::diagnostics::RepairStatus::Applied,
            summary: application.summary,
        }
    }
}

/// Everything shown before any model-network access.
///
/// Every field a user must see before confirming is here, and the
/// confirmation they hand back names the digest this carries — so a
/// screen that omitted one of these could not produce a confirmation
/// the service layer would accept.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, TS)]
pub struct ModelOffer {
    pub id: String,
    pub source_url: String,
    pub source_revision: String,
    pub license: String,
    pub size_bytes: String,
    pub sha256: String,
    pub runtime: String,
    pub destination: String,
    pub destination_path: String,
    pub required_bytes: String,
    /// `None` when the platform will not report free space, which is
    /// shown as unknown rather than as plenty or as none.
    pub available_bytes: Option<String>,
    pub sufficient_space: bool,
    pub state: String,
    /// Present when the state is a failure, describing which one.
    pub failure: Option<String>,
}

impl From<ModelPlan> for ModelOffer {
    fn from(plan: ModelPlan) -> Self {
        let (state, failure) = describe_state(&plan.state);
        Self {
            id: plan.id,
            source_url: plan.source_url,
            source_revision: plan.source_revision,
            license: plan.license,
            // Byte counts cross as decimal strings. A half-gigabyte
            // artifact is well inside what a double represents exactly,
            // but the boundary is one this contract should not leave to
            // the frontend's number type to respect.
            size_bytes: plan.size_bytes.to_string(),
            sha256: plan.sha256,
            runtime: plan.runtime,
            destination: plan.destination,
            destination_path: plan.destination_path,
            required_bytes: plan.required_bytes.to_string(),
            available_bytes: plan.available_bytes.map(|bytes| bytes.to_string()),
            sufficient_space: plan.sufficient_space,
            state,
            failure,
        }
    }
}

impl From<InstallReport> for ModelOutcome {
    fn from(report: InstallReport) -> Self {
        let (state, failure) = describe_state(&report.state);
        Self {
            id: report.id,
            state,
            failure,
            promoted: report.promoted,
        }
    }
}

/// The state's discriminant, and a line about the failure when there is
/// one.
///
/// Flattened here rather than mirrored as a tagged union, because the
/// frontend branches on the discriminant and renders the line; a
/// second enumeration would be a second thing to keep in step for no
/// rendering it enables.
fn describe_state(state: &ModelState) -> (String, Option<String>) {
    match state {
        ModelState::Available => ("available".into(), None),
        ModelState::Downloading { .. } => ("downloading".into(), None),
        ModelState::Verifying => ("verifying".into(), None),
        ModelState::Ready => ("ready".into(), None),
        ModelState::Cancelled => ("cancelled".into(), None),
        ModelState::Failed { reason } => ("failed".into(), Some(describe_failure(reason))),
    }
}

fn describe_failure(reason: &ModelFailure) -> String {
    match reason {
        ModelFailure::SizeMismatch { expected, observed } => {
            format!("the artifact was {observed} bytes where the catalog describes {expected}")
        }
        ModelFailure::DigestMismatch { .. } => {
            "the artifact did not match the checked-in digest, so nothing was installed".into()
        }
        ModelFailure::InsufficientSpace {
            required,
            available,
        } => format!("installing it needs {required} bytes and {available} are free"),
        ModelFailure::Transport { summary }
        | ModelFailure::Storage { summary }
        | ModelFailure::InstalledArtifactUnrecognized { summary } => summary.clone(),
    }
}

/// How far an acquisition has got.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, TS)]
pub struct ModelProgress {
    pub id: String,
    pub received_bytes: String,
    pub total_bytes: String,
}

/// The event the host publishes as bytes arrive.
pub const MODEL_PROGRESS_EVENT: &str = "scrybe://model-progress";

/// How one acquisition ended.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, TS)]
pub struct ModelOutcome {
    pub id: String,
    pub state: String,
    pub failure: Option<String>,
    /// Whether this call put a verified artifact at the destination.
    pub promoted: bool,
}
