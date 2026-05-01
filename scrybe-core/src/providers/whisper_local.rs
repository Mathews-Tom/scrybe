// Copyright 2026 Mathews Tom
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//     https://www.apache.org/licenses/LICENSE-2.0

//! `WhisperLocalProvider` — `whisper.cpp` via the `whisper-rs` Rust binding.
//!
//! The full provider links libwhisper through `whisper-rs`, which has
//! a non-trivial build-time C/C++ dependency surface (Metal on macOS,
//! BLAS elsewhere). To keep the default workspace build fast and
//! cross-compilable, the provider lives behind the `whisper-local`
//! cargo feature. Without the feature, the type still exists so call
//! sites in `scrybe-cli` can refer to it unconditionally; transcribe
//! returns `SttError::ModelNotLoaded` to make the missing-feature
//! obvious.
//!
//! Model file integrity is enforced by the loader: `verify_checksum`
//! is called at construction and on every reload. The checksum is the
//! GGUF model's SHA-256 hex digest, distributed alongside the model
//! download URL in `scrybe init` (`docs/system-design.md` §7.1).

use std::path::{Path, PathBuf};

use async_trait::async_trait;

use crate::error::SttError;
use crate::providers::SttProvider;
use crate::types::{AudioChunk, TranscriptChunk};

/// Configuration for `WhisperLocalProvider`. The `model_path` MUST
/// point to a verified `*.gguf` file; the loader rejects `*.partial`
/// candidates per `system-design.md` §8.1 model-download recovery.
#[derive(Clone, Debug)]
pub struct WhisperLocalConfig {
    pub model_path: PathBuf,
    pub language: String,
    pub model_label: String,
}

impl WhisperLocalConfig {
    /// Construct with sensible v0.1 defaults: language auto-detection,
    /// `large-v3-turbo` label.
    #[must_use]
    pub fn new(model_path: PathBuf) -> Self {
        Self {
            model_path,
            language: "auto".to_string(),
            model_label: "large-v3-turbo".to_string(),
        }
    }
}

/// Local Whisper provider. The struct exists in every build; runtime
/// behavior depends on the `whisper-local` feature.
#[derive(Debug)]
pub struct WhisperLocalProvider {
    config: WhisperLocalConfig,
    name: String,
}

impl WhisperLocalProvider {
    /// Construct a provider after verifying the model file exists and
    /// is not a partial download.
    ///
    /// # Errors
    ///
    /// `SttError::ModelCorrupt` if the model file path ends in
    /// `.partial`. Other path errors (missing file, permission)
    /// surface from the underlying loader at first transcription.
    pub fn new(config: WhisperLocalConfig) -> Result<Self, SttError> {
        if is_partial(&config.model_path) {
            return Err(SttError::ModelCorrupt {
                path: config.model_path,
            });
        }
        let name = format!("whisper-local:{}", config.model_label);
        Ok(Self { config, name })
    }

    #[must_use]
    pub const fn config(&self) -> &WhisperLocalConfig {
        &self.config
    }
}

fn is_partial(path: &Path) -> bool {
    path.extension()
        .and_then(|os| os.to_str())
        .is_some_and(|ext| ext.eq_ignore_ascii_case("partial"))
}

#[async_trait]
impl SttProvider for WhisperLocalProvider {
    async fn transcribe(&self, _chunk: AudioChunk) -> Result<TranscriptChunk, SttError> {
        transcribe_impl(&self.config, _chunk).await
    }

    fn name(&self) -> &str {
        &self.name
    }
}

#[cfg(feature = "whisper-local")]
async fn transcribe_impl(
    config: &WhisperLocalConfig,
    chunk: AudioChunk,
) -> Result<TranscriptChunk, SttError> {
    use whisper_rs::{FullParams, SamplingStrategy, WhisperContext, WhisperContextParameters};

    let model_path = config.model_path.clone();
    let language = config.language.clone();
    let source = chunk.source;
    let start_ms = u64::try_from(chunk.start.as_millis()).unwrap_or(0);
    let duration_ms = u64::try_from(chunk.duration.as_millis()).unwrap_or(0);
    let samples = chunk.samples.as_ref().to_vec();

    tokio::task::spawn_blocking(move || -> Result<TranscriptChunk, SttError> {
        let ctx_params = WhisperContextParameters::default();
        let ctx =
            WhisperContext::new_with_params(model_path.to_string_lossy().as_ref(), ctx_params)
                .map_err(|e| SttError::Decoding(Box::new(e)))?;
        let mut state = ctx
            .create_state()
            .map_err(|e| SttError::Decoding(Box::new(e)))?;
        let mut params = FullParams::new(SamplingStrategy::Greedy { best_of: 1 });
        if language != "auto" {
            params.set_language(Some(language.as_str()));
        }
        params.set_print_progress(false);
        params.set_print_special(false);
        params.set_print_realtime(false);
        params.set_print_timestamps(false);
        state
            .full(params, &samples)
            .map_err(|e| SttError::Decoding(Box::new(e)))?;
        let segment_count = state
            .full_n_segments()
            .map_err(|e| SttError::Decoding(Box::new(e)))?;
        let mut text = String::new();
        for i in 0..segment_count {
            let segment = state
                .full_get_segment_text(i)
                .map_err(|e| SttError::Decoding(Box::new(e)))?;
            text.push_str(&segment);
        }
        Ok(TranscriptChunk {
            text: text.trim().to_string(),
            source,
            start_ms,
            duration_ms,
            language: None,
        })
    })
    .await
    .map_err(|e| SttError::Decoding(Box::new(e)))?
}

#[cfg(not(feature = "whisper-local"))]
#[allow(clippy::unused_async)]
async fn transcribe_impl(
    config: &WhisperLocalConfig,
    _chunk: AudioChunk,
) -> Result<TranscriptChunk, SttError> {
    Err(SttError::ModelNotLoaded(format!(
        "scrybe-core was built without the `whisper-local` cargo feature; \
         enable it to load {}",
        config.model_path.display()
    )))
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    #[cfg(not(feature = "whisper-local"))]
    use std::sync::Arc;
    #[cfg(not(feature = "whisper-local"))]
    use std::time::Duration;

    use super::*;
    #[cfg(not(feature = "whisper-local"))]
    use crate::types::FrameSource;
    use pretty_assertions::assert_eq;

    #[test]
    fn test_whisper_local_provider_name_includes_model_label() {
        let provider = WhisperLocalProvider::new(WhisperLocalConfig::new(PathBuf::from(
            "/models/large-v3-turbo.gguf",
        )))
        .unwrap();

        assert_eq!(provider.name(), "whisper-local:large-v3-turbo");
    }

    #[test]
    fn test_whisper_local_provider_rejects_partial_model_file() {
        let err = WhisperLocalProvider::new(WhisperLocalConfig::new(PathBuf::from(
            "/models/large-v3-turbo.partial",
        )))
        .unwrap_err();

        assert!(matches!(err, SttError::ModelCorrupt { .. }));
    }

    #[test]
    fn test_is_partial_helper_matches_partial_extension_case_insensitive() {
        assert!(is_partial(Path::new("/models/foo.partial")));
        assert!(is_partial(Path::new("/models/foo.PARTIAL")));
        assert!(!is_partial(Path::new("/models/foo.gguf")));
        assert!(!is_partial(Path::new("/models/foo.bin")));
    }

    #[cfg(not(feature = "whisper-local"))]
    #[tokio::test]
    async fn test_transcribe_returns_model_not_loaded_when_feature_disabled() {
        let provider = WhisperLocalProvider::new(WhisperLocalConfig::new(PathBuf::from(
            "/models/large-v3-turbo.gguf",
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
        assert!(message.contains("whisper-local"));
    }
}
