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
use std::time::Duration;

#[cfg(feature = "stt-sherpa")]
use std::collections::HashMap;
#[cfg(feature = "stt-sherpa")]
use std::path::Path;

use async_trait::async_trait;

use crate::error::SttError;
use crate::providers::streaming::{StreamingSttProvider, StreamingUpdate};
use crate::providers::SttProvider;
use crate::types::{AudioChunk, FrameSource, TranscriptChunk};

#[cfg(feature = "stt-sherpa")]
use crate::pipeline::normalize::STT_SAMPLE_RATE;
#[cfg(feature = "stt-sherpa")]
use crate::providers::streaming::{token_timings, StreamingStage};

#[cfg(feature = "stt-sherpa")]
use sherpa_onnx::{OnlineRecognizer, OnlineRecognizerConfig, OnlineStream};
#[cfg(feature = "stt-sherpa")]
use std::sync::Arc;
#[cfg(feature = "stt-sherpa")]
use tokio::sync::Mutex;

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
    /// One persistent recognition stream per capture source, opened on
    /// that source's first live audio and closed by `finalize`. The
    /// batch `SttProvider::transcribe` path never touches this map.
    #[cfg(feature = "stt-sherpa")]
    streams: Mutex<HashMap<FrameSource, SourceStream>>,
}

/// One source's open segment.
#[cfg(feature = "stt-sherpa")]
struct SourceStream {
    /// `Arc` so the decode step can run on a blocking thread without
    /// holding the map's lock guard across the await.
    stream: Arc<OnlineStream>,
    /// Session offset at which this segment's stream began; token
    /// timestamps are relative to it.
    segment_start_ms: u64,
    /// 16 kHz samples accepted into this segment.
    accepted_samples: u64,
    /// Last hypothesis reported, so an unchanged stream emits nothing.
    reported_text: String,
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
                streams: Mutex::new(HashMap::new()),
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
                let start_ms = duration_ms(chunk.start);
                let tokens = token_timings(&result.tokens, result.timestamps.as_deref(), start_ms)?;
                Ok(TranscriptChunk {
                    text: result.text.trim().to_string(),
                    source: chunk.source,
                    start_ms,
                    duration_ms: duration_ms(chunk.duration),
                    language: Some("en".to_string()),
                    tokens,
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

/// Milliseconds of `value`, saturating rather than wrapping.
#[cfg(feature = "stt-sherpa")]
fn duration_ms(value: Duration) -> u64 {
    u64::try_from(value.as_millis()).unwrap_or(u64::MAX)
}

/// One recognizer hypothesis, decoded off the async runtime.
#[cfg(feature = "stt-sherpa")]
struct Decoded {
    text: String,
    tokens: Vec<String>,
    timestamps: Option<Vec<f32>>,
}

/// Push optional audio into `stream`, drain the decoder, and read the
/// current hypothesis.
///
/// Runs on a blocking thread: sherpa-onnx decoding is CPU-bound and
/// would otherwise stall the capture task. Both handles are `Arc` so no
/// map lock is held by the blocking closure.
#[cfg(feature = "stt-sherpa")]
async fn decode(
    recognizer: Arc<OnlineRecognizer>,
    stream: Arc<OnlineStream>,
    samples: Option<Arc<[f32]>>,
    input_finished: bool,
) -> Result<Decoded, SttError> {
    tokio::task::spawn_blocking(move || {
        if let Some(samples) = samples {
            #[allow(clippy::cast_possible_wrap, clippy::cast_possible_truncation)]
            stream.accept_waveform(STT_SAMPLE_RATE as i32, samples.as_ref());
        }
        if input_finished {
            stream.input_finished();
        }
        while recognizer.is_ready(&stream) {
            recognizer.decode(&stream);
        }
        let result = recognizer.get_result(&stream).ok_or_else(|| {
            SttError::Decoding(Box::new(std::io::Error::other(
                "sherpa-onnx returned no streaming result",
            )))
        })?;
        Ok(Decoded {
            text: result.text,
            tokens: result.tokens,
            timestamps: result.timestamps,
        })
    })
    .await
    .map_err(|error| SttError::Decoding(Box::new(error)))?
}

#[async_trait]
impl StreamingSttProvider for SherpaStreamingProvider {
    #[allow(unused_variables)]
    async fn accept(&self, audio: AudioChunk) -> Result<Option<StreamingUpdate>, SttError> {
        #[cfg(feature = "stt-sherpa")]
        {
            if audio.samples.is_empty() {
                return Ok(None);
            }
            // The lock is held across the decode so a concurrent
            // `finalize` cannot close the segment mid-decode, and is
            // released as soon as the segment state has been updated.
            let mut streams = self.streams.lock().await;
            let state = streams.entry(audio.source).or_insert_with(|| SourceStream {
                stream: Arc::new(self.recognizer.create_stream()),
                segment_start_ms: duration_ms(audio.start),
                accepted_samples: 0,
                reported_text: String::new(),
            });
            let decoded = decode(
                Arc::clone(&self.recognizer),
                Arc::clone(&state.stream),
                Some(Arc::clone(&audio.samples)),
                false,
            )
            .await?;
            state.accepted_samples = state
                .accepted_samples
                .saturating_add(audio.samples.len() as u64);
            let text = decoded.text.trim().to_string();
            let grew = text != state.reported_text;
            if grew {
                state.reported_text.clone_from(&text);
            }
            let segment_start_ms = state.segment_start_ms;
            let accepted_samples = state.accepted_samples;
            drop(streams);

            if !grew {
                // Nothing new was recognized; do not re-write the WAL.
                return Ok(None);
            }
            let tokens = token_timings(
                &decoded.tokens,
                decoded.timestamps.as_deref(),
                segment_start_ms,
            )?;
            return Ok(Some(StreamingUpdate {
                stage: StreamingStage::Partial,
                chunk: TranscriptChunk {
                    text,
                    source: audio.source,
                    start_ms: segment_start_ms,
                    duration_ms: accepted_samples * 1_000 / u64::from(STT_SAMPLE_RATE),
                    language: Some("en".to_string()),
                    tokens,
                },
            }));
        }

        #[cfg(not(feature = "stt-sherpa"))]
        {
            Err(SttError::ModelNotLoaded(format!(
                "scrybe-core was built without the `stt-sherpa` cargo feature; enable it to load {}",
                self.config.model_dir.display()
            )))
        }
    }

    #[allow(unused_variables)]
    async fn finalize(
        &self,
        source: FrameSource,
        start: Duration,
        duration: Duration,
    ) -> Result<Option<StreamingUpdate>, SttError> {
        #[cfg(feature = "stt-sherpa")]
        {
            // Removing the state closes the segment: the stream handle
            // drops with this call, and a second `finalize` for the same
            // segment finds nothing and returns `None`.
            let state = {
                let mut streams = self.streams.lock().await;
                streams.remove(&source)
            };
            let Some(state) = state else {
                return Ok(None);
            };
            if state.accepted_samples == 0 {
                return Ok(None);
            }
            let decoded = decode(
                Arc::clone(&self.recognizer),
                Arc::clone(&state.stream),
                None,
                true,
            )
            .await?;
            let tokens = token_timings(
                &decoded.tokens,
                decoded.timestamps.as_deref(),
                state.segment_start_ms,
            )?;
            return Ok(Some(StreamingUpdate {
                stage: StreamingStage::Final,
                chunk: TranscriptChunk {
                    text: decoded.text.trim().to_string(),
                    source,
                    start_ms: duration_ms(start),
                    duration_ms: duration_ms(duration),
                    language: Some("en".to_string()),
                    tokens,
                },
            }));
        }

        #[cfg(not(feature = "stt-sherpa"))]
        {
            Err(SttError::ModelNotLoaded(format!(
                "scrybe-core was built without the `stt-sherpa` cargo feature; enable it to load {}",
                self.config.model_dir.display()
            )))
        }
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
