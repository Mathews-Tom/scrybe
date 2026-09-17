// Copyright 2026 Mathews Tom
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//     https://www.apache.org/licenses/LICENSE-2.0

//! Serializable configuration types.

use std::collections::BTreeMap;

use scrybe_core::config::{Config, ShellIndicator};
use serde::{Deserialize, Serialize};

use crate::diagnostics::Severity;

/// A configuration value a GUI may write.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConfigValueKind {
    Text,
    Integer,
    Boolean,
}

/// A typed value bound for one [`ConfigField`].
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ConfigValue {
    Boolean(bool),
    Integer(i64),
    Text(String),
}

impl ConfigValue {
    /// The kind this value can satisfy.
    #[must_use]
    pub const fn kind(&self) -> ConfigValueKind {
        match self {
            Self::Boolean(_) => ConfigValueKind::Boolean,
            Self::Integer(_) => ConfigValueKind::Integer,
            Self::Text(_) => ConfigValueKind::Text,
        }
    }
}

impl From<&str> for ConfigValue {
    fn from(value: &str) -> Self {
        Self::Text(value.to_string())
    }
}

impl From<String> for ConfigValue {
    fn from(value: String) -> Self {
        Self::Text(value)
    }
}

impl From<bool> for ConfigValue {
    fn from(value: bool) -> Self {
        Self::Boolean(value)
    }
}

impl From<u32> for ConfigValue {
    fn from(value: u32) -> Self {
        Self::Integer(i64::from(value))
    }
}

/// The closed set of configuration fields a GUI owns.
///
/// Adding a variant is the only way to widen the write surface, which
/// is what keeps credentials out of it: no variant addresses
/// `[stt].api_key_env`, `[llm].api_key_env`, or any other
/// credential-bearing key, so no update can name one.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConfigField {
    StorageRoot,
    StorageAudioBitrateKbps,
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
    AgentAccessEnabled,
}

/// Every field a GUI may write, in a stable order.
pub const EDITABLE_FIELDS: [ConfigField; 13] = [
    ConfigField::StorageRoot,
    ConfigField::StorageAudioBitrateKbps,
    ConfigField::RecordSource,
    ConfigField::RecordSystemBackend,
    ConfigField::RecordLlm,
    ConfigField::SttProvider,
    ConfigField::SttModel,
    ConfigField::SttLanguage,
    ConfigField::LlmProvider,
    ConfigField::LlmBaseUrl,
    ConfigField::LlmModel,
    ConfigField::ConsentDefaultMode,
    ConfigField::AgentAccessEnabled,
];

impl ConfigField {
    /// The TOML table the field lives in.
    #[must_use]
    pub const fn table(self) -> &'static str {
        match self {
            Self::StorageRoot | Self::StorageAudioBitrateKbps => "storage",
            Self::RecordSource | Self::RecordSystemBackend | Self::RecordLlm => "record",
            Self::SttProvider | Self::SttModel | Self::SttLanguage => "stt",
            Self::LlmProvider | Self::LlmBaseUrl | Self::LlmModel => "llm",
            Self::ConsentDefaultMode => "consent",
            Self::AgentAccessEnabled => "agent_access",
        }
    }

    /// The TOML key within [`Self::table`].
    #[must_use]
    pub const fn key(self) -> &'static str {
        match self {
            Self::StorageRoot => "root",
            Self::StorageAudioBitrateKbps => "audio_bitrate_kbps",
            Self::RecordSource => "source",
            Self::RecordSystemBackend => "system_backend",
            Self::RecordLlm => "llm",
            Self::SttProvider | Self::LlmProvider => "provider",
            Self::SttModel | Self::LlmModel => "model",
            Self::SttLanguage => "language",
            Self::LlmBaseUrl => "base_url",
            Self::ConsentDefaultMode => "default_mode",
            Self::AgentAccessEnabled => "enabled",
        }
    }

    /// The value kind the field accepts.
    #[must_use]
    pub const fn kind(self) -> ConfigValueKind {
        match self {
            Self::StorageAudioBitrateKbps => ConfigValueKind::Integer,
            Self::AgentAccessEnabled => ConfigValueKind::Boolean,
            _ => ConfigValueKind::Text,
        }
    }
}

/// A set of field changes a GUI wants applied together.
///
/// Applied as one unit: the complete candidate document is validated
/// before any of it becomes durable.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ConfigUpdate {
    changes: BTreeMap<ConfigField, ConfigValue>,
}

impl ConfigUpdate {
    /// An update that changes nothing.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds or replaces one field change.
    #[must_use]
    pub fn set(mut self, field: ConfigField, value: impl Into<ConfigValue>) -> Self {
        self.changes.insert(field, value.into());
        self
    }

    /// The requested changes, in field order.
    pub fn entries(&self) -> impl Iterator<Item = (ConfigField, &ConfigValue)> {
        self.changes.iter().map(|(field, value)| (*field, value))
    }

    /// Whether the update would change nothing.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.changes.is_empty()
    }
}

/// A validation message about one field, or about the document.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ConfigDiagnostic {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub field: Option<ConfigField>,
    pub severity: Severity,
    pub message: String,
}

/// The form-ready projection of the current configuration.
///
/// Credential-bearing keys are absent. Whether a hosted provider needs
/// a credential is reported as a boolean, so a settings screen can warn
/// about it without the layer ever handling the value or its variable
/// name.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ConfigForm {
    pub schema_version: u32,
    pub storage_root: String,
    pub storage_audio_format: String,
    pub storage_audio_bitrate_kbps: u32,
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
    /// The configured speech-to-text provider is hosted and names an
    /// environment variable for its credential.
    pub stt_requires_hosted_credential: bool,
    /// The configured notes provider is hosted and names an environment
    /// variable for its credential.
    pub llm_requires_hosted_credential: bool,
}

impl From<&Config> for ConfigForm {
    fn from(config: &Config) -> Self {
        Self {
            schema_version: config.schema_version,
            storage_root: config.storage.root.display().to_string(),
            storage_audio_format: config.storage.audio_format.clone(),
            storage_audio_bitrate_kbps: config.storage.audio_bitrate_kbps,
            record_source: config.record.source.clone(),
            record_system_backend: config.record.system_backend.clone(),
            record_llm: config.record.llm.clone(),
            stt_provider: config.stt.provider.clone(),
            stt_model: config.stt.model.clone(),
            stt_language: config.stt.language.clone(),
            llm_provider: config.llm.provider.clone(),
            llm_base_url: config.llm.base_url.clone(),
            llm_model: config.llm.model.clone(),
            consent_default_mode: config.consent.default_mode.as_str().to_string(),
            shell_indicators: config
                .shell
                .indicators()
                .iter()
                .copied()
                .map(indicator_label)
                .map(str::to_string)
                .collect(),
            agent_access_enabled: config.agent_access.enabled,
            stt_requires_hosted_credential: config.stt.api_key_env.is_some(),
            llm_requires_hosted_credential: config.llm.api_key_env.is_some(),
        }
    }
}

/// The canonical `[shell].indicators` spelling of one indicator.
///
/// `scrybe-core` keeps its own accessor private, so the mapping is
/// restated here rather than borrowed. It cannot drift silently:
/// `test_every_indicator_label_matches_the_core_serialized_spelling`
/// compares every variant against the spelling `scrybe-core`'s own
/// `Serialize` implementation produces.
const fn indicator_label(indicator: ShellIndicator) -> &'static str {
    match indicator {
        ShellIndicator::MenuBarWaveform => "menu-bar-waveform",
        ShellIndicator::MenuBarLabel => "menu-bar-label",
        ShellIndicator::FloatingWindow => "floating-window",
    }
}

/// The configuration as a GUI sees it: where it lives, what it says,
/// and what is wrong with it.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ConfigSnapshot {
    /// Where the configuration file is or would be written.
    pub path: String,
    /// Whether that file exists. When it does not, [`Self::form`]
    /// reflects built-in defaults.
    pub exists: bool,
    pub form: ConfigForm,
    pub diagnostics: Vec<ConfigDiagnostic>,
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    #[test]
    fn test_the_form_carries_no_credential_key() {
        let mut config = Config::default();
        config.llm.api_key_env = Some("SCRYBE_TEST_LLM_KEY".into());
        config.stt.api_key_env = Some("SCRYBE_TEST_STT_KEY".into());

        let encoded = serde_json::to_string(&ConfigForm::from(&config)).unwrap();

        assert!(!encoded.contains("SCRYBE_TEST_LLM_KEY"));
        assert!(!encoded.contains("SCRYBE_TEST_STT_KEY"));
        assert!(!encoded.contains("api_key"));
    }

    #[test]
    fn test_the_form_reports_that_a_hosted_provider_needs_a_credential() {
        let mut config = Config::default();
        config.llm.api_key_env = Some("SCRYBE_TEST_LLM_KEY".into());

        let form = ConfigForm::from(&config);

        assert!(form.llm_requires_hosted_credential);
        assert!(!form.stt_requires_hosted_credential);
    }

    #[test]
    fn test_every_indicator_label_matches_the_core_serialized_spelling() {
        for indicator in [
            ShellIndicator::MenuBarWaveform,
            ShellIndicator::MenuBarLabel,
            ShellIndicator::FloatingWindow,
        ] {
            let canonical = serde_json::to_value(indicator).unwrap();

            assert_eq!(canonical, indicator_label(indicator));
        }
    }

    #[test]
    fn test_no_editable_field_addresses_a_credential_key() {
        for field in EDITABLE_FIELDS {
            assert!(
                !field.key().contains("api_key"),
                "{field:?} addresses a credential key"
            );
        }
    }

    #[test]
    fn test_an_update_keeps_one_value_per_field_in_field_order() {
        let update = ConfigUpdate::new()
            .set(ConfigField::LlmModel, "llama3")
            .set(ConfigField::StorageRoot, "~/meetings")
            .set(ConfigField::LlmModel, "qwen3");

        let entries: Vec<_> = update.entries().map(|(field, _)| field).collect();

        assert_eq!(
            entries,
            vec![ConfigField::StorageRoot, ConfigField::LlmModel]
        );
    }

    #[test]
    fn test_every_editable_field_declares_a_value_kind_its_writer_can_check() {
        for field in EDITABLE_FIELDS {
            let sample = match field.kind() {
                ConfigValueKind::Text => ConfigValue::Text("x".into()),
                ConfigValueKind::Integer => ConfigValue::Integer(1),
                ConfigValueKind::Boolean => ConfigValue::Boolean(true),
            };

            assert_eq!(sample.kind(), field.kind());
        }
    }
}
