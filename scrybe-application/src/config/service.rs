// Copyright 2026 Mathews Tom
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//     https://www.apache.org/licenses/LICENSE-2.0

//! Reading and updating the configuration document.
//!
//! An update is applied as one unit and in one order: edit the document
//! in place, validate the **complete** candidate through the strict
//! schema, then replace the file atomically. Nothing is written until
//! the whole candidate is known to be valid, so a rejected update
//! leaves the previous file exactly as it was — there is no partially
//! applied state to roll back from.
//!
//! [`ConfigChange`] is the change event. It is returned only by a
//! successful [`ConfigService::apply`], so its existence is the proof
//! that the replacement is already durable.

use std::path::{Path, PathBuf};

use scrybe_core::config::Config;
use scrybe_core::storage::atomic_replace;
use serde::{Deserialize, Serialize};

use crate::config::contract::{ConfigDiagnostic, ConfigField, ConfigForm, ConfigSnapshot};
use crate::config::editor;
use crate::config::ConfigUpdate;
use crate::diagnostics::Severity;
use crate::error::{ApplicationError, ErrorCode};
use crate::Result;

/// A configuration change that is already durable.
///
/// Emitted only after the replacement has been written and renamed
/// into place, so a consumer that reacts to it can rely on a
/// subsequent read seeing the new values.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ConfigChange {
    /// The fields the update named, in field order.
    pub fields: Vec<ConfigField>,
    /// The configuration as it now stands.
    pub form: ConfigForm,
}

/// Reads and updates one configuration file.
pub struct ConfigService {
    path: PathBuf,
}

impl ConfigService {
    /// A service over the configuration file at `path`.
    #[must_use]
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    /// A service over the platform-conventional path, honoring
    /// `SCRYBE_CONFIG`.
    ///
    /// # Errors
    ///
    /// [`ErrorCode::ConfigUnreadable`] if no configuration path can be
    /// resolved at all.
    pub fn discover() -> Result<Self> {
        let path = Config::discover_path().map_err(|source| {
            ApplicationError::new(
                ErrorCode::ConfigUnreadable,
                "no configuration path could be resolved",
            )
            .with_source(source)
        })?;
        Ok(Self::new(path))
    }

    /// Where the configuration file is, or would be written.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The validated configuration, or built-in defaults when no file
    /// exists yet.
    ///
    /// # Errors
    ///
    /// [`ErrorCode::ConfigUnreadable`] when the file exists but cannot
    /// be read or parsed.
    pub fn load(&self) -> Result<Config> {
        if !self.path.exists() {
            return Ok(Config::default());
        }
        Config::load(&self.path).map_err(|source| {
            ApplicationError::new(
                ErrorCode::ConfigUnreadable,
                format!(
                    "the configuration file at {} could not be read",
                    self.path.display()
                ),
            )
            .with_source(source)
        })
    }

    /// The form-ready projection plus whatever is questionable about it.
    ///
    /// # Errors
    ///
    /// As [`Self::load`].
    pub fn snapshot(&self) -> Result<ConfigSnapshot> {
        let config = self.load()?;
        Ok(ConfigSnapshot {
            path: self.path.display().to_string(),
            exists: self.path.exists(),
            form: ConfigForm::from(&config),
            diagnostics: diagnose(&config),
        })
    }

    /// Applies a GUI-owned update.
    ///
    /// The document is edited in place so comments and blocks this
    /// layer does not model survive; the complete candidate is then
    /// validated through the strict schema; only then is the file
    /// replaced atomically. Any failure before the replacement leaves
    /// the previous file untouched.
    ///
    /// # Errors
    ///
    /// [`ErrorCode::ConfigUnreadable`] if the existing file cannot be
    /// read or is not well-formed TOML,
    /// [`ErrorCode::ConfigInvalid`] if the candidate is not a valid
    /// complete configuration, and
    /// [`ErrorCode::ConfigWriteFailed`] if the durable replacement
    /// fails.
    pub fn apply(&self, update: &ConfigUpdate) -> Result<ConfigChange> {
        let current = if self.path.exists() {
            std::fs::read_to_string(&self.path).map_err(|source| {
                ApplicationError::new(
                    ErrorCode::ConfigUnreadable,
                    format!(
                        "the configuration file at {} could not be read",
                        self.path.display()
                    ),
                )
                .with_source(source)
            })?
        } else {
            String::new()
        };

        let candidate = editor::apply(&current, update)?;
        let validated = Config::from_toml_str(&candidate, &self.path).map_err(|source| {
            ApplicationError::new(
                ErrorCode::ConfigInvalid,
                "the updated configuration is not a valid complete document",
            )
            .with_source(source)
        })?;

        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent).map_err(|source| {
                ApplicationError::new(
                    ErrorCode::ConfigWriteFailed,
                    format!(
                        "the configuration directory {} could not be created",
                        parent.display()
                    ),
                )
                .with_source(source)
            })?;
        }
        atomic_replace(&self.path, candidate.as_bytes()).map_err(|source| {
            ApplicationError::new(
                ErrorCode::ConfigWriteFailed,
                format!(
                    "the configuration file at {} could not be replaced",
                    self.path.display()
                ),
            )
            .with_source(source)
        })?;

        Ok(ConfigChange {
            fields: update.entries().map(|(field, _)| field).collect(),
            form: ConfigForm::from(&validated),
        })
    }
}

/// Validation messages the strict schema accepts but a running system
/// would not honor.
///
/// Every check delegates to the `validated_*` accessor `scrybe-core`
/// already exposes, so there is one definition of what each value may
/// be.
fn diagnose(config: &Config) -> Vec<ConfigDiagnostic> {
    let mut diagnostics = Vec::new();
    let mut unrecognized = |field: ConfigField, value: &str, what: &str| {
        diagnostics.push(ConfigDiagnostic {
            field: Some(field),
            severity: Severity::Warning,
            message: format!("{what} {value:?} is not recognized and will not be honored"),
        });
    };

    if config.record.validated_source().is_none() {
        unrecognized(
            ConfigField::RecordSource,
            &config.record.source,
            "capture source",
        );
    }
    if config.record.validated_system_backend().is_none() {
        unrecognized(
            ConfigField::RecordSystemBackend,
            &config.record.system_backend,
            "system-audio backend",
        );
    }
    if config.record.validated_llm().is_none() {
        unrecognized(ConfigField::RecordLlm, &config.record.llm, "notes backend");
    }
    if config.diarizer.validated_kind().is_none() {
        diagnostics.push(ConfigDiagnostic {
            field: None,
            severity: Severity::Warning,
            message: format!(
                "diarizer kind {:?} is not recognized and will not be honored",
                config.diarizer.kind
            ),
        });
    }
    if url::Url::parse(&config.llm.base_url).is_err() {
        diagnostics.push(ConfigDiagnostic {
            field: Some(ConfigField::LlmBaseUrl),
            severity: Severity::Error,
            message: "the notes provider base URL is not a valid URL".to_string(),
        });
    }
    diagnostics
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use crate::config::ConfigValue;
    use pretty_assertions::assert_eq;

    const HAND_WRITTEN: &str = r#"schema_version = 1

# One root so a backup tool has one target.
[storage]
root = "~/scrybe"
audio_format = "opus"

[llm]
provider = "openai-compat"
base_url = "http://127.0.0.1:11434/v1"
model = "qwen3:8b"
api_key_env = "SCRYBE_LLM_KEY"

# Advanced: unmodelled by the settings surface.
[hooks.webhook]
url = "http://127.0.0.1:9000/scrybe"
timeout_ms = 3000
"#;

    struct Fixture {
        dir: tempfile::TempDir,
    }

    impl Fixture {
        fn new(body: &str) -> Self {
            let dir = tempfile::tempdir().unwrap();
            std::fs::write(dir.path().join("config.toml"), body).unwrap();
            Self { dir }
        }

        fn service(&self) -> ConfigService {
            ConfigService::new(self.dir.path().join("config.toml"))
        }

        fn body(&self) -> String {
            std::fs::read_to_string(self.dir.path().join("config.toml")).unwrap()
        }
    }

    #[test]
    fn test_a_successful_update_is_durable_before_the_change_event_exists() {
        let fixture = Fixture::new(HAND_WRITTEN);
        let service = fixture.service();

        let change = service
            .apply(&ConfigUpdate::new().set(ConfigField::LlmModel, "llama3.3"))
            .unwrap();

        assert_eq!(change.fields, vec![ConfigField::LlmModel]);
        assert_eq!(change.form.llm_model, "llama3.3");
        assert_eq!(service.load().unwrap().llm.model, "llama3.3");
        assert!(fixture
            .body()
            .contains("# One root so a backup tool has one target."));
    }

    #[test]
    fn test_an_update_preserves_comments_and_unmodelled_blocks_on_disk() {
        let fixture = Fixture::new(HAND_WRITTEN);

        fixture
            .service()
            .apply(&ConfigUpdate::new().set(ConfigField::StorageAudioBitrateKbps, 64_u32))
            .unwrap();

        let body = fixture.body();
        assert!(body.contains("# Advanced: unmodelled by the settings surface."));
        assert!(body.contains("[hooks.webhook]"));
        assert!(body.contains("timeout_ms = 3000"));
        assert!(body.contains("audio_bitrate_kbps = 64"));
    }

    #[test]
    fn test_a_candidate_the_strict_schema_rejects_leaves_the_previous_file_byte_identical() {
        let fixture = Fixture::new(HAND_WRITTEN);
        let before = fixture.body();

        // `audio_bitrate_kbps` is a `u32`; a negative integer is
        // well-formed TOML the complete-document check refuses.
        let mut update = ConfigUpdate::new();
        update = update.set(
            ConfigField::StorageAudioBitrateKbps,
            ConfigValue::Integer(-1),
        );

        let error = fixture.service().apply(&update).unwrap_err();

        assert_eq!(error.code(), ErrorCode::ConfigInvalid);
        assert_eq!(fixture.body(), before);
    }

    #[test]
    fn test_an_unreadable_existing_file_is_never_overwritten() {
        let fixture = Fixture::new("[storage\nroot = \"~/scrybe\"\n");
        let before = fixture.body();

        let error = fixture
            .service()
            .apply(&ConfigUpdate::new().set(ConfigField::StorageRoot, "~/meetings"))
            .unwrap_err();

        assert_eq!(error.code(), ErrorCode::ConfigUnreadable);
        assert_eq!(fixture.body(), before);
    }

    #[test]
    fn test_updating_when_no_file_exists_yet_writes_a_valid_complete_document() {
        let dir = tempfile::tempdir().unwrap();
        let service = ConfigService::new(dir.path().join("nested").join("config.toml"));

        let change = service
            .apply(&ConfigUpdate::new().set(ConfigField::AgentAccessEnabled, true))
            .unwrap();

        assert!(change.form.agent_access_enabled);
        assert!(service.load().unwrap().agent_access.enabled);
    }

    #[test]
    fn test_no_update_can_write_a_credential() {
        let fixture = Fixture::new(HAND_WRITTEN);
        let mut update = ConfigUpdate::new();
        for field in crate::config::EDITABLE_FIELDS {
            update = match field.kind() {
                crate::config::ConfigValueKind::Text => update.set(field, "synthetic"),
                crate::config::ConfigValueKind::Integer => update.set(field, 64_u32),
                crate::config::ConfigValueKind::Boolean => update.set(field, true),
                crate::config::ConfigValueKind::TextList => {
                    update.set(field, vec!["menu-bar-label".to_string()])
                }
            };
        }

        // Every text field set to the same recognized-or-not value keeps
        // the document well-formed; what matters is the credential.
        let _ = fixture.service().apply(&update);

        assert!(fixture.body().contains(r#"api_key_env = "SCRYBE_LLM_KEY""#));
    }

    #[test]
    fn test_a_snapshot_flags_an_unrecognized_capture_source() {
        let fixture = Fixture::new("schema_version = 1\n\n[record]\nsource = \"telepathy\"\n");

        let snapshot = fixture.service().snapshot().unwrap();

        assert_eq!(
            snapshot
                .diagnostics
                .iter()
                .filter(|diagnostic| diagnostic.field == Some(ConfigField::RecordSource))
                .count(),
            1
        );
    }

    #[test]
    fn test_a_snapshot_of_the_default_configuration_is_free_of_diagnostics() {
        let dir = tempfile::tempdir().unwrap();
        let service = ConfigService::new(dir.path().join("config.toml"));

        let snapshot = service.snapshot().unwrap();

        assert!(!snapshot.exists);
        assert_eq!(snapshot.diagnostics, Vec::new());
    }
}
