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
use crate::providers::retry::RetryPolicy;
use crate::types::ConsentMode;

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
    pub record: RecordConfig,
    #[serde(default)]
    pub stt: SttConfig,
    #[serde(default)]
    pub llm: LlmConfig,
    #[serde(default)]
    pub context: ContextConfig,
    #[serde(default)]
    pub hooks: HooksConfig,
    #[serde(default)]
    pub consent: ConsentConfig,
    /// Linux-specific capture overrides. Present in the schema on every
    /// platform so a config file authored on Linux still parses cleanly
    /// on macOS / Windows; the `scrybe-capture-linux` adapter is the
    /// only consumer.
    #[serde(default)]
    pub linux: LinuxConfig,
    /// Windows-specific capture overrides. Present in the schema on
    /// every platform so a config file authored on Windows still parses
    /// cleanly on macOS / Linux; the `scrybe-capture-win` adapter is the
    /// only consumer.
    #[serde(default)]
    pub windows: WindowsConfig,
    /// Android-specific capture overrides. Present in the schema on
    /// every platform so a config file authored on Android still parses
    /// cleanly on desktop hosts; the `scrybe-android` adapter is the
    /// only consumer.
    #[serde(default)]
    pub android: AndroidConfig,
    /// Diarizer selection. `kind` is empty by default ("auto"); the
    /// pipeline routes via `scrybe_core::diarize::select_kind` against
    /// the live `Capabilities` and `MeetingContext`.
    #[serde(default)]
    pub diarizer: DiarizerConfig,
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
            record: RecordConfig::default(),
            stt: SttConfig::default(),
            llm: LlmConfig::default(),
            context: ContextConfig::default(),
            hooks: HooksConfig::default(),
            consent: ConsentConfig::default(),
            linux: LinuxConfig::default(),
            windows: WindowsConfig::default(),
            android: AndroidConfig::default(),
            diarizer: DiarizerConfig::default(),
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

/// `[record]` block. These are the defaults consumed by bare
/// `scrybe record`; CLI flags override them for one-off runs.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RecordConfig {
    #[serde(default = "default_record_source")]
    pub source: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub whisper_model: Option<PathBuf>,
    #[serde(default = "default_record_llm")]
    pub llm: String,
}

fn default_record_source() -> String {
    RECORD_SOURCE_SYNTHETIC.to_string()
}

fn default_record_llm() -> String {
    RECORD_LLM_STUB.to_string()
}

impl Default for RecordConfig {
    fn default() -> Self {
        Self {
            source: default_record_source(),
            whisper_model: None,
            llm: default_record_llm(),
        }
    }
}

pub const RECORD_SOURCE_SYNTHETIC: &str = "synthetic";
pub const RECORD_SOURCE_MIC: &str = "mic";
pub const RECORD_SOURCE_MIC_SYSTEM: &str = "mic+system";

pub const RECORD_LLM_STUB: &str = "stub";
pub const RECORD_LLM_OPENAI_COMPAT: &str = "openai-compat";

impl RecordConfig {
    #[must_use]
    pub fn validated_source(&self) -> Option<&'static str> {
        match self.source.as_str() {
            RECORD_SOURCE_SYNTHETIC => Some(RECORD_SOURCE_SYNTHETIC),
            RECORD_SOURCE_MIC => Some(RECORD_SOURCE_MIC),
            RECORD_SOURCE_MIC_SYSTEM => Some(RECORD_SOURCE_MIC_SYSTEM),
            _ => None,
        }
    }

    #[must_use]
    pub fn validated_llm(&self) -> Option<&'static str> {
        match self.llm.as_str() {
            RECORD_LLM_STUB => Some(RECORD_LLM_STUB),
            RECORD_LLM_OPENAI_COMPAT => Some(RECORD_LLM_OPENAI_COMPAT),
            _ => None,
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
    /// Retry policy applied by cloud STT providers. Defaults to the
    /// system-wide default; see [`RetryPolicy::default`].
    #[serde(default)]
    pub retry: RetryPolicy,
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
            retry: RetryPolicy::default(),
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
    /// Retry policy applied by cloud LLM providers.
    #[serde(default)]
    pub retry: RetryPolicy,
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
            retry: RetryPolicy::default(),
        }
    }
}

/// `[context]` block. Lists the context-provider names to consult and
/// (optionally) the path to the local `.ics` calendar for
/// `IcsFileProvider`.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ContextConfig {
    /// Provider names in priority order. The CLI registers a provider
    /// only if its name appears here. Default is empty, which means the
    /// session uses CLI-flag context only — preserves v0.1 behavior.
    #[serde(default)]
    pub sources: Vec<String>,
    /// Path to the local `.ics` calendar consulted by
    /// `IcsFileProvider`. `None` disables the provider even when
    /// `"ics"` appears in `sources`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ics_path: Option<PathBuf>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HooksConfig {
    #[serde(default)]
    pub enabled: Vec<String>,
    /// Webhook hook configuration. Present only when `"webhook"` is in
    /// `enabled`; absent in the default config so v0.1 installs do not
    /// surface a half-configured block.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub webhook: Option<WebhookConfig>,
}

/// `[hooks.webhook]` block. The HMAC secret is loaded from the
/// environment variable named in `secret_env`; the secret value never
/// appears in the config file.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WebhookConfig {
    pub url: String,
    /// Environment variable that holds the HMAC-SHA256 secret. `None`
    /// produces an unsigned webhook (acceptable for receivers that
    /// authenticate by URL alone).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub secret_env: Option<String>,
    #[serde(default = "default_webhook_timeout_ms")]
    pub timeout_ms: u32,
}

const fn default_webhook_timeout_ms() -> u32 {
    10_000
}

impl Default for WebhookConfig {
    fn default() -> Self {
        Self {
            url: String::new(),
            secret_env: None,
            timeout_ms: default_webhook_timeout_ms(),
        }
    }
}

/// `[consent]` block. Sets the default mode used when no `--consent`
/// CLI flag is passed; the floor is `Quick` per `system-design.md` §5.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConsentConfig {
    #[serde(default)]
    pub default_mode: ConsentMode,
}

/// `[linux]` block. Linux capture-adapter overrides; ignored on
/// other platforms.
///
/// The `audio_backend` field corresponds to the
/// `scrybe-capture-linux` `Backend` enum and accepts `"auto"`,
/// `"pipewire"`, or `"pulse"`. Default is `"auto"` so a user-edited
/// config that omits the block continues to work.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LinuxConfig {
    #[serde(default = "default_linux_audio_backend")]
    pub audio_backend: String,
}

fn default_linux_audio_backend() -> String {
    "auto".to_string()
}

impl Default for LinuxConfig {
    fn default() -> Self {
        Self {
            audio_backend: default_linux_audio_backend(),
        }
    }
}

/// Backend names accepted by `LinuxConfig::audio_backend`.
///
/// Kept as string constants here (rather than importing the
/// `scrybe-capture-linux` `Backend` enum) because `scrybe-core` must
/// not depend on a platform adapter crate. Tested for parity with
/// `scrybe_capture_linux::backend::Backend::from_config_str`.
pub const LINUX_AUDIO_BACKEND_AUTO: &str = "auto";
pub const LINUX_AUDIO_BACKEND_PIPEWIRE: &str = "pipewire";
pub const LINUX_AUDIO_BACKEND_PULSE: &str = "pulse";

impl LinuxConfig {
    /// Validated view of the configured backend. Returns the canonical
    /// string (`"auto"`, `"pipewire"`, or `"pulse"`) when the field is
    /// recognised, or `None` for any other value so the caller can
    /// surface a useful error rather than silently accepting a typo.
    #[must_use]
    pub fn validated_backend(&self) -> Option<&'static str> {
        match self.audio_backend.as_str() {
            LINUX_AUDIO_BACKEND_AUTO => Some(LINUX_AUDIO_BACKEND_AUTO),
            LINUX_AUDIO_BACKEND_PIPEWIRE => Some(LINUX_AUDIO_BACKEND_PIPEWIRE),
            LINUX_AUDIO_BACKEND_PULSE => Some(LINUX_AUDIO_BACKEND_PULSE),
            _ => None,
        }
    }
}

/// `[windows]` block. Windows capture-adapter overrides; ignored on
/// other platforms.
///
/// The `audio_backend` field corresponds to the
/// `scrybe-capture-win` `Backend` enum and accepts `"auto"`,
/// `"wasapi-loopback"`, or `"wasapi-process-loopback"`. Default is
/// `"auto"` so a user-edited config that omits the block continues
/// to work; auto resolves to per-process loopback on Windows 10 build
/// 20348+ and falls back to system-wide loopback on older builds.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WindowsConfig {
    #[serde(default = "default_windows_audio_backend")]
    pub audio_backend: String,
}

fn default_windows_audio_backend() -> String {
    "auto".to_string()
}

impl Default for WindowsConfig {
    fn default() -> Self {
        Self {
            audio_backend: default_windows_audio_backend(),
        }
    }
}

/// Backend names accepted by `WindowsConfig::audio_backend`.
///
/// Kept as string constants here (rather than importing the
/// `scrybe-capture-win` `Backend` enum) because `scrybe-core` must
/// not depend on a platform adapter crate. Tested for parity with
/// `scrybe_capture_win::backend::Backend::from_config_str`.
pub const WINDOWS_AUDIO_BACKEND_AUTO: &str = "auto";
pub const WINDOWS_AUDIO_BACKEND_WASAPI_LOOPBACK: &str = "wasapi-loopback";
pub const WINDOWS_AUDIO_BACKEND_WASAPI_PROCESS_LOOPBACK: &str = "wasapi-process-loopback";

impl WindowsConfig {
    /// Validated view of the configured backend. Returns the canonical
    /// string (`"auto"`, `"wasapi-loopback"`, or `"wasapi-process-loopback"`)
    /// when the field is recognised, or `None` for any other value so
    /// the caller can surface a useful error rather than silently
    /// accepting a typo.
    #[must_use]
    pub fn validated_backend(&self) -> Option<&'static str> {
        match self.audio_backend.as_str() {
            WINDOWS_AUDIO_BACKEND_AUTO => Some(WINDOWS_AUDIO_BACKEND_AUTO),
            WINDOWS_AUDIO_BACKEND_WASAPI_LOOPBACK => Some(WINDOWS_AUDIO_BACKEND_WASAPI_LOOPBACK),
            WINDOWS_AUDIO_BACKEND_WASAPI_PROCESS_LOOPBACK => {
                Some(WINDOWS_AUDIO_BACKEND_WASAPI_PROCESS_LOOPBACK)
            }
            _ => None,
        }
    }
}

/// `[android]` block. Android capture-adapter overrides; ignored on
/// other platforms.
///
/// `audio_backend` corresponds to the `scrybe-android` `Backend` enum
/// and accepts `"auto"`, `"media-projection"`, or `"mic-only"`. Default
/// is `"auto"`, which the runtime resolves against the host's
/// `MediaProjection` availability (API 29+) at session start.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AndroidConfig {
    #[serde(default = "default_android_audio_backend")]
    pub audio_backend: String,
}

fn default_android_audio_backend() -> String {
    "auto".to_string()
}

impl Default for AndroidConfig {
    fn default() -> Self {
        Self {
            audio_backend: default_android_audio_backend(),
        }
    }
}

/// Backend names accepted by `AndroidConfig::audio_backend`.
///
/// Kept as string constants (rather than importing the `scrybe-android`
/// `Backend` enum) so `scrybe-core` does not depend on a platform
/// adapter crate. Tested for parity with
/// `scrybe_android::backend::Backend::from_config_str`.
pub const ANDROID_AUDIO_BACKEND_AUTO: &str = "auto";
pub const ANDROID_AUDIO_BACKEND_MEDIA_PROJECTION: &str = "media-projection";
pub const ANDROID_AUDIO_BACKEND_MIC_ONLY: &str = "mic-only";

impl AndroidConfig {
    /// Validated view of the configured backend. Returns the canonical
    /// string when the field is recognised, or `None` for any other
    /// value so the caller can surface a useful error rather than
    /// silently accepting a typo.
    #[must_use]
    pub fn validated_backend(&self) -> Option<&'static str> {
        match self.audio_backend.as_str() {
            ANDROID_AUDIO_BACKEND_AUTO => Some(ANDROID_AUDIO_BACKEND_AUTO),
            ANDROID_AUDIO_BACKEND_MEDIA_PROJECTION => Some(ANDROID_AUDIO_BACKEND_MEDIA_PROJECTION),
            ANDROID_AUDIO_BACKEND_MIC_ONLY => Some(ANDROID_AUDIO_BACKEND_MIC_ONLY),
            _ => None,
        }
    }
}

/// `[diarizer]` block. Diarizer selection.
///
/// Defaults to `auto`, which the pipeline resolves via
/// `scrybe_core::diarize::select_kind` against the live `Capabilities`
/// and `MeetingContext`. Set explicitly to `"binary-channel"` or
/// `"pyannote-onnx"` to override the auto-rule.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DiarizerConfig {
    #[serde(default = "default_diarizer_kind")]
    pub kind: String,
}

fn default_diarizer_kind() -> String {
    DIARIZER_KIND_AUTO.to_string()
}

impl Default for DiarizerConfig {
    fn default() -> Self {
        Self {
            kind: default_diarizer_kind(),
        }
    }
}

/// Sentinel meaning "let the pipeline decide". Resolved against
/// `Capabilities` + `MeetingContext` per `system-design.md` §4.4.
pub const DIARIZER_KIND_AUTO: &str = "auto";
/// Mirrors `crate::diarize::kind::DIARIZER_KIND_BINARY_CHANNEL`.
///
/// Held as a config-side constant so `scrybe-cli` and the round-trip
/// tests can reference the canonical string without importing the
/// diarizer module.
pub const DIARIZER_KIND_BINARY_CHANNEL: &str = "binary-channel";
/// Mirrors `crate::diarize::kind::DIARIZER_KIND_PYANNOTE_ONNX`.
pub const DIARIZER_KIND_PYANNOTE_ONNX: &str = "pyannote-onnx";

impl DiarizerConfig {
    /// Validated view of the configured kind. Returns one of the three
    /// canonical strings (`"auto"`, `"binary-channel"`, `"pyannote-onnx"`)
    /// when the field is recognised, or `None` for any other value so
    /// the caller can surface a useful error rather than silently
    /// accepting a typo.
    #[must_use]
    pub fn validated_kind(&self) -> Option<&'static str> {
        match self.kind.as_str() {
            DIARIZER_KIND_AUTO => Some(DIARIZER_KIND_AUTO),
            DIARIZER_KIND_BINARY_CHANNEL => Some(DIARIZER_KIND_BINARY_CHANNEL),
            DIARIZER_KIND_PYANNOTE_ONNX => Some(DIARIZER_KIND_PYANNOTE_ONNX),
            _ => None,
        }
    }

    /// True when the configured kind is the auto-routing sentinel.
    #[must_use]
    pub fn is_auto(&self) -> bool {
        self.kind == DIARIZER_KIND_AUTO
    }
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
            toml::from_str(text).map_err(|e| classify_toml_error(&e, source_path, text))?;
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

fn classify_toml_error(err: &toml::de::Error, source_path: &Path, text: &str) -> ConfigError {
    let message = err.message();
    if let Some(unknown_key) = message.strip_prefix("unknown field `") {
        if let Some((key, _)) = unknown_key.split_once('`') {
            let line = err.span().map_or(1, |span| line_of_byte(text, span.start));
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

/// 1-indexed line number for a byte offset in `text`. `toml::de::Error::span`
/// returns a byte range; `toml-rs` does not expose line/column directly,
/// so this counts newlines in the prefix up to `byte`. Pulling in
/// `bytecount` for SIMD newline counting would be overkill for a config
/// file that fits on a screen; the naive scan runs once per parse error.
#[allow(clippy::naive_bytecount)]
fn line_of_byte(text: &str, byte: usize) -> usize {
    let clamped = byte.min(text.len());
    text.as_bytes()[..clamped]
        .iter()
        .filter(|&&b| b == b'\n')
        .count()
        + 1
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
        assert_eq!(c.record.source, RECORD_SOURCE_SYNTHETIC);
        assert_eq!(c.record.llm, RECORD_LLM_STUB);
        assert_eq!(c.record.whisper_model, None);
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
    fn test_config_from_toml_str_with_record_overrides() {
        let toml = r#"
schema_version = 1

[record]
source = "mic+system"
whisper_model = "~/Library/Application Support/scrybe/models/ggml-base.en.bin"
llm = "openai-compat"
"#;

        let c = Config::from_toml_str(toml, &fake_path()).unwrap();

        assert_eq!(c.record.validated_source(), Some(RECORD_SOURCE_MIC_SYSTEM));
        assert_eq!(c.record.validated_llm(), Some(RECORD_LLM_OPENAI_COMPAT));
        assert_eq!(
            c.record.whisper_model.as_deref(),
            Some(Path::new(
                "~/Library/Application Support/scrybe/models/ggml-base.en.bin"
            ))
        );
    }

    #[test]
    fn test_config_rejects_unknown_top_level_field_and_reports_correct_line() {
        let toml = "schema_version = 1\nweight = 42\n";

        let err = Config::from_toml_str(toml, &fake_path()).unwrap_err();

        match err {
            ConfigError::UnknownKey { key, line } => {
                assert_eq!(key, "weight");
                assert_eq!(line, 2, "weight is on line 2 of the source");
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
    fn test_line_of_byte_counts_newlines_to_produce_1_indexed_line_number() {
        let text = "first\nsecond\nthird\n";

        assert_eq!(line_of_byte(text, 0), 1);
        assert_eq!(line_of_byte(text, 5), 1);
        assert_eq!(line_of_byte(text, 6), 2);
        assert_eq!(line_of_byte(text, 13), 3);
        assert_eq!(line_of_byte(text, 9_999), 4);
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
    fn test_config_default_includes_empty_context_and_default_consent_mode() {
        let c = Config::default();

        assert!(c.context.sources.is_empty());
        assert!(c.context.ics_path.is_none());
        assert!(c.hooks.webhook.is_none());
        assert_eq!(c.consent.default_mode, ConsentMode::Quick);
    }

    #[test]
    fn test_config_round_trips_default_through_toml_with_new_v0_2_blocks() {
        let original = Config::default();

        let encoded = toml::to_string(&original).unwrap();
        let decoded = Config::from_toml_str(&encoded, &fake_path()).unwrap();

        assert_eq!(decoded, original);
    }

    #[test]
    fn test_config_parses_explicit_retry_block_under_stt() {
        let toml = r#"
[stt]
provider = "openai-compat"
model = "whisper-large-v3"
base_url = "https://api.groq.com/openai/v1"
api_key_env = "GROQ_API_KEY"

[stt.retry]
max_attempts = 5
initial_backoff_ms = 250
max_backoff_ms = 16000
"#;
        let c = Config::from_toml_str(toml, &fake_path()).unwrap();

        assert_eq!(c.stt.retry.max_attempts, 5);
        assert_eq!(c.stt.retry.initial_backoff_ms, 250);
        assert_eq!(c.stt.retry.max_backoff_ms, 16_000);
    }

    #[test]
    fn test_config_parses_context_sources_and_ics_path() {
        let toml = r#"
[context]
sources = ["cli", "ics"]
ics_path = "/Users/tom/.calendars/work.ics"
"#;
        let c = Config::from_toml_str(toml, &fake_path()).unwrap();

        assert_eq!(c.context.sources, vec!["cli", "ics"]);
        assert_eq!(
            c.context.ics_path,
            Some(PathBuf::from("/Users/tom/.calendars/work.ics"))
        );
    }

    #[test]
    fn test_config_parses_webhook_block_with_optional_secret_env() {
        let toml = r#"
[hooks]
enabled = ["webhook"]

[hooks.webhook]
url = "https://example.com/scrybe-webhook"
secret_env = "SCRYBE_WEBHOOK_SECRET"
timeout_ms = 5000
"#;
        let c = Config::from_toml_str(toml, &fake_path()).unwrap();

        let webhook = c.hooks.webhook.expect("webhook block parsed");
        assert_eq!(webhook.url, "https://example.com/scrybe-webhook");
        assert_eq!(webhook.secret_env.as_deref(), Some("SCRYBE_WEBHOOK_SECRET"));
        assert_eq!(webhook.timeout_ms, 5_000);
    }

    #[test]
    fn test_config_parses_consent_default_mode() {
        let toml = r#"
[consent]
default_mode = "notify"
"#;
        let c = Config::from_toml_str(toml, &fake_path()).unwrap();

        assert_eq!(c.consent.default_mode, ConsentMode::Notify);
    }

    #[test]
    fn test_linux_config_default_audio_backend_is_auto() {
        let c = LinuxConfig::default();

        assert_eq!(c.audio_backend, "auto");
    }

    #[test]
    fn test_config_default_includes_linux_block_with_auto_backend() {
        let c = Config::default();

        assert_eq!(c.linux.audio_backend, "auto");
    }

    #[test]
    fn test_config_parses_explicit_linux_audio_backend_pipewire() {
        let toml = r#"
[linux]
audio_backend = "pipewire"
"#;

        let c = Config::from_toml_str(toml, &fake_path()).unwrap();

        assert_eq!(c.linux.audio_backend, "pipewire");
    }

    #[test]
    fn test_config_parses_explicit_linux_audio_backend_pulse() {
        let toml = r#"
[linux]
audio_backend = "pulse"
"#;

        let c = Config::from_toml_str(toml, &fake_path()).unwrap();

        assert_eq!(c.linux.audio_backend, "pulse");
    }

    #[test]
    fn test_config_rejects_unknown_field_inside_linux_block() {
        let toml = r#"
[linux]
audio_backend = "auto"
unknown_extra = true
"#;

        let err = Config::from_toml_str(toml, &fake_path()).unwrap_err();

        match err {
            ConfigError::UnknownKey { key, .. } => assert_eq!(key, "unknown_extra"),
            ConfigError::Parse { message, .. } => assert!(
                message.contains("unknown_extra"),
                "parse fallback should name the key: {message}"
            ),
            other => panic!("expected UnknownKey or Parse, got {other:?}"),
        }
    }

    #[test]
    fn test_config_round_trip_preserves_linux_audio_backend_override() {
        let mut original = Config::default();
        original.linux.audio_backend = "pipewire".to_string();

        let encoded = toml::to_string(&original).unwrap();
        let decoded = Config::from_toml_str(&encoded, &fake_path()).unwrap();

        assert_eq!(decoded.linux.audio_backend, "pipewire");
    }

    #[test]
    fn test_linux_config_validated_backend_accepts_canonical_values() {
        for value in ["auto", "pipewire", "pulse"] {
            let cfg = LinuxConfig {
                audio_backend: value.to_string(),
            };

            assert_eq!(cfg.validated_backend(), Some(value));
        }
    }

    #[test]
    fn test_linux_config_validated_backend_returns_none_for_unknown_value() {
        let cfg = LinuxConfig {
            audio_backend: "alsa".to_string(),
        };

        assert!(cfg.validated_backend().is_none());
    }

    #[test]
    fn test_linux_config_validated_backend_returns_none_for_empty_string() {
        let cfg = LinuxConfig {
            audio_backend: String::new(),
        };

        assert!(cfg.validated_backend().is_none());
    }

    #[test]
    fn test_linux_config_validated_backend_is_case_sensitive() {
        let cfg = LinuxConfig {
            audio_backend: "PipeWire".to_string(),
        };

        assert!(cfg.validated_backend().is_none());
    }

    #[test]
    fn test_windows_config_default_audio_backend_is_auto() {
        let c = WindowsConfig::default();

        assert_eq!(c.audio_backend, "auto");
    }

    #[test]
    fn test_config_default_includes_windows_block_with_auto_backend() {
        let c = Config::default();

        assert_eq!(c.windows.audio_backend, "auto");
    }

    #[test]
    fn test_config_parses_explicit_windows_audio_backend_wasapi_loopback() {
        let toml = r#"
[windows]
audio_backend = "wasapi-loopback"
"#;

        let c = Config::from_toml_str(toml, &fake_path()).unwrap();

        assert_eq!(c.windows.audio_backend, "wasapi-loopback");
    }

    #[test]
    fn test_config_parses_explicit_windows_audio_backend_wasapi_process_loopback() {
        let toml = r#"
[windows]
audio_backend = "wasapi-process-loopback"
"#;

        let c = Config::from_toml_str(toml, &fake_path()).unwrap();

        assert_eq!(c.windows.audio_backend, "wasapi-process-loopback");
    }

    #[test]
    fn test_config_rejects_unknown_field_inside_windows_block() {
        let toml = r#"
[windows]
audio_backend = "auto"
unknown_extra = true
"#;

        let err = Config::from_toml_str(toml, &fake_path()).unwrap_err();

        match err {
            ConfigError::UnknownKey { key, .. } => assert_eq!(key, "unknown_extra"),
            ConfigError::Parse { message, .. } => assert!(
                message.contains("unknown_extra"),
                "parse fallback should name the key: {message}"
            ),
            other => panic!("expected UnknownKey or Parse, got {other:?}"),
        }
    }

    #[test]
    fn test_config_round_trip_preserves_windows_audio_backend_override() {
        let mut original = Config::default();
        original.windows.audio_backend = "wasapi-process-loopback".to_string();

        let encoded = toml::to_string(&original).unwrap();
        let decoded = Config::from_toml_str(&encoded, &fake_path()).unwrap();

        assert_eq!(decoded.windows.audio_backend, "wasapi-process-loopback");
    }

    #[test]
    fn test_windows_config_validated_backend_accepts_canonical_values() {
        for value in ["auto", "wasapi-loopback", "wasapi-process-loopback"] {
            let cfg = WindowsConfig {
                audio_backend: value.to_string(),
            };

            assert_eq!(cfg.validated_backend(), Some(value));
        }
    }

    #[test]
    fn test_windows_config_validated_backend_returns_none_for_unknown_value() {
        let cfg = WindowsConfig {
            audio_backend: "wasapi".to_string(),
        };

        assert!(cfg.validated_backend().is_none());
    }

    #[test]
    fn test_windows_config_validated_backend_returns_none_for_empty_string() {
        let cfg = WindowsConfig {
            audio_backend: String::new(),
        };

        assert!(cfg.validated_backend().is_none());
    }

    #[test]
    fn test_windows_config_validated_backend_is_case_sensitive() {
        let cfg = WindowsConfig {
            audio_backend: "WASAPI-Loopback".to_string(),
        };

        assert!(cfg.validated_backend().is_none());
    }

    #[test]
    fn test_android_config_default_audio_backend_is_auto() {
        let c = AndroidConfig::default();

        assert_eq!(c.audio_backend, "auto");
    }

    #[test]
    fn test_config_default_includes_android_block_with_auto_backend() {
        let c = Config::default();

        assert_eq!(c.android.audio_backend, "auto");
    }

    #[test]
    fn test_config_parses_explicit_android_audio_backend_media_projection() {
        let toml = r#"
[android]
audio_backend = "media-projection"
"#;

        let c = Config::from_toml_str(toml, &fake_path()).unwrap();

        assert_eq!(c.android.audio_backend, "media-projection");
    }

    #[test]
    fn test_config_parses_explicit_android_audio_backend_mic_only() {
        let toml = r#"
[android]
audio_backend = "mic-only"
"#;

        let c = Config::from_toml_str(toml, &fake_path()).unwrap();

        assert_eq!(c.android.audio_backend, "mic-only");
    }

    #[test]
    fn test_config_rejects_unknown_field_inside_android_block() {
        let toml = r#"
[android]
audio_backend = "auto"
unknown_extra = true
"#;

        let err = Config::from_toml_str(toml, &fake_path()).unwrap_err();

        match err {
            ConfigError::UnknownKey { key, .. } => assert_eq!(key, "unknown_extra"),
            ConfigError::Parse { message, .. } => assert!(
                message.contains("unknown_extra"),
                "parse fallback should name the key: {message}"
            ),
            other => panic!("expected UnknownKey or Parse, got {other:?}"),
        }
    }

    #[test]
    fn test_config_round_trip_preserves_android_audio_backend_override() {
        let mut original = Config::default();
        original.android.audio_backend = "media-projection".to_string();

        let encoded = toml::to_string(&original).unwrap();
        let decoded = Config::from_toml_str(&encoded, &fake_path()).unwrap();

        assert_eq!(decoded.android.audio_backend, "media-projection");
    }

    #[test]
    fn test_android_config_validated_backend_accepts_canonical_values() {
        for value in ["auto", "media-projection", "mic-only"] {
            let cfg = AndroidConfig {
                audio_backend: value.to_string(),
            };

            assert_eq!(cfg.validated_backend(), Some(value));
        }
    }

    #[test]
    fn test_android_config_validated_backend_returns_none_for_unknown_value() {
        let cfg = AndroidConfig {
            audio_backend: "system".to_string(),
        };

        assert!(cfg.validated_backend().is_none());
    }

    #[test]
    fn test_android_config_validated_backend_returns_none_for_empty_string() {
        let cfg = AndroidConfig {
            audio_backend: String::new(),
        };

        assert!(cfg.validated_backend().is_none());
    }

    #[test]
    fn test_android_config_validated_backend_is_case_sensitive() {
        let cfg = AndroidConfig {
            audio_backend: "Media-Projection".to_string(),
        };

        assert!(cfg.validated_backend().is_none());
    }

    #[test]
    fn test_diarizer_config_default_kind_is_auto() {
        let c = DiarizerConfig::default();

        assert_eq!(c.kind, "auto");
        assert!(c.is_auto());
    }

    #[test]
    fn test_config_default_includes_diarizer_block_with_auto_kind() {
        let c = Config::default();

        assert_eq!(c.diarizer.kind, "auto");
    }

    #[test]
    fn test_config_parses_explicit_diarizer_kind_binary_channel() {
        let toml = r#"
[diarizer]
kind = "binary-channel"
"#;

        let c = Config::from_toml_str(toml, &fake_path()).unwrap();

        assert_eq!(c.diarizer.kind, "binary-channel");
        assert!(!c.diarizer.is_auto());
    }

    #[test]
    fn test_config_parses_explicit_diarizer_kind_pyannote_onnx() {
        let toml = r#"
[diarizer]
kind = "pyannote-onnx"
"#;

        let c = Config::from_toml_str(toml, &fake_path()).unwrap();

        assert_eq!(c.diarizer.kind, "pyannote-onnx");
    }

    #[test]
    fn test_config_rejects_unknown_field_inside_diarizer_block() {
        let toml = r#"
[diarizer]
kind = "auto"
unknown_extra = true
"#;

        let err = Config::from_toml_str(toml, &fake_path()).unwrap_err();

        match err {
            ConfigError::UnknownKey { key, .. } => assert_eq!(key, "unknown_extra"),
            ConfigError::Parse { message, .. } => assert!(
                message.contains("unknown_extra"),
                "parse fallback should name the key: {message}"
            ),
            other => panic!("expected UnknownKey or Parse, got {other:?}"),
        }
    }

    #[test]
    fn test_config_round_trip_preserves_diarizer_kind_override() {
        let mut original = Config::default();
        original.diarizer.kind = "pyannote-onnx".to_string();

        let encoded = toml::to_string(&original).unwrap();
        let decoded = Config::from_toml_str(&encoded, &fake_path()).unwrap();

        assert_eq!(decoded.diarizer.kind, "pyannote-onnx");
    }

    #[test]
    fn test_diarizer_config_validated_kind_accepts_canonical_values() {
        for value in ["auto", "binary-channel", "pyannote-onnx"] {
            let cfg = DiarizerConfig {
                kind: value.to_string(),
            };

            assert_eq!(cfg.validated_kind(), Some(value));
        }
    }

    #[test]
    fn test_diarizer_config_validated_kind_returns_none_for_unknown_value() {
        let cfg = DiarizerConfig {
            kind: "neural".to_string(),
        };

        assert!(cfg.validated_kind().is_none());
    }

    #[test]
    fn test_diarizer_config_validated_kind_returns_none_for_empty_string() {
        let cfg = DiarizerConfig {
            kind: String::new(),
        };

        assert!(cfg.validated_kind().is_none());
    }

    #[test]
    fn test_diarizer_config_validated_kind_is_case_sensitive() {
        let cfg = DiarizerConfig {
            kind: "Pyannote-Onnx".to_string(),
        };

        assert!(cfg.validated_kind().is_none());
    }

    #[test]
    fn test_diarizer_config_kind_constants_match_diarize_module_constants() {
        use crate::diarize::kind::{
            DIARIZER_KIND_BINARY_CHANNEL as KIND_BC, DIARIZER_KIND_PYANNOTE_ONNX as KIND_PY,
        };

        assert_eq!(DIARIZER_KIND_BINARY_CHANNEL, KIND_BC);
        assert_eq!(DIARIZER_KIND_PYANNOTE_ONNX, KIND_PY);
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
