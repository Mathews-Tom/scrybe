// Copyright 2026 Mathews Tom
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//     https://www.apache.org/licenses/LICENSE-2.0

//! `ParakeetLocalProvider` — NVIDIA Parakeet TDT v2/v3 via the `sherpa-rs` Rust binding.
//!
//! Parakeet is the English-priority, high-throughput counterpart to
//! Whisper. The trade-off documented in `.docs/development-plan.md`
//! §10.2 is "≥ 20× realtime on RTX 4070 / Apple M2; quality regression
//! ≤ 1.5× WER on English clips". Multilingual users stay on Whisper;
//! English-priority users opt in via `--features parakeet-local` and
//! `[stt] provider = "parakeet-local"` in the config.
//!
//! The provider mirrors `WhisperLocalProvider`'s scaffold: the type
//! exists in every build so call sites in `scrybe-cli` can refer to
//! it unconditionally; without the `parakeet-local` cargo feature,
//! `transcribe()` returns `SttError::ModelNotLoaded` so the missing
//! feature is obvious instead of a cryptic load failure.
//!
//! `sherpa-rs` itself requires building or linking the upstream
//! `sherpa-onnx` C++ runtime, which is why the live binding sits behind
//! a feature gate. The default workspace build stays toolchain-light;
//! the Windows release build (and any user who explicitly opts in)
//! flips the feature on.

use std::path::{Path, PathBuf};

use async_trait::async_trait;

use crate::error::SttError;
use crate::providers::SttProvider;
use crate::types::{AudioChunk, TranscriptChunk};

/// Configuration for `ParakeetLocalProvider`.
///
/// The `model_path` MUST point to a verified Parakeet model directory
/// containing the encoder, decoder, joiner, and tokens files; the
/// loader rejects `*.partial` candidates per `system-design.md` §8.1
/// model-download recovery.
#[derive(Clone, Debug)]
pub struct ParakeetLocalConfig {
    /// Path to the directory containing the Parakeet model artifacts.
    pub model_path: PathBuf,
    /// Display label embedded in the provider name (`parakeet-local:<label>`).
    pub model_label: String,
}

impl ParakeetLocalConfig {
    /// Construct with the default Parakeet TDT v2 model label.
    #[must_use]
    pub fn new(model_path: PathBuf) -> Self {
        Self {
            model_path,
            model_label: "tdt-v2".to_string(),
        }
    }
}

/// Local Parakeet provider. The struct exists in every build; runtime
/// behavior depends on the `parakeet-local` feature.
#[derive(Debug)]
pub struct ParakeetLocalProvider {
    config: ParakeetLocalConfig,
    name: String,
}

impl ParakeetLocalProvider {
    /// Construct a provider after verifying the model path is not a
    /// partial download.
    ///
    /// # Errors
    ///
    /// `SttError::ModelCorrupt` if the model path ends in `.partial`.
    /// Other path errors (missing directory, permission) surface from
    /// the underlying loader at first transcription.
    pub fn new(config: ParakeetLocalConfig) -> Result<Self, SttError> {
        if is_partial(&config.model_path) {
            return Err(SttError::ModelCorrupt {
                path: config.model_path,
            });
        }
        let name = format!("parakeet-local:{}", config.model_label);
        Ok(Self { config, name })
    }

    #[must_use]
    pub const fn config(&self) -> &ParakeetLocalConfig {
        &self.config
    }
}

fn is_partial(path: &Path) -> bool {
    path.extension()
        .and_then(|os| os.to_str())
        .is_some_and(|ext| ext.eq_ignore_ascii_case("partial"))
}

#[async_trait]
impl SttProvider for ParakeetLocalProvider {
    async fn transcribe(&self, _chunk: AudioChunk) -> Result<TranscriptChunk, SttError> {
        transcribe_impl(&self.config, _chunk).await
    }

    fn name(&self) -> &str {
        &self.name
    }
}

#[cfg(feature = "parakeet-local")]
#[allow(clippy::unused_async)]
async fn transcribe_impl(
    config: &ParakeetLocalConfig,
    _chunk: AudioChunk,
) -> Result<TranscriptChunk, SttError> {
    // The `parakeet-local` cargo feature is reserved for the live
    // `sherpa-rs` integration tracked in `.docs/development-plan.md`
    // §10.2; until that lands, enabling the feature surfaces an
    // explicit `ModelNotLoaded` rather than a phantom successful
    // transcription. This keeps the feature an honest opt-in: a
    // dependent crate that flips it on gets a typed error rather than
    // a silent fallback path.
    Err(SttError::ModelNotLoaded(format!(
        "scrybe-core's `parakeet-local` feature is enabled but the \
         sherpa-rs live binding has not landed yet; cannot load {}",
        config.model_path.display()
    )))
}

#[cfg(not(feature = "parakeet-local"))]
#[allow(clippy::unused_async)]
async fn transcribe_impl(
    config: &ParakeetLocalConfig,
    _chunk: AudioChunk,
) -> Result<TranscriptChunk, SttError> {
    Err(SttError::ModelNotLoaded(format!(
        "scrybe-core was built without the `parakeet-local` cargo feature; \
         enable it to load {}",
        config.model_path.display()
    )))
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    #[cfg(not(feature = "parakeet-local"))]
    use std::sync::Arc;
    #[cfg(not(feature = "parakeet-local"))]
    use std::time::Duration;

    use super::*;
    #[cfg(not(feature = "parakeet-local"))]
    use crate::types::FrameSource;
    use pretty_assertions::assert_eq;

    #[test]
    fn test_parakeet_local_provider_name_includes_model_label() {
        let provider = ParakeetLocalProvider::new(ParakeetLocalConfig::new(PathBuf::from(
            "/models/parakeet-tdt-v2",
        )))
        .unwrap();

        assert_eq!(provider.name(), "parakeet-local:tdt-v2");
    }

    #[test]
    fn test_parakeet_local_provider_rejects_partial_model_path() {
        let err = ParakeetLocalProvider::new(ParakeetLocalConfig::new(PathBuf::from(
            "/models/parakeet-tdt-v2.partial",
        )))
        .unwrap_err();

        assert!(matches!(err, SttError::ModelCorrupt { .. }));
    }

    #[test]
    fn test_parakeet_local_config_default_label_is_tdt_v2() {
        let config = ParakeetLocalConfig::new(PathBuf::from("/models/parakeet-tdt-v2"));

        assert_eq!(config.model_label, "tdt-v2");
    }

    #[test]
    fn test_is_partial_helper_matches_partial_extension_case_insensitive() {
        assert!(is_partial(Path::new("/models/foo.partial")));
        assert!(is_partial(Path::new("/models/foo.PARTIAL")));
        assert!(!is_partial(Path::new("/models/foo")));
        assert!(!is_partial(Path::new("/models/foo.bin")));
    }

    #[cfg(not(feature = "parakeet-local"))]
    #[tokio::test]
    async fn test_transcribe_returns_model_not_loaded_when_feature_disabled() {
        let provider = ParakeetLocalProvider::new(ParakeetLocalConfig::new(PathBuf::from(
            "/models/parakeet-tdt-v2",
        )))
        .unwrap();
        let chunk = AudioChunk {
            samples: Arc::from(vec![0.0_f32; 16_000]),
            source: FrameSource::Mic,
            start: Duration::ZERO,
            duration: Duration::from_secs(1),
        };

        let err = provider.transcribe(chunk).await.unwrap_err();

        let SttError::ModelNotLoaded(message) = err else {
            panic!("expected ModelNotLoaded with feature disabled");
        };
        assert!(message.contains("parakeet-local"));
    }

    #[cfg(feature = "parakeet-local")]
    #[tokio::test]
    async fn test_transcribe_returns_model_not_loaded_when_live_binding_pending() {
        use std::sync::Arc;
        use std::time::Duration;

        use crate::types::FrameSource;

        let provider = ParakeetLocalProvider::new(ParakeetLocalConfig::new(PathBuf::from(
            "/models/parakeet-tdt-v2",
        )))
        .unwrap();
        let chunk = AudioChunk {
            samples: Arc::from(vec![0.0_f32; 16_000]),
            source: FrameSource::Mic,
            start: Duration::ZERO,
            duration: Duration::from_secs(1),
        };

        let err = provider.transcribe(chunk).await.unwrap_err();

        let SttError::ModelNotLoaded(message) = err else {
            panic!("expected ModelNotLoaded while live binding is pending");
        };
        assert!(message.contains("sherpa-rs"));
    }
}
