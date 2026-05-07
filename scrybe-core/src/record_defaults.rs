// Copyright 2026 Mathews Tom
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//     https://www.apache.org/licenses/LICENSE-2.0

//! Ergonomic-default resolution for the `scrybe record <title>`
//! subcommand.
//!
//! Pulls values from the on-disk config first, then falls back to
//! platform-aware defaults that pick the most common useful
//! configuration:
//!
//! - **Source**: `mic+system` on macOS, `mic` elsewhere. The ergonomic
//!   command is meant for capturing both sides of a conversation;
//!   mic-only is the fallback when a system tap isn't available.
//! - **Whisper model**: `ggml-medium.en.bin` at the platform
//!   project-data path, but only if it exists on disk. The caller
//!   surfaces a clear setup error when the file is missing rather
//!   than letting the session fail at first use.
//! - **LLM**: `openai-compat` if a TCP connect to `127.0.0.1:11434`
//!   succeeds within 100 ms (typically a local Ollama), otherwise the
//!   stub provider so the session still completes with a templated
//!   `notes.md`.
//!
//! These resolvers observe environment state (file existence, TCP
//! reachability) but do not mutate it. The schema-default values in
//! `config::RecordConfig` remain `synthetic` / `stub` so existing
//! `scrybe rec` invocations and bare `scrybe record` semantics with a
//! schema-default config produce identical behavior to v1.0.4 for
//! users who haven't customized their config or invoked the new
//! ergonomic entry point.

use std::net::{Shutdown, TcpStream, ToSocketAddrs};
use std::path::PathBuf;
use std::time::Duration;

use directories::ProjectDirs;

use crate::config::{
    RecordConfig, RECORD_LLM_OPENAI_COMPAT, RECORD_LLM_STUB, RECORD_SOURCE_MIC,
    RECORD_SOURCE_MIC_SYSTEM, RECORD_SOURCE_SYNTHETIC,
};

const OLLAMA_PROBE_HOST: &str = "127.0.0.1";
const OLLAMA_PROBE_PORT: u16 = 11434;
const OLLAMA_PROBE_TIMEOUT: Duration = Duration::from_millis(100);
const DEFAULT_WHISPER_MODEL_FILENAME: &str = "ggml-medium.en.bin";

/// Resolve the Whisper model path for the ergonomic record command.
///
/// Returns `cfg.whisper_model` if set; otherwise the platform default
/// path only if the file exists on disk. Returns `None` if neither
/// source produces a usable path; the caller surfaces a setup error.
#[must_use]
pub fn ergonomic_whisper_model(cfg: &RecordConfig) -> Option<PathBuf> {
    if let Some(path) = &cfg.whisper_model {
        return Some(path.clone());
    }
    let path = default_whisper_model_path()?;
    path.exists().then_some(path)
}

/// Platform-default Whisper model path regardless of file existence.
/// Used by `scrybe setup` to know where to download the model to.
#[must_use]
pub fn default_whisper_model_path() -> Option<PathBuf> {
    let dirs = ProjectDirs::from("dev", "scrybe", "scrybe")?;
    Some(
        dirs.data_dir()
            .join("models")
            .join(DEFAULT_WHISPER_MODEL_FILENAME),
    )
}

/// Resolve the capture source for the ergonomic record command.
///
/// Honors explicit non-synthetic config values; otherwise picks the
/// platform default (mic+system on macOS, mic elsewhere).
#[must_use]
pub fn ergonomic_source(cfg: &RecordConfig) -> &'static str {
    if let Some(s) = cfg.validated_source() {
        if s != RECORD_SOURCE_SYNTHETIC {
            return s;
        }
    }
    platform_default_source()
}

/// Platform-default capture source for the ergonomic record command.
#[must_use]
pub const fn platform_default_source() -> &'static str {
    if cfg!(target_os = "macos") {
        RECORD_SOURCE_MIC_SYSTEM
    } else {
        RECORD_SOURCE_MIC
    }
}

/// Resolve the LLM kind for the ergonomic record command.
///
/// Honors explicit non-stub config values; otherwise probes the
/// canonical Ollama endpoint and selects `openai-compat` if reachable,
/// falling back to the stub provider so the session still completes.
#[must_use]
pub fn ergonomic_llm(cfg: &RecordConfig) -> &'static str {
    if let Some(l) = cfg.validated_llm() {
        if l != RECORD_LLM_STUB {
            return l;
        }
    }
    if probe_ollama_local() {
        RECORD_LLM_OPENAI_COMPAT
    } else {
        RECORD_LLM_STUB
    }
}

fn probe_ollama_local() -> bool {
    let Ok(mut addrs) = (OLLAMA_PROBE_HOST, OLLAMA_PROBE_PORT).to_socket_addrs() else {
        return false;
    };
    let Some(addr) = addrs.next() else {
        return false;
    };
    TcpStream::connect_timeout(&addr, OLLAMA_PROBE_TIMEOUT).is_ok_and(|stream| {
        let _ = stream.shutdown(Shutdown::Both);
        true
    })
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use crate::config::RecordConfig;
    use std::path::Path;

    #[test]
    fn test_ergonomic_whisper_model_returns_explicit_config_value() {
        let cfg = RecordConfig {
            whisper_model: Some(PathBuf::from("/explicit/path/model.bin")),
            ..RecordConfig::default()
        };
        assert_eq!(
            ergonomic_whisper_model(&cfg).as_deref(),
            Some(Path::new("/explicit/path/model.bin"))
        );
    }

    #[test]
    fn test_ergonomic_whisper_model_returns_none_or_existing_path_when_unset() {
        let cfg = RecordConfig {
            whisper_model: None,
            ..RecordConfig::default()
        };
        if let Some(path) = ergonomic_whisper_model(&cfg) {
            assert!(
                path.exists(),
                "resolver returned non-existent path {path:?}"
            );
        }
    }

    #[test]
    fn test_default_whisper_model_path_lands_under_bundle_id_dir() {
        let path = default_whisper_model_path().expect("ProjectDirs should resolve");
        assert!(
            path.to_string_lossy().contains("dev.scrybe.scrybe")
                || path.to_string_lossy().contains("scrybe"),
            "path does not contain bundle id or scrybe segment: {path:?}"
        );
        assert!(path.ends_with(DEFAULT_WHISPER_MODEL_FILENAME));
    }

    #[test]
    fn test_ergonomic_source_honors_explicit_mic_system() {
        let cfg = RecordConfig {
            source: RECORD_SOURCE_MIC_SYSTEM.to_string(),
            ..RecordConfig::default()
        };
        assert_eq!(ergonomic_source(&cfg), RECORD_SOURCE_MIC_SYSTEM);
    }

    #[test]
    fn test_ergonomic_source_honors_explicit_mic() {
        let cfg = RecordConfig {
            source: RECORD_SOURCE_MIC.to_string(),
            ..RecordConfig::default()
        };
        assert_eq!(ergonomic_source(&cfg), RECORD_SOURCE_MIC);
    }

    #[test]
    fn test_ergonomic_source_falls_back_to_platform_default_for_synthetic() {
        let cfg = RecordConfig {
            source: RECORD_SOURCE_SYNTHETIC.to_string(),
            ..RecordConfig::default()
        };
        assert_eq!(ergonomic_source(&cfg), platform_default_source());
    }

    #[test]
    fn test_ergonomic_source_falls_back_to_platform_default_for_unknown_value() {
        let cfg = RecordConfig {
            source: "garbage-value".to_string(),
            ..RecordConfig::default()
        };
        assert_eq!(ergonomic_source(&cfg), platform_default_source());
    }

    #[test]
    fn test_platform_default_source_matches_target_os() {
        if cfg!(target_os = "macos") {
            assert_eq!(platform_default_source(), RECORD_SOURCE_MIC_SYSTEM);
        } else {
            assert_eq!(platform_default_source(), RECORD_SOURCE_MIC);
        }
    }

    #[test]
    fn test_ergonomic_llm_honors_explicit_openai_compat() {
        let cfg = RecordConfig {
            llm: RECORD_LLM_OPENAI_COMPAT.to_string(),
            ..RecordConfig::default()
        };
        assert_eq!(ergonomic_llm(&cfg), RECORD_LLM_OPENAI_COMPAT);
    }

    #[test]
    fn test_ergonomic_llm_returns_validated_value_on_stub_with_probe_outcome() {
        let cfg = RecordConfig {
            llm: RECORD_LLM_STUB.to_string(),
            ..RecordConfig::default()
        };
        let result = ergonomic_llm(&cfg);
        assert!(
            result == RECORD_LLM_STUB || result == RECORD_LLM_OPENAI_COMPAT,
            "ergonomic_llm returned unexpected value: {result:?}"
        );
    }
}
