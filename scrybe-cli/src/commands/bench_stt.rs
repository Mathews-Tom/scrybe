// Copyright 2026 Mathews Tom
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//     https://www.apache.org/licenses/LICENSE-2.0

//! `scrybe bench stt` — paired same-audio `whisper-local` vs `sherpa-onnx`
//! accuracy/latency bench over a `schema_version = 1` paired-corpus
//! manifest (`scrybe_core::testing::paired`).
//!
//! See `INSTALL.md`'s "Optional streaming Zipformer and English paired STT
//! benchmark" section for the full operator manual (native runtime
//! provisioning, model acquisition, corpus format, and result
//! interpretation). This module implements what that section documents; it
//! does not restate it.
//!
//! Corpus loading/validation happens before timing and is excluded from every
//! `provider_lifecycle_secs`. Each timed clip starts without a provider:
//! `provider_lifecycle_secs` wraps construction of a fresh backend plus its
//! single `SttProvider::transcribe` call. This is an explicit
//! cold-provider-per-clip contract for both backends, so neither backend
//! receives an initialization-lifecycle advantage. `MEASUREMENT_SCOPE`
//! records the contract in the JSON report.
//! CLI contract coverage (parsing, missing-feature/missing-runtime
//! rejection) lives in `scrybe-cli/tests/bench_stt.rs`, matching the
//! existing `tests/repair_sigkill.rs` integration-test convention.

#[cfg(feature = "stt-sherpa")]
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::time::Instant;

use anyhow::{bail, Context, Result};
use clap::Args;
use serde::Serialize;

#[cfg(feature = "stt-sherpa")]
use scrybe_core::providers::sherpa_streaming::{SherpaStreamingConfig, SherpaStreamingProvider};
#[cfg(feature = "whisper-local")]
use scrybe_core::providers::whisper_local::{WhisperLocalConfig, WhisperLocalProvider};
use scrybe_core::providers::SttProvider;
use scrybe_core::testing::paired::{
    aggregate, load_corpus, ClipMeasurement, PairedCorpus, PairedReport, BACKEND_SHERPA_ONNX,
    BACKEND_WHISPER_LOCAL,
};

/// CLI args for `scrybe bench stt`.
#[derive(Args, Debug, Clone)]
pub struct SttArgs {
    /// Paired-corpus TOML manifest (`schema_version = 1`; see
    /// `INSTALL.md`). Audio must already be 16 kHz mono PCM16 WAV.
    #[arg(long)]
    pub corpus: PathBuf,

    /// Whisper.cpp-compatible GGML model file (e.g. `ggml-base.en.bin`,
    /// not GGUF). Requires `--features whisper-local`.
    #[arg(long)]
    pub whisper_model: PathBuf,

    /// Directory holding the pinned streaming Zipformer model set.
    /// Requires `--features stt-sherpa`.
    #[arg(long)]
    pub sherpa_model: PathBuf,
}

/// Full `scrybe bench stt` output: the shared [`PairedReport`] plus the
/// model identities and timing caveats the report alone does not carry.
#[derive(Debug, Serialize)]
pub struct SttBenchOutput {
    pub corpus_manifest: PathBuf,
    pub whisper_model: PathBuf,
    pub sherpa_model_dir: PathBuf,
    pub sherpa_runtime_lib_dir: PathBuf,
    pub measurement_scope: MeasurementScope,
    pub report: PairedReport,
}

/// Machine-readable documentation of exactly what
/// `provider_lifecycle_secs` measures.
#[derive(Debug, Serialize)]
pub struct MeasurementScope {
    pub lifecycle: &'static str,
    pub corpus_loading_and_validation: &'static str,
    pub provider_lifecycle_secs: &'static str,
}

const MEASUREMENT_SCOPE: MeasurementScope = MeasurementScope {
    lifecycle: "cold-provider-per-clip",
    corpus_loading_and_validation: "excluded: paired::load_corpus parses, checksums, and format-validates every clip before measurement",
    provider_lifecycle_secs: "provider construction plus one SttProvider::transcribe call, repeated from a fresh provider for every clip and backend",
};

/// Run `scrybe bench stt`.
///
/// # Errors
///
/// Returns an explicit `anyhow::Error` for: either cargo feature missing,
/// an unprovisioned/missing/sentinel `SHERPA_ONNX_LIB_DIR`, corpus load or
/// validation failure, provider construction failure, any per-clip
/// `SttProvider::transcribe` failure, or `paired::aggregate` rejecting the
/// collected measurements. Every failure aborts before printing a report.
pub async fn run(args: SttArgs) -> Result<()> {
    #[cfg(not(feature = "whisper-local"))]
    require_whisper_feature()?;
    let sherpa_runtime_lib_dir = require_sherpa_runtime_lib_dir()?;
    let corpus = load_corpus(&args.corpus).with_context(|| {
        format!(
            "loading paired STT corpus manifest at {}",
            args.corpus.display()
        )
    })?;
    let mut measurements = measure_backend(
        || build_whisper_provider(&args.whisper_model),
        BACKEND_WHISPER_LOCAL,
        &corpus,
    )
    .await?;
    measurements.extend(
        measure_backend(
            || build_sherpa_provider(&args.sherpa_model),
            BACKEND_SHERPA_ONNX,
            &corpus,
        )
        .await?,
    );
    let report =
        aggregate(&corpus, measurements).context("aggregating paired STT bench measurements")?;
    let output = SttBenchOutput {
        corpus_manifest: args.corpus,
        whisper_model: args.whisper_model,
        sherpa_model_dir: args.sherpa_model,
        sherpa_runtime_lib_dir,
        measurement_scope: MEASUREMENT_SCOPE,
        report,
    };
    let json = serde_json::to_string_pretty(&output).context("serializing STT bench report")?;
    println!("{json}");
    Ok(())
}

/// Run every corpus clip through a freshly constructed provider. The timer
/// starts before construction and stops after transcription, enforcing the
/// cold-provider-per-clip contract recorded in [`MEASUREMENT_SCOPE`].
async fn measure_backend<F>(
    mut build_provider: F,
    backend: &str,
    corpus: &PairedCorpus,
) -> Result<Vec<ClipMeasurement>>
where
    F: FnMut() -> Result<Box<dyn SttProvider>>,
{
    let mut measurements = Vec::with_capacity(corpus.clips.len());
    for clip in &corpus.clips {
        let audio = clip.audio.clone();
        let started = Instant::now();
        let provider = build_provider().with_context(|| {
            format!(
                "{backend} cold provider initialization failed for clip {}",
                clip.id
            )
        })?;
        let transcript = provider
            .transcribe(audio)
            .await
            .with_context(|| format!("{backend} transcribe failed for clip {}", clip.id))?;
        let provider_lifecycle_secs = started.elapsed().as_secs_f64();
        measurements.push(ClipMeasurement {
            clip_id: clip.id.clone(),
            backend: backend.to_string(),
            hypothesis: transcript.text,
            provider_lifecycle_secs,
        });
    }
    Ok(measurements)
}

#[cfg(not(feature = "whisper-local"))]
fn require_whisper_feature() -> Result<()> {
    bail!(
        "`scrybe bench stt` requires the `whisper-local` cargo feature; rebuild with \
         `--features whisper-local,stt-sherpa`."
    );
}

fn build_whisper_provider(model_path: &Path) -> Result<Box<dyn SttProvider>> {
    #[cfg(feature = "whisper-local")]
    {
        let provider = WhisperLocalProvider::new(WhisperLocalConfig::new(model_path.to_path_buf()))
            .with_context(|| format!("loading whisper.cpp model at {}", model_path.display()))?;
        Ok(Box::new(provider))
    }
    #[cfg(not(feature = "whisper-local"))]
    {
        let _ = model_path;
        bail!(
            "`scrybe bench stt` requires the `whisper-local` cargo feature; rebuild with \
             `--features whisper-local,stt-sherpa`."
        );
    }
}

fn build_sherpa_provider(model_dir: &Path) -> Result<Box<dyn SttProvider>> {
    #[cfg(feature = "stt-sherpa")]
    {
        let provider =
            SherpaStreamingProvider::new(SherpaStreamingConfig::new(model_dir.to_path_buf()))
                .with_context(|| {
                    format!("loading sherpa-onnx model set at {}", model_dir.display())
                })?;
        Ok(Box::new(provider))
    }
    #[cfg(not(feature = "stt-sherpa"))]
    {
        let _ = model_dir;
        bail!(
            "`scrybe bench stt` requires the `stt-sherpa` cargo feature; rebuild with \
             `--features whisper-local,stt-sherpa`."
        );
    }
}

/// `.cargo/config.toml`'s build-time sentinel; kept in sync manually.
#[cfg(feature = "stt-sherpa")]
const SHERPA_SENTINEL_VALUE: &str = "__scrybe_requires_explicit_sherpa_runtime__";

/// Native static-lib filenames `sherpa-onnx-sys` links (C API, core, ONNX
/// Runtime -- see `INSTALL.md`): `lib`-prefixed `.a` everywhere except
/// Windows, which uses MSVC `.lib` naming.
#[cfg(all(feature = "stt-sherpa", target_os = "windows"))]
const SHERPA_REQUIRED_LIBS: [&str; 3] = [
    "sherpa-onnx-c-api.lib",
    "sherpa-onnx-core.lib",
    "onnxruntime.lib",
];
#[cfg(all(feature = "stt-sherpa", not(target_os = "windows")))]
const SHERPA_REQUIRED_LIBS: [&str; 3] = [
    "libsherpa-onnx-c-api.a",
    "libsherpa-onnx-core.a",
    "libonnxruntime.a",
];

fn require_sherpa_runtime_lib_dir() -> Result<PathBuf> {
    #[cfg(feature = "stt-sherpa")]
    {
        validate_sherpa_runtime_lib_dir(std::env::var_os("SHERPA_ONNX_LIB_DIR"))
    }
    #[cfg(not(feature = "stt-sherpa"))]
    {
        bail!(
            "`scrybe bench stt` requires the `stt-sherpa` cargo feature; rebuild with \
             `--features whisper-local,stt-sherpa`."
        );
    }
}

/// Reject a missing variable, the build-time sentinel, a nonexistent
/// directory, and a directory missing (or holding an empty placeholder
/// for) any required static lib. Pure over its argument -- exercised by
/// `tests/bench_stt.rs` via per-subprocess environments, never by
/// mutating this process's global environment.
#[cfg(feature = "stt-sherpa")]
fn validate_sherpa_runtime_lib_dir(raw: Option<OsString>) -> Result<PathBuf> {
    let raw = raw.ok_or_else(|| {
        anyhow::anyhow!(
            "SHERPA_ONNX_LIB_DIR is not set; `scrybe bench stt` requires an explicit, \
             pre-provisioned sherpa-onnx static runtime directory at execution time"
        )
    })?;
    if raw.to_str() == Some(SHERPA_SENTINEL_VALUE) {
        bail!(
            "SHERPA_ONNX_LIB_DIR is still the build-time sentinel from .cargo/config.toml; \
             provision a real sherpa-onnx static runtime directory before running \
             `scrybe bench stt`"
        );
    }
    let dir = PathBuf::from(raw);
    if !dir.is_dir() {
        bail!(
            "SHERPA_ONNX_LIB_DIR={} is not a directory; provision the sherpa-onnx runtime first",
            dir.display()
        );
    }
    for name in SHERPA_REQUIRED_LIBS {
        let provisioned =
            std::fs::metadata(dir.join(name)).is_ok_and(|m| m.is_file() && m.len() > 0);
        if !provisioned {
            bail!(
                "SHERPA_ONNX_LIB_DIR={} does not contain a provisioned sherpa-onnx runtime \
                 (missing or empty {name})",
                dir.display()
            );
        }
    }
    Ok(dir)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use async_trait::async_trait;

    use super::*;

    struct FailingProvider;
    struct EchoProvider;

    fn corpus(ids: &[&str]) -> PairedCorpus {
        PairedCorpus {
            clips: ids
                .iter()
                .map(|id| scrybe_core::testing::paired::PairedClip {
                    id: (*id).to_string(),
                    reference: "hello world".to_string(),
                    audio: scrybe_core::types::AudioChunk {
                        samples: std::sync::Arc::from(vec![0.0_f32; 1_600]),
                        source: scrybe_core::types::FrameSource::Mic,
                        start: std::time::Duration::ZERO,
                        duration: std::time::Duration::from_millis(100),
                    },
                    audio_sha256: "0".repeat(64),
                    provenance: "unit-test fixture".to_string(),
                })
                .collect(),
        }
    }

    #[async_trait]
    impl SttProvider for FailingProvider {
        async fn transcribe(
            &self,
            _chunk: scrybe_core::types::AudioChunk,
        ) -> Result<scrybe_core::types::TranscriptChunk, scrybe_core::error::SttError> {
            Err(scrybe_core::error::SttError::ModelNotLoaded("stub".into()))
        }

        fn name(&self) -> &'static str {
            "failing-stub"
        }
    }

    #[async_trait]
    impl SttProvider for EchoProvider {
        async fn transcribe(
            &self,
            chunk: scrybe_core::types::AudioChunk,
        ) -> Result<scrybe_core::types::TranscriptChunk, scrybe_core::error::SttError> {
            Ok(scrybe_core::types::TranscriptChunk {
                text: "hello world".to_string(),
                source: chunk.source,
                start_ms: 0,
                duration_ms: 100,
                language: Some("en".to_string()),
                tokens: Vec::new(),
            })
        }

        fn name(&self) -> &'static str {
            "echo-stub"
        }
    }

    #[tokio::test]
    async fn test_measure_backend_propagates_transcribe_failure_with_clip_context() {
        let corpus = corpus(&["en-01"]);

        let err = measure_backend(|| Ok(Box::new(FailingProvider)), "stub-backend", &corpus)
            .await
            .unwrap_err();

        assert!(err.to_string().contains("en-01"));
    }

    #[tokio::test]
    async fn test_measure_backend_constructs_a_fresh_provider_for_every_clip() {
        let corpus = corpus(&["en-01", "en-02"]);
        let mut construction_count = 0;

        let measurements = measure_backend(
            || {
                construction_count += 1;
                Ok(Box::new(EchoProvider))
            },
            "stub-backend",
            &corpus,
        )
        .await
        .unwrap();

        assert_eq!(construction_count, corpus.clips.len());
        assert_eq!(measurements.len(), corpus.clips.len());
    }

    #[tokio::test]
    async fn test_measure_backend_includes_provider_construction_in_timing() {
        let corpus = corpus(&["en-01"]);
        let construction_delay = std::time::Duration::from_millis(20);

        let measurements = measure_backend(
            || {
                std::thread::sleep(construction_delay);
                Ok(Box::new(EchoProvider))
            },
            "stub-backend",
            &corpus,
        )
        .await
        .unwrap();

        assert!(measurements[0].provider_lifecycle_secs >= construction_delay.as_secs_f64());
    }
}
