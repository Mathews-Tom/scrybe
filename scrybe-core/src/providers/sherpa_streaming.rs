// Copyright 2026 Mathews Tom
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//! `SherpaStreamingProvider` — streaming Zipformer ASR via the official
//! `sherpa-onnx` Rust binding.
//!
//! The `stt-sherpa` feature is opt-in because it links a pre-provisioned native
//! runtime. The workspace Cargo configuration makes an absent runtime fail
//! before `sherpa-onnx-sys` can download one. This provider accepts only the
//! pinned streaming Zipformer artifact set; partial, missing, or wrong-sized
//! files fail at construction.

use std::path::PathBuf;

#[cfg(feature = "stt-sherpa")]
use std::path::Path;

use async_trait::async_trait;

use crate::error::SttError;
use crate::providers::SttProvider;
use crate::types::{AudioChunk, TranscriptChunk};

#[cfg(feature = "stt-sherpa")]
use sherpa_onnx::{OnlineRecognizer, OnlineRecognizerConfig};
#[cfg(feature = "stt-sherpa")]
use std::sync::Arc;

const MODEL_LABEL: &str = "zipformer-en-2023-06-26";
#[cfg(feature = "stt-sherpa")]
const ENCODER_FILE: &str = "encoder-epoch-99-avg-1-chunk-16-left-128.int8.onnx";
#[cfg(feature = "stt-sherpa")]
const DECODER_FILE: &str = "decoder-epoch-99-avg-1-chunk-16-left-128.onnx";
#[cfg(feature = "stt-sherpa")]
const JOINER_FILE: &str = "joiner-epoch-99-avg-1-chunk-16-left-128.int8.onnx";
#[cfg(feature = "stt-sherpa")]
const TOKENS_FILE: &str = "tokens.txt";
#[cfg(feature = "stt-sherpa")]
const MODEL_FILES: [(&str, u64); 4] = [
    (ENCODER_FILE, 71_083_163),
    (DECODER_FILE, 2_092_621),
    (JOINER_FILE, 259_335),
    (TOKENS_FILE, 5_048),
];

/// Configuration for [`SherpaStreamingProvider`].
#[derive(Clone, Debug)]
pub struct SherpaStreamingConfig {
    /// Directory containing the pinned streaming Zipformer artifacts.
    pub model_dir: PathBuf,
    /// Number of CPU threads given to sherpa-onnx.
    pub num_threads: i32,
    /// Display label embedded in the provider name.
    pub model_label: String,
}

impl SherpaStreamingConfig {
    /// Construct configuration for the pinned English streaming Zipformer model.
    #[must_use]
    pub fn new(model_dir: PathBuf) -> Self {
        Self {
            model_dir,
            num_threads: 2,
            model_label: MODEL_LABEL.to_string(),
        }
    }
}

/// Local streaming Sherpa-ONNX provider.
///
/// The type exists in every build. Constructing it without `stt-sherpa` fails
/// explicitly rather than silently selecting the stub provider.
pub struct SherpaStreamingProvider {
    config: SherpaStreamingConfig,
    name: String,
    #[cfg(feature = "stt-sherpa")]
    recognizer: Arc<OnlineRecognizer>,
}

impl SherpaStreamingProvider {
    /// Create a provider after validating every pinned model artifact.
    ///
    /// # Errors
    ///
    /// Returns [`SttError::ModelCorrupt`] for partial or wrong-sized artifacts,
    /// [`SttError::ModelNotLoaded`] for a missing feature or unreadable model,
    /// and [`SttError::Decoding`] when sherpa-onnx rejects the model set.
    #[cfg_attr(not(feature = "stt-sherpa"), allow(clippy::needless_pass_by_value))]
    pub fn new(config: SherpaStreamingConfig) -> Result<Self, SttError> {
        #[cfg(feature = "stt-sherpa")]
        {
            let paths = validate_model_files(&config.model_dir)?;
            let mut recognizer_config = OnlineRecognizerConfig::default();
            recognizer_config.model_config.transducer.encoder =
                Some(paths.encoder.to_string_lossy().into_owned());
            recognizer_config.model_config.transducer.decoder =
                Some(paths.decoder.to_string_lossy().into_owned());
            recognizer_config.model_config.transducer.joiner =
                Some(paths.joiner.to_string_lossy().into_owned());
            recognizer_config.model_config.tokens =
                Some(paths.tokens.to_string_lossy().into_owned());
            recognizer_config.model_config.num_threads = config.num_threads;
            recognizer_config.decoding_method = Some("greedy_search".to_string());

            let recognizer = OnlineRecognizer::create(&recognizer_config).ok_or_else(|| {
                SttError::Decoding(Box::new(std::io::Error::other(format!(
                    "sherpa-onnx rejected model directory {}",
                    config.model_dir.display()
                ))))
            })?;
            let name = format!("sherpa-streaming:{}", config.model_label);
            Ok(Self {
                config,
                name,
                recognizer: Arc::new(recognizer),
            })
        }

        #[cfg(not(feature = "stt-sherpa"))]
        {
            Err(SttError::ModelNotLoaded(format!(
                "scrybe-core was built without the `stt-sherpa` cargo feature; enable it to load {}",
                config.model_dir.display()
            )))
        }
    }

    #[must_use]
    pub const fn config(&self) -> &SherpaStreamingConfig {
        &self.config
    }
}

#[cfg(feature = "stt-sherpa")]
struct ModelPaths {
    encoder: PathBuf,
    decoder: PathBuf,
    joiner: PathBuf,
    tokens: PathBuf,
}

#[cfg(feature = "stt-sherpa")]
fn validate_model_files(model_dir: &Path) -> Result<ModelPaths, SttError> {
    if is_partial(model_dir) {
        return Err(SttError::ModelCorrupt {
            path: model_dir.to_path_buf(),
        });
    }

    for (file_name, expected_size) in MODEL_FILES {
        let path = model_dir.join(file_name);
        if is_partial(&path)
            || std::fs::metadata(&path).map_or(true, |metadata| {
                !metadata.is_file() || metadata.len() != expected_size
            })
        {
            return Err(SttError::ModelCorrupt { path });
        }
    }

    Ok(ModelPaths {
        encoder: model_dir.join(ENCODER_FILE),
        decoder: model_dir.join(DECODER_FILE),
        joiner: model_dir.join(JOINER_FILE),
        tokens: model_dir.join(TOKENS_FILE),
    })
}

#[cfg(feature = "stt-sherpa")]
fn is_partial(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case("partial"))
}

#[async_trait]
impl SttProvider for SherpaStreamingProvider {
    #[allow(unused_variables)]
    async fn transcribe(&self, chunk: AudioChunk) -> Result<TranscriptChunk, SttError> {
        #[cfg(feature = "stt-sherpa")]
        {
            let recognizer = Arc::clone(&self.recognizer);
            return tokio::task::spawn_blocking(move || {
                let stream = recognizer.create_stream();
                stream.accept_waveform(16_000, chunk.samples.as_ref());
                stream.input_finished();
                while recognizer.is_ready(&stream) {
                    recognizer.decode(&stream);
                }
                let result = recognizer.get_result(&stream).ok_or_else(|| {
                    SttError::Decoding(Box::new(std::io::Error::other(
                        "sherpa-onnx returned no transcript result",
                    )))
                })?;
                Ok(TranscriptChunk {
                    text: result.text.trim().to_string(),
                    source: chunk.source,
                    start_ms: u64::try_from(chunk.start.as_millis()).unwrap_or(u64::MAX),
                    duration_ms: u64::try_from(chunk.duration.as_millis()).unwrap_or(u64::MAX),
                    language: Some("en".to_string()),
                })
            })
            .await
            .map_err(|error| SttError::Decoding(Box::new(error)))?;
        }

        #[cfg(not(feature = "stt-sherpa"))]
        {
            Err(SttError::ModelNotLoaded(format!(
                "scrybe-core was built without the `stt-sherpa` cargo feature; enable it to load {}",
                self.config.model_dir.display()
            )))
        }
    }

    fn name(&self) -> &str {
        &self.name
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    #[test]
    fn test_sherpa_streaming_config_uses_pinned_model_label() {
        let config = SherpaStreamingConfig::new(PathBuf::from("/models/zipformer"));

        assert_eq!(config.model_label, MODEL_LABEL);
        assert_eq!(config.num_threads, 2);
    }

    #[cfg(not(feature = "stt-sherpa"))]
    #[test]
    fn test_provider_rejects_requested_model_without_feature() {
        let Err(error) = SherpaStreamingProvider::new(SherpaStreamingConfig::new(PathBuf::from(
            "/models/zipformer",
        ))) else {
            panic!("missing feature must reject the requested model");
        };

        assert!(matches!(error, SttError::ModelNotLoaded(_)));
        assert!(error.to_string().contains("stt-sherpa"));
    }

    #[cfg(feature = "stt-sherpa")]
    #[test]
    fn test_provider_rejects_wrong_sized_model_artifact_before_runtime_load() {
        let dir = tempfile::tempdir().unwrap();
        let Err(error) =
            SherpaStreamingProvider::new(SherpaStreamingConfig::new(dir.path().to_path_buf()))
        else {
            panic!("wrong-sized artifacts must fail before runtime construction");
        };

        assert!(matches!(error, SttError::ModelCorrupt { .. }));
    }
}
