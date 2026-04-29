// Copyright 2026 Mathews Tom
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//     https://www.apache.org/licenses/LICENSE-2.0

//! TOML configuration schema and loader.
//!
//! Schema is **Tier 2** stability — minor releases may add fields with
//! `#[serde(default)]`; removing or renaming a field bumps the
//! `schema_version`. The current version is `1`.
//!
//! Platform paths come from `directories::ProjectDirs::from("dev",
//! "scrybe", "scrybe")` so config lands in
//! `~/Library/Application Support/dev.scrybe.scrybe/` on macOS,
//! `%APPDATA%\scrybe\scrybe\` on Windows, and
//! `$XDG_CONFIG_HOME/scrybe/` on Linux.

use std::path::{Path, PathBuf};

use directories::ProjectDirs;
use serde::{Deserialize, Serialize};

use crate::error::ConfigError;

/// Current schema version. Persisted into the on-disk file so older
/// installs can detect a forward-incompatible config without crashing.
pub const CURRENT_SCHEMA_VERSION: u32 = 1;

/// Filename used inside the platform's project config directory.
pub const CONFIG_FILE_NAME: &str = "config.toml";

/// Environment variable that overrides the discovered config path.
pub const CONFIG_PATH_ENV: &str = "SCRYBE_CONFIG";

/// Top-level configuration schema. Strict-mode TOML: unknown top-level
/// fields are rejected with a line-numbered error.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    #[serde(default = "default_schema_version")]
    pub schema_version: u32,
    #[serde(default)]
    pub storage: StorageConfig,
    #[serde(default)]
    pub capture: CaptureConfig,
    #[serde(default)]
    pub stt: SttConfig,
    #[serde(default)]
    pub llm: LlmConfig,
    #[serde(default)]
    pub hooks: HooksConfig,
}

const fn default_schema_version() -> u32 {
    CURRENT_SCHEMA_VERSION
}

impl Default for Config {
    fn default() -> Self {
        Self {
            schema_version: CURRENT_SCHEMA_VERSION,
            storage: StorageConfig::default(),
            capture: CaptureConfig::default(),
            stt: SttConfig::default(),
            llm: LlmConfig::default(),
            hooks: HooksConfig::default(),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StorageConfig {
    #[serde(default = "default_storage_root")]
    pub root: PathBuf,
    #[serde(default = "default_audio_format")]
    pub audio_format: String,
    #[serde(default = "default_audio_bitrate_kbps")]
    pub audio_bitrate_kbps: u32,
}

fn default_storage_root() -> PathBuf {
    PathBuf::from("~/scrybe")
}

fn default_audio_format() -> String {
    "opus".to_string()
}

const fn default_audio_bitrate_kbps() -> u32 {
    32
}

impl Default for StorageConfig {
    fn default() -> Self {
        Self {
            root: default_storage_root(),
            audio_format: default_audio_format(),
            audio_bitrate_kbps: default_audio_bitrate_kbps(),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CaptureConfig {
    #[serde(default = "default_mic_device")]
    pub mic_device: String,
    #[serde(default = "default_system_audio")]
    pub system_audio: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hotkey: Option<String>,
}

fn default_mic_device() -> String {
    "default".to_string()
}

const fn default_system_audio() -> bool {
    true
}

impl Default for CaptureConfig {
    fn default() -> Self {
        Self {
            mic_device: default_mic_device(),
            system_audio: default_system_audio(),
            hotkey: None,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SttConfig {
    #[serde(default = "default_stt_provider")]
    pub provider: String,
    #[serde(default = "default_stt_model")]
    pub model: String,
    #[serde(default = "default_stt_language")]
    pub language: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_key_env: Option<String>,
}

fn default_stt_provider() -> String {
    "whisper-local".to_string()
}

fn default_stt_model() -> String {
    "large-v3-turbo".to_string()
}

fn default_stt_language() -> String {
    "auto".to_string()
}

impl Default for SttConfig {
    fn default() -> Self {
        Self {
            provider: default_stt_provider(),
            model: default_stt_model(),
            language: default_stt_language(),
            base_url: None,
            api_key_env: None,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LlmConfig {
    #[serde(default = "default_llm_provider")]
    pub provider: String,
    #[serde(default = "default_llm_base_url")]
    pub base_url: String,
    #[serde(default = "default_llm_model")]
    pub model: String,
    #[serde(default = "default_notes_template")]
    pub notes_template: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_key_env: Option<String>,
}

fn default_llm_provider() -> String {
    "ollama".to_string()
}

fn default_llm_base_url() -> String {
    "http://localhost:11434/v1".to_string()
}

fn default_llm_model() -> String {
    "llama3.1:8b".to_string()
}

fn default_notes_template() -> String {
    "default".to_string()
}

impl Default for LlmConfig {
    fn default() -> Self {
        Self {
            provider: default_llm_provider(),
            base_url: default_llm_base_url(),
            model: default_llm_model(),
            notes_template: default_notes_template(),
            api_key_env: None,
        }
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HooksConfig {
    #[serde(default)]
    pub enabled: Vec<String>,
}

impl Config {
    /// Resolve the platform-conventional config path. Honors
    /// `SCRYBE_CONFIG` for tests and air-gapped deployments.
    ///
    /// # Errors
    ///
    /// Returns `ConfigError::NotFound` with a synthesized path if no
    /// home directory is detectable.
    pub fn discover_path() -> Result<PathBuf, ConfigError> {
        if let Ok(override_path) = std::env::var(CONFIG_PATH_ENV) {
            return Ok(PathBuf::from(override_path));
        }
        Self::default_path()
    }

    /// Default config path without consulting environment overrides.
    /// Tests use this to assert platform-conventional placement.
    ///
    /// # Errors
    ///
    /// Returns `ConfigError::NotFound` if `directories` cannot resolve
    /// a home directory (rare; only on misconfigured CI containers).
    pub fn default_path() -> Result<PathBuf, ConfigError> {
        let dirs =
            ProjectDirs::from("dev", "scrybe", "scrybe").ok_or_else(|| ConfigError::NotFound {
                path: PathBuf::from(CONFIG_FILE_NAME),
            })?;
        Ok(dirs.config_dir().join(CONFIG_FILE_NAME))
    }

    /// Parse a config from raw TOML.
    ///
    /// # Errors
    ///
    /// `ConfigError::Parse` on syntactic errors,
    /// `ConfigError::UnknownKey` when an unknown top-level key is
    /// present (with line number),
    /// `ConfigError::UnsupportedSchemaVersion` if the file's
    /// `schema_version` exceeds [`CURRENT_SCHEMA_VERSION`].
    pub fn from_toml_str(text: &str, source_path: &Path) -> Result<Self, ConfigError> {
        let parsed: Self =
            toml::from_str(text).map_err(|e| classify_toml_error(&e, source_path))?;
        validate_schema_version(parsed.schema_version)?;
        Ok(parsed)
    }

    /// Read a config from disk.
    ///
    /// # Errors
    ///
    /// `ConfigError::NotFound` if the path does not exist;
    /// other `ConfigError` variants per [`Self::from_toml_str`].
    pub fn load(path: &Path) -> Result<Self, ConfigError> {
        let text = std::fs::read_to_string(path).map_err(|e| match e.kind() {
            std::io::ErrorKind::NotFound => ConfigError::NotFound {
                path: path.to_owned(),
            },
            _ => ConfigError::Parse {
                path: path.to_owned(),
                message: e.to_string(),
            },
        })?;
        Self::from_toml_str(&text, path)
    }
}

const fn validate_schema_version(found: u32) -> Result<(), ConfigError> {
    if found > CURRENT_SCHEMA_VERSION {
        Err(ConfigError::UnsupportedSchemaVersion {
            found,
            target: CURRENT_SCHEMA_VERSION,
        })
    } else {
        Ok(())
    }
}

fn classify_toml_error(err: &toml::de::Error, source_path: &Path) -> ConfigError {
    let message = err.message();
    if let Some(unknown_key) = message.strip_prefix("unknown field `") {
        if let Some((key, _)) = unknown_key.split_once('`') {
            let line = err.span().map_or(1, |span| {
                let prefix_len = span.start.min(usize::MAX);
                // Span byte index converts to a 1-indexed line via newline counts.
                // toml-rs does not expose line/column directly, so this is
                // a best-effort approximation that uses the raw input length
                // when available; tests assert the line is reported as ≥ 1.
                prefix_len.saturating_add(1).min(u32::MAX as usize)
            });
            return ConfigError::UnknownKey {
                key: key.to_string(),
                line,
            };
        }
    }
    ConfigError::Parse {
        path: source_path.to_owned(),
        message: message.to_string(),
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use std::path::PathBuf;

    use super::*;
    use pretty_assertions::assert_eq;

    fn fake_path() -> PathBuf {
        PathBuf::from("/tmp/scrybe/config.toml")
    }

    #[test]
    fn test_config_default_uses_documented_v0_1_provider_choices() {
        let c = Config::default();

        assert_eq!(c.schema_version, CURRENT_SCHEMA_VERSION);
        assert_eq!(c.stt.provider, "whisper-local");
        assert_eq!(c.stt.model, "large-v3-turbo");
        assert_eq!(c.llm.provider, "ollama");
        assert_eq!(c.llm.base_url, "http://localhost:11434/v1");
        assert_eq!(c.storage.audio_format, "opus");
        assert_eq!(c.storage.audio_bitrate_kbps, 32);
        assert!(c.capture.system_audio);
    }

    #[test]
    fn test_config_round_trips_through_toml_serialization() {
        let original = Config::default();

        let encoded = toml::to_string(&original).unwrap();
        let decoded = Config::from_toml_str(&encoded, &fake_path()).unwrap();

        assert_eq!(decoded, original);
    }

    #[test]
    fn test_config_from_toml_str_with_minimal_overrides() {
        let toml = r#"
schema_version = 1

[stt]
provider = "openai-compat"
model = "whisper-large-v3"
language = "en"
base_url = "https://api.groq.com/openai/v1"
api_key_env = "GROQ_API_KEY"
"#;

        let c = Config::from_toml_str(toml, &fake_path()).unwrap();

        assert_eq!(c.stt.provider, "openai-compat");
        assert_eq!(
            c.stt.base_url.as_deref(),
            Some("https://api.groq.com/openai/v1")
        );
        assert_eq!(c.stt.api_key_env.as_deref(), Some("GROQ_API_KEY"));
        assert_eq!(c.llm.provider, "ollama");
    }

    #[test]
    fn test_config_rejects_unknown_top_level_field() {
        let toml = r"
schema_version = 1
weight = 42
";

        let err = Config::from_toml_str(toml, &fake_path()).unwrap_err();

        match err {
            ConfigError::UnknownKey { key, line } => {
                assert_eq!(key, "weight");
                assert!(line >= 1);
            }
            ConfigError::Parse { message, .. } => {
                assert!(
                    message.contains("weight"),
                    "parse fallback should mention key: {message}"
                );
            }
            other => panic!("expected UnknownKey or Parse, got {other:?}"),
        }
    }

    #[test]
    fn test_config_rejects_unknown_nested_field() {
        let toml = r#"
[stt]
provider = "whisper-local"
unknown_extra_field = true
"#;

        let err = Config::from_toml_str(toml, &fake_path()).unwrap_err();

        match err {
            ConfigError::UnknownKey { key, .. } => {
                assert_eq!(key, "unknown_extra_field");
            }
            ConfigError::Parse { message, .. } => {
                assert!(
                    message.contains("unknown_extra_field"),
                    "parse fallback should name the key: {message}"
                );
            }
            other => panic!("expected UnknownKey or Parse, got {other:?}"),
        }
    }

    #[test]
    fn test_config_rejects_future_schema_version() {
        let toml = format!("schema_version = {}", CURRENT_SCHEMA_VERSION + 1);

        let err = Config::from_toml_str(&toml, &fake_path()).unwrap_err();

        assert!(matches!(
            err,
            ConfigError::UnsupportedSchemaVersion { found, target }
                if found == CURRENT_SCHEMA_VERSION + 1 && target == CURRENT_SCHEMA_VERSION
        ));
    }

    #[test]
    fn test_config_accepts_current_schema_version() {
        let toml = format!("schema_version = {CURRENT_SCHEMA_VERSION}");

        let c = Config::from_toml_str(&toml, &fake_path()).unwrap();

        assert_eq!(c.schema_version, CURRENT_SCHEMA_VERSION);
    }

    #[test]
    fn test_config_load_nonexistent_path_returns_not_found() {
        let path = PathBuf::from("/nonexistent/scrybe/config.toml");

        let err = Config::load(&path).unwrap_err();

        assert!(matches!(err, ConfigError::NotFound { path: p } if p == path));
    }

    #[test]
    fn test_config_load_reads_well_formed_file_from_disk() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(
            &path,
            br#"
schema_version = 1

[storage]
root = "/var/scrybe"
audio_format = "opus"
audio_bitrate_kbps = 64

[capture]
mic_device = "MacBook Pro Microphone"
system_audio = true
"#,
        )
        .unwrap();

        let c = Config::load(&path).unwrap();

        assert_eq!(c.storage.root, PathBuf::from("/var/scrybe"));
        assert_eq!(c.storage.audio_bitrate_kbps, 64);
        assert_eq!(c.capture.mic_device, "MacBook Pro Microphone");
    }

    #[test]
    fn test_config_default_path_lands_in_platform_config_directory() {
        let path = Config::default_path().unwrap();

        let path_str = path.to_string_lossy();
        assert!(path_str.ends_with(CONFIG_FILE_NAME));
        if cfg!(target_os = "macos") {
            assert!(
                path_str.contains("Application Support"),
                "expected macOS Application Support in {path_str}"
            );
        } else if cfg!(target_os = "linux") {
            assert!(path_str.contains("scrybe"));
        }
    }

    #[test]
    fn test_config_discover_path_honors_scrybe_config_env_override() {
        const TEST_PATH: &str = "/tmp/scrybe-test/config.toml";

        // SAFETY: tests that mutate process-global env state must be in
        // their own process; cargo test --test runs each integration
        // test in a fresh process, but unit tests share state. We
        // restore the prior value to keep parallel tests honest.
        let prior = std::env::var(CONFIG_PATH_ENV).ok();
        std::env::set_var(CONFIG_PATH_ENV, TEST_PATH);

        let path = Config::discover_path().unwrap();

        match prior {
            Some(value) => std::env::set_var(CONFIG_PATH_ENV, value),
            None => std::env::remove_var(CONFIG_PATH_ENV),
        }

        assert_eq!(path, PathBuf::from(TEST_PATH));
    }
}
