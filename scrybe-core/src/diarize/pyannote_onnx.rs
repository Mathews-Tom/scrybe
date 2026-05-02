// Copyright 2026 Mathews Tom
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//     https://www.apache.org/licenses/LICENSE-2.0

//! `PyannoteOnnxDiarizer` — neural fallback for multi-party / in-room
//! meetings (`docs/system-design.md` §4.4).
//!
//! The diarizer wraps a [`PyannoteBackend`] so the trait surface and the
//! cluster-to-name mapping (which uses `MeetingContext.attendees` to
//! turn anonymous cluster labels into named speakers when possible) can
//! be unit-tested without bundling an ONNX model.
//!
//! The live binding lands as a v0.5.x follow-up tracked in
//! `.docs/development-plan.md` §11.2. Without the `diarize-pyannote`
//! cargo feature, calling [`PyannoteOnnxDiarizer::new_live`] returns a
//! typed error rather than a phantom successful load — the same
//! pattern as `WhisperLocalProvider` / `ParakeetLocalProvider`.

use std::path::{Path, PathBuf};

use async_trait::async_trait;

use crate::context::MeetingContext;
use crate::error::{CoreError, PipelineError};
use crate::types::{AttributedChunk, SpeakerLabel, TranscriptChunk};

use super::Diarizer;

/// Configuration for [`PyannoteOnnxDiarizer`].
///
/// `model_path` MUST point to a verified pyannote-onnx model directory.
/// The `*.partial` rejection mirrors the model-download recovery rule in
/// `system-design.md` §8.1.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PyannoteOnnxConfig {
    pub model_path: PathBuf,
    /// Display label embedded in the provider name (`pyannote-onnx:<label>`).
    pub model_label: String,
}

impl PyannoteOnnxConfig {
    /// Construct with the default 3.1 model label.
    #[must_use]
    pub fn new(model_path: PathBuf) -> Self {
        Self {
            model_path,
            model_label: "3.1".to_string(),
        }
    }
}

/// One cluster returned by the underlying ONNX model. Each cluster
/// carries a stable index (0, 1, 2, …) and the contiguous half-open
/// time intervals (in milliseconds) where that cluster spoke.
///
/// Backends emit clusters; the diarizer maps clusters to
/// `SpeakerLabel::Named` using `MeetingContext.attendees` order, falling
/// back to `Named("Speaker N")` when the attendee list is shorter than
/// the cluster set.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SpeakerCluster {
    pub cluster_index: u32,
    pub spans_ms: Vec<(u64, u64)>,
}

/// Pluggable inference backend. The diarizer holds one of these and
/// delegates to it; tests construct a deterministic stub, the live
/// binding wires up `pyannote-onnx` via the ONNX runtime.
///
/// This is the seam that lets v0.5.0 ship the Diarizer trait surface
/// and the cluster-to-name mapping without depending on an unmerged
/// upstream Rust crate.
#[async_trait]
pub trait PyannoteBackend: Send + Sync {
    /// Run the model over the merged per-channel transcripts. Each
    /// returned cluster represents one detected speaker.
    ///
    /// # Errors
    ///
    /// Backends return `CoreError::Pipeline` for compute-time failures
    /// (model not loaded, ONNX runtime panic, audio embedding mismatch)
    /// and `CoreError::Storage` for I/O failures while reading model
    /// artifacts.
    async fn cluster(
        &self,
        mic_chunks: &[TranscriptChunk],
        sys_chunks: &[TranscriptChunk],
    ) -> Result<Vec<SpeakerCluster>, CoreError>;
}

/// Neural diarizer. Generic over the backend so v0.5 can ship a
/// stub-validated trait surface today and a live ONNX runtime tomorrow
/// without changing the public API.
#[derive(Debug)]
pub struct PyannoteOnnxDiarizer<B: PyannoteBackend> {
    backend: B,
    name: String,
}

impl<B: PyannoteBackend> PyannoteOnnxDiarizer<B> {
    /// Stable provider-name prefix surfaced in `meta.toml`.
    pub const NAME_PREFIX: &'static str = "pyannote-onnx";

    /// Construct a diarizer around an arbitrary backend. Callers wire
    /// up the live ONNX backend or a deterministic stub as appropriate.
    #[must_use]
    pub fn with_backend(backend: B, model_label: impl Into<String>) -> Self {
        let name = format!("{}:{}", Self::NAME_PREFIX, model_label.into());
        Self { backend, name }
    }

    /// Borrow the underlying backend. Tests use this to assert backend
    /// state after a `diarize` call.
    pub const fn backend(&self) -> &B {
        &self.backend
    }
}

impl PyannoteOnnxDiarizer<LivePyannoteBackend> {
    /// Construct a diarizer wired to the live ONNX backend.
    ///
    /// # Errors
    ///
    /// `CoreError::Pipeline` when the `diarize-pyannote` cargo feature
    /// is disabled (the live binding is feature-gated). `CoreError::Storage`
    /// if the model path is a `*.partial` download in progress; the
    /// loader rejects partial paths to surface model-corruption early.
    pub fn new_live(config: PyannoteOnnxConfig) -> Result<Self, CoreError> {
        if is_partial(&config.model_path) {
            return Err(CoreError::Pipeline(PipelineError::DiarizerUnavailable {
                reason: format!(
                    "pyannote-onnx model path looks like a partial download: {}",
                    config.model_path.display()
                ),
            }));
        }
        let backend = LivePyannoteBackend::new(config.model_path)?;
        Ok(Self::with_backend(backend, config.model_label))
    }
}

#[async_trait]
impl<B: PyannoteBackend> Diarizer for PyannoteOnnxDiarizer<B> {
    async fn diarize(
        &self,
        mic_chunks: &[TranscriptChunk],
        sys_chunks: &[TranscriptChunk],
        ctx: &MeetingContext,
    ) -> Result<Vec<AttributedChunk>, CoreError> {
        let clusters = self.backend.cluster(mic_chunks, sys_chunks).await?;
        Ok(attribute_chunks(mic_chunks, sys_chunks, &clusters, ctx))
    }

    fn name(&self) -> &str {
        &self.name
    }
}

/// Live ONNX backend.
///
/// Constructed only when the `diarize-pyannote` feature is on; without
/// the feature, [`Self::new`] returns a typed error so the missing
/// dependency is obvious instead of a phantom successful load.
#[derive(Debug)]
pub struct LivePyannoteBackend {
    #[cfg_attr(not(feature = "diarize-pyannote"), allow(dead_code))]
    model_path: PathBuf,
}

impl LivePyannoteBackend {
    /// Load the live ONNX backend.
    ///
    /// # Errors
    ///
    /// `CoreError::Pipeline` when the `diarize-pyannote` cargo feature
    /// is disabled, or when the live binding has not yet landed (the
    /// feature is present but the runtime wiring is a v0.5.x follow-up).
    pub fn new(model_path: PathBuf) -> Result<Self, CoreError> {
        ensure_live_binding_available(&model_path)?;
        Ok(Self { model_path })
    }
}

#[cfg(feature = "diarize-pyannote")]
fn ensure_live_binding_available(model_path: &Path) -> Result<(), CoreError> {
    Err(CoreError::Pipeline(PipelineError::DiarizerUnavailable {
        reason: format!(
            "scrybe-core's `diarize-pyannote` feature is enabled but the \
             pyannote-onnx live runtime binding has not landed yet; cannot \
             load {}",
            model_path.display()
        ),
    }))
}

#[cfg(not(feature = "diarize-pyannote"))]
fn ensure_live_binding_available(model_path: &Path) -> Result<(), CoreError> {
    Err(CoreError::Pipeline(PipelineError::DiarizerUnavailable {
        reason: format!(
            "scrybe-core was built without the `diarize-pyannote` cargo \
             feature; enable it to load {}",
            model_path.display()
        ),
    }))
}

#[async_trait]
impl PyannoteBackend for LivePyannoteBackend {
    async fn cluster(
        &self,
        _mic: &[TranscriptChunk],
        _sys: &[TranscriptChunk],
    ) -> Result<Vec<SpeakerCluster>, CoreError> {
        Err(CoreError::Pipeline(PipelineError::DiarizerUnavailable {
            reason: format!(
                "pyannote-onnx live runtime binding pending; backend constructed \
                 with model_path={}",
                self.model_path.display()
            ),
        }))
    }
}

fn is_partial(path: &Path) -> bool {
    path.extension()
        .and_then(|os| os.to_str())
        .is_some_and(|ext| ext.eq_ignore_ascii_case("partial"))
}

/// Map cluster spans onto transcript chunks. A chunk that overlaps a
/// cluster span is attributed to that cluster's speaker label; chunks
/// with no overlap fall back to `SpeakerLabel::Unknown` so the renderer
/// surfaces unrouted utterances rather than silently dropping them.
fn attribute_chunks(
    mic_chunks: &[TranscriptChunk],
    sys_chunks: &[TranscriptChunk],
    clusters: &[SpeakerCluster],
    ctx: &MeetingContext,
) -> Vec<AttributedChunk> {
    let mut out: Vec<AttributedChunk> = Vec::with_capacity(mic_chunks.len() + sys_chunks.len());
    for chunk in mic_chunks.iter().chain(sys_chunks.iter()) {
        let speaker = find_cluster(chunk, clusters).map_or(SpeakerLabel::Unknown, |cluster_idx| {
            name_for_cluster(cluster_idx, ctx)
        });
        out.push(AttributedChunk {
            chunk: chunk.clone(),
            speaker,
        });
    }
    out.sort_by_key(|a| a.chunk.start_ms);
    out
}

fn find_cluster(chunk: &TranscriptChunk, clusters: &[SpeakerCluster]) -> Option<u32> {
    let chunk_start = chunk.start_ms;
    let chunk_end = chunk_start.saturating_add(chunk.duration_ms);
    clusters
        .iter()
        .find(|cluster| {
            cluster
                .spans_ms
                .iter()
                .any(|(span_start, span_end)| chunk_start < *span_end && chunk_end > *span_start)
        })
        .map(|cluster| cluster.cluster_index)
}

fn name_for_cluster(cluster_idx: u32, ctx: &MeetingContext) -> SpeakerLabel {
    let attendee = usize::try_from(cluster_idx)
        .ok()
        .and_then(|i| ctx.attendees.get(i));
    attendee.map_or_else(
        || SpeakerLabel::Named(format!("Speaker {}", cluster_idx + 1)),
        |name| SpeakerLabel::Named(name.clone()),
    )
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use crate::types::FrameSource;
    use pretty_assertions::assert_eq;
    use std::sync::Mutex;

    /// Deterministic backend used by tests. Captures the inputs it saw
    /// and returns whatever clusters the test wired up.
    struct StubBackend {
        clusters: Vec<SpeakerCluster>,
        observed_inputs: Mutex<Option<(usize, usize)>>,
    }

    impl StubBackend {
        const fn new(clusters: Vec<SpeakerCluster>) -> Self {
            Self {
                clusters,
                observed_inputs: Mutex::new(None),
            }
        }
    }

    #[async_trait]
    impl PyannoteBackend for StubBackend {
        async fn cluster(
            &self,
            mic: &[TranscriptChunk],
            sys: &[TranscriptChunk],
        ) -> Result<Vec<SpeakerCluster>, CoreError> {
            *self.observed_inputs.lock().unwrap() = Some((mic.len(), sys.len()));
            Ok(self.clusters.clone())
        }
    }

    fn t(text: &str, source: FrameSource, start_ms: u64, duration_ms: u64) -> TranscriptChunk {
        TranscriptChunk {
            text: text.into(),
            source,
            start_ms,
            duration_ms,
            language: None,
        }
    }

    #[test]
    fn test_pyannote_onnx_config_default_label_is_3_1() {
        let cfg = PyannoteOnnxConfig::new(PathBuf::from("/models/pyannote"));

        assert_eq!(cfg.model_label, "3.1");
    }

    #[test]
    fn test_pyannote_onnx_diarizer_name_includes_model_label() {
        let backend = StubBackend::new(vec![]);

        let diarizer = PyannoteOnnxDiarizer::with_backend(backend, "3.1");

        assert_eq!(diarizer.name(), "pyannote-onnx:3.1");
    }

    #[tokio::test]
    async fn test_pyannote_diarize_routes_chunks_to_clusters_by_temporal_overlap() {
        let backend = StubBackend::new(vec![
            SpeakerCluster {
                cluster_index: 0,
                spans_ms: vec![(0, 2_000)],
            },
            SpeakerCluster {
                cluster_index: 1,
                spans_ms: vec![(2_000, 4_000)],
            },
        ]);
        let diarizer = PyannoteOnnxDiarizer::with_backend(backend, "3.1");
        let ctx = MeetingContext::default();
        let mic = vec![t("Hi.", FrameSource::Mic, 100, 500)];
        let sys = vec![t("Hello.", FrameSource::System, 2_500, 800)];

        let attributed = diarizer.diarize(&mic, &sys, &ctx).await.unwrap();

        assert_eq!(attributed.len(), 2);
        assert_eq!(attributed[0].chunk.text, "Hi.");
        assert_eq!(
            attributed[0].speaker,
            SpeakerLabel::Named("Speaker 1".into())
        );
        assert_eq!(attributed[1].chunk.text, "Hello.");
        assert_eq!(
            attributed[1].speaker,
            SpeakerLabel::Named("Speaker 2".into())
        );
    }

    #[tokio::test]
    async fn test_pyannote_diarize_maps_cluster_indices_onto_attendee_names_when_available() {
        let backend = StubBackend::new(vec![
            SpeakerCluster {
                cluster_index: 0,
                spans_ms: vec![(0, 1_000)],
            },
            SpeakerCluster {
                cluster_index: 1,
                spans_ms: vec![(1_000, 2_000)],
            },
        ]);
        let diarizer = PyannoteOnnxDiarizer::with_backend(backend, "3.1");
        let ctx = MeetingContext {
            attendees: vec!["Alex".into(), "Sam".into()],
            ..MeetingContext::default()
        };
        let mic = vec![t("First.", FrameSource::Mic, 100, 500)];
        let sys = vec![t("Second.", FrameSource::System, 1_200, 500)];

        let attributed = diarizer.diarize(&mic, &sys, &ctx).await.unwrap();

        assert_eq!(attributed[0].speaker, SpeakerLabel::Named("Alex".into()));
        assert_eq!(attributed[1].speaker, SpeakerLabel::Named("Sam".into()));
    }

    #[tokio::test]
    async fn test_pyannote_diarize_falls_back_to_speaker_n_when_attendee_list_is_short() {
        let backend = StubBackend::new(vec![SpeakerCluster {
            cluster_index: 2,
            spans_ms: vec![(0, 1_000)],
        }]);
        let diarizer = PyannoteOnnxDiarizer::with_backend(backend, "3.1");
        let ctx = MeetingContext {
            attendees: vec!["Alex".into(), "Sam".into()],
            ..MeetingContext::default()
        };
        let mic = vec![t("Hi.", FrameSource::Mic, 100, 500)];

        let attributed = diarizer.diarize(&mic, &[], &ctx).await.unwrap();

        assert_eq!(
            attributed[0].speaker,
            SpeakerLabel::Named("Speaker 3".into()),
        );
    }

    #[tokio::test]
    async fn test_pyannote_diarize_emits_unknown_when_no_cluster_overlaps() {
        let backend = StubBackend::new(vec![SpeakerCluster {
            cluster_index: 0,
            spans_ms: vec![(10_000, 11_000)],
        }]);
        let diarizer = PyannoteOnnxDiarizer::with_backend(backend, "3.1");
        let mic = vec![t("Stranded.", FrameSource::Mic, 100, 500)];

        let attributed = diarizer
            .diarize(&mic, &[], &MeetingContext::default())
            .await
            .unwrap();

        assert_eq!(attributed[0].speaker, SpeakerLabel::Unknown);
    }

    #[tokio::test]
    async fn test_pyannote_diarize_sorts_merged_chunks_by_start_ms() {
        let backend = StubBackend::new(vec![SpeakerCluster {
            cluster_index: 0,
            spans_ms: vec![(0, 10_000)],
        }]);
        let diarizer = PyannoteOnnxDiarizer::with_backend(backend, "3.1");
        let mic = vec![
            t("Mic late.", FrameSource::Mic, 5_000, 500),
            t("Mic early.", FrameSource::Mic, 1_000, 500),
        ];
        let sys = vec![
            t("Sys early.", FrameSource::System, 0, 500),
            t("Sys mid.", FrameSource::System, 3_000, 500),
        ];

        let attributed = diarizer
            .diarize(&mic, &sys, &MeetingContext::default())
            .await
            .unwrap();

        let texts: Vec<&str> = attributed.iter().map(|a| a.chunk.text.as_str()).collect();
        assert_eq!(
            texts,
            vec!["Sys early.", "Mic early.", "Sys mid.", "Mic late."]
        );
    }

    #[tokio::test]
    async fn test_pyannote_diarize_passes_both_channels_to_backend() {
        let backend = StubBackend::new(vec![]);
        let diarizer = PyannoteOnnxDiarizer::with_backend(backend, "3.1");
        let mic = vec![t("a.", FrameSource::Mic, 0, 100)];
        let sys = vec![
            t("b.", FrameSource::System, 100, 100),
            t("c.", FrameSource::System, 200, 100),
        ];

        let _ = diarizer
            .diarize(&mic, &sys, &MeetingContext::default())
            .await
            .unwrap();

        let observed = diarizer
            .backend()
            .observed_inputs
            .lock()
            .unwrap()
            .expect("backend.cluster called");
        assert_eq!(observed, (1, 2));
    }

    #[test]
    fn test_pyannote_onnx_diarizer_new_live_rejects_partial_model_path() {
        let cfg = PyannoteOnnxConfig::new(PathBuf::from("/models/pyannote.partial"));

        let err = PyannoteOnnxDiarizer::new_live(cfg).unwrap_err();

        match err {
            CoreError::Pipeline(PipelineError::DiarizerUnavailable { reason }) => {
                assert!(
                    reason.contains("partial"),
                    "reason should mention partial download: {reason}"
                );
            }
            other => panic!("expected DiarizerUnavailable, got {other:?}"),
        }
    }

    #[cfg(not(feature = "diarize-pyannote"))]
    #[test]
    fn test_pyannote_onnx_diarizer_new_live_errors_when_feature_disabled() {
        let cfg = PyannoteOnnxConfig::new(PathBuf::from("/models/pyannote"));

        let err = PyannoteOnnxDiarizer::new_live(cfg).unwrap_err();

        match err {
            CoreError::Pipeline(PipelineError::DiarizerUnavailable { reason }) => {
                assert!(
                    reason.contains("diarize-pyannote"),
                    "reason should name the cargo feature: {reason}"
                );
            }
            other => panic!("expected DiarizerUnavailable, got {other:?}"),
        }
    }

    #[cfg(feature = "diarize-pyannote")]
    #[test]
    fn test_pyannote_onnx_diarizer_new_live_errors_when_runtime_binding_pending() {
        let cfg = PyannoteOnnxConfig::new(PathBuf::from("/models/pyannote"));

        let err = PyannoteOnnxDiarizer::new_live(cfg).unwrap_err();

        match err {
            CoreError::Pipeline(PipelineError::DiarizerUnavailable { reason }) => {
                assert!(
                    reason.contains("pyannote-onnx"),
                    "reason should mention pyannote-onnx: {reason}"
                );
            }
            other => panic!("expected DiarizerUnavailable, got {other:?}"),
        }
    }

    #[test]
    fn test_is_partial_helper_matches_partial_extension_case_insensitive() {
        assert!(is_partial(Path::new("/models/foo.partial")));
        assert!(is_partial(Path::new("/models/foo.PARTIAL")));
        assert!(!is_partial(Path::new("/models/foo")));
        assert!(!is_partial(Path::new("/models/foo.bin")));
    }

    #[test]
    fn test_speaker_cluster_round_trips_via_clone_and_eq() {
        let cluster = SpeakerCluster {
            cluster_index: 1,
            spans_ms: vec![(100, 200), (300, 400)],
        };

        assert_eq!(cluster.clone(), cluster);
    }
}
