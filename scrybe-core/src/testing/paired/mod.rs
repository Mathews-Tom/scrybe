// Copyright 2026 Mathews Tom
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//     https://www.apache.org/licenses/LICENSE-2.0

//! Paired speech-to-text corpus harness.
//!
//! Distinct from `testing::multilingual`'s 20-clip Whisper-only
//! regression manifest: a *paired* corpus manifest carries an explicit
//! `audio` path and `sha256` per clip so the exact same decoded PCM16
//! bytes can be fed to more than one `SttProvider` backend
//! (`whisper-local`, `sherpa-onnx`) and their transcripts compared
//! head-to-head on identical input. [`load_corpus`] owns manifest
//! parsing, audio decoding, and checksum verification; [`aggregate`]
//! turns a caller-collected set of per-backend transcripts into a
//! serializable report. WER itself is *not* reimplemented here — both
//! the per-clip ratio and the aggregate weighting denominator reuse
//! `testing::multilingual::{word_error_rate, tokens_for}` so the two
//! corpora can never silently diverge on what counts as a "word".
//!
//! Tier-3 internal: this is a harness for the regression/bench suite,
//! not a public API. Stability is best-effort; sibling crates in this
//! workspace (e.g. `scrybe-cli`'s `bench stt` command) are the only
//! consumers.

mod wav;

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use self::wav::decode_pcm16_wav;
use super::multilingual::{tokens_for, word_error_rate};
use crate::pipeline::normalize::STT_SAMPLE_RATE;
use crate::types::{AudioChunk, FrameSource};

/// The only accepted paired-manifest schema version.
pub const CURRENT_PAIRED_MANIFEST_VERSION: u32 = 1;

/// Report schema version. Bump when [`PairedReport`]'s shape changes
/// in a way a consumer deserializing a previously-written report
/// would need to know about.
pub const CURRENT_PAIRED_REPORT_VERSION: u32 = 2;

/// STT backend identifier for the local `whisper-rs` provider.
pub const BACKEND_WHISPER_LOCAL: &str = "whisper-local";
/// STT backend identifier for the `sherpa-onnx` streaming provider.
pub const BACKEND_SHERPA_ONNX: &str = "sherpa-onnx";
/// Every clip requires exactly one measurement from each of these backends.
pub const KNOWN_BACKENDS: [&str; 2] = [BACKEND_WHISPER_LOCAL, BACKEND_SHERPA_ONNX];

const ONLY_SUPPORTED_LANGUAGE: &str = "en";
const REQUIRED_CHANNELS: u16 = 1;
const SHA256_HEX_LEN: usize = 64;

/// Top-level paired-manifest schema. Strict TOML: unknown fields are
/// rejected.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct PairedManifestV1 {
    schema_version: u32,
    clips: Vec<PairedClipManifestV1>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct PairedClipManifestV1 {
    id: String,
    language: String,
    audio: String,
    sha256: String,
    reference: String,
    provenance: String,
}

/// Corpus loading and validation failures, retaining clip-specific context.
#[derive(Debug, thiserror::Error)]
pub enum PairedCorpusError {
    #[error("paired manifest not found at {path}")]
    NotFound { path: String },
    #[error("paired manifest io error at {path}: {source}")]
    Io {
        path: String,
        #[source]
        source: std::io::Error,
    },
    #[error("paired manifest parse error: {0}")]
    Parse(String),
    #[error("paired manifest schema version {found} is unsupported; this build only understands schema version {target}")]
    UnsupportedSchemaVersion { found: u32, target: u32 },
    #[error("paired manifest contains zero clips; corpus is unusable")]
    Empty,
    #[error("paired manifest contains duplicate clip id {0}")]
    DuplicateClipId(String),
    #[error("clip {id}: {field} must be non-empty")]
    EmptyField { id: String, field: &'static str },
    #[error("clip {id}: language must be exactly \"en\", found {found:?}")]
    UnsupportedLanguage { id: String, found: String },
    #[error("clip {id}: sha256 must be exactly 64 hex characters, found {found:?}")]
    InvalidSha256Format { id: String, found: String },
    #[error("clip {id}: audio path is unusable: {detail}")]
    InvalidAudioPath { id: String, detail: String },
    #[error("clip {id}: sha256 mismatch — manifest declares {expected} but the audio file hashes to {actual}")]
    ChecksumMismatch {
        id: String,
        expected: String,
        actual: String,
    },
    #[error("clip {id}: audio is not valid 16 kHz mono PCM16 (no resampling): {detail}")]
    InvalidAudioFormat { id: String, detail: String },
}

/// A checksummed clip decoded once and shared unchanged between backends.
#[derive(Clone, Debug)]
pub struct PairedClip {
    pub id: String,
    pub reference: String,
    pub audio: AudioChunk,
    pub audio_sha256: String,
    pub provenance: String,
}

/// A loaded, fully validated paired corpus. Guaranteed non-empty with
/// unique clip ids (see [`load_corpus`]).
#[derive(Clone, Debug)]
pub struct PairedCorpus {
    pub clips: Vec<PairedClip>,
}

/// Read and validate the paired manifest at `path`, decode every
/// clip's referenced WAV file, and verify it against the manifest's
/// declared checksum.
///
/// Validates: exact schema version match, non-empty clip list, unique
/// non-empty clip ids, `language == "en"`, non-empty
/// reference/audio-path/provenance, a well-formed 64-hex-character
/// `sha256`, that the resolved (symlink- and `..`-resolved) audio path
/// stays within the manifest's own directory, that the audio file's
/// real SHA-256 matches the manifest's declared value, and that the
/// audio decodes as strict 16 kHz mono PCM16 (no resampling — a
/// mismatched format is rejected, not converted).
///
/// # Errors
///
/// See [`PairedCorpusError`] for the full set of rejection reasons.
pub fn load_corpus(path: &Path) -> Result<PairedCorpus, PairedCorpusError> {
    let body = std::fs::read_to_string(path).map_err(|e| match e.kind() {
        std::io::ErrorKind::NotFound => PairedCorpusError::NotFound {
            path: path.display().to_string(),
        },
        _ => PairedCorpusError::Io {
            path: path.display().to_string(),
            source: e,
        },
    })?;
    let manifest: PairedManifestV1 =
        toml::from_str(&body).map_err(|e| PairedCorpusError::Parse(e.to_string()))?;

    if manifest.schema_version != CURRENT_PAIRED_MANIFEST_VERSION {
        return Err(PairedCorpusError::UnsupportedSchemaVersion {
            found: manifest.schema_version,
            target: CURRENT_PAIRED_MANIFEST_VERSION,
        });
    }
    if manifest.clips.is_empty() {
        return Err(PairedCorpusError::Empty);
    }

    let manifest_dir = match path.parent() {
        Some(dir) if !dir.as_os_str().is_empty() => dir,
        _ => Path::new("."),
    };
    let corpus_root = std::fs::canonicalize(manifest_dir).map_err(|e| PairedCorpusError::Io {
        path: manifest_dir.display().to_string(),
        source: e,
    })?;

    let mut seen_ids = std::collections::BTreeSet::new();
    let mut clips = Vec::with_capacity(manifest.clips.len());
    for raw in manifest.clips {
        let normalized_sha256 = validate_clip_manifest(&raw)?;
        if !seen_ids.insert(raw.id.clone()) {
            return Err(PairedCorpusError::DuplicateClipId(raw.id));
        }

        let audio_path = manifest_dir.join(&raw.audio);
        let audio_path_canonical = std::fs::canonicalize(&audio_path).map_err(|e| {
            PairedCorpusError::InvalidAudioPath {
                id: raw.id.clone(),
                detail: format!("{} ({e})", audio_path.display()),
            }
        })?;
        if !audio_path_canonical.starts_with(&corpus_root) {
            return Err(PairedCorpusError::InvalidAudioPath {
                id: raw.id,
                detail: format!(
                    "{} resolves outside the corpus root {} (rejecting a relative-path or symlink escape)",
                    audio_path_canonical.display(),
                    corpus_root.display()
                ),
            });
        }

        let bytes = std::fs::read(&audio_path_canonical).map_err(|e| {
            PairedCorpusError::InvalidAudioPath {
                id: raw.id.clone(),
                detail: format!("{} ({e})", audio_path_canonical.display()),
            }
        })?;

        let actual_hash = format!("{:x}", Sha256::digest(&bytes));
        if actual_hash != normalized_sha256 {
            return Err(PairedCorpusError::ChecksumMismatch {
                id: raw.id,
                expected: normalized_sha256,
                actual: actual_hash,
            });
        }

        let decoded =
            decode_pcm16_wav(&bytes).map_err(|detail| PairedCorpusError::InvalidAudioFormat {
                id: raw.id.clone(),
                detail,
            })?;
        if decoded.sample_rate != STT_SAMPLE_RATE || decoded.channels != REQUIRED_CHANNELS {
            return Err(PairedCorpusError::InvalidAudioFormat {
                id: raw.id,
                detail: format!(
                    "{} Hz / {} channel(s)",
                    decoded.sample_rate, decoded.channels
                ),
            });
        }

        #[allow(clippy::cast_precision_loss)]
        let duration_secs = decoded.samples.len() as f64 / f64::from(STT_SAMPLE_RATE);

        clips.push(PairedClip {
            id: raw.id,
            reference: raw.reference,
            audio: AudioChunk {
                samples: Arc::from(decoded.samples),
                source: FrameSource::Mic,
                start: Duration::ZERO,
                duration: Duration::from_secs_f64(duration_secs),
            },
            audio_sha256: actual_hash,
            provenance: raw.provenance,
        });
    }

    Ok(PairedCorpus { clips })
}

fn validate_clip_manifest(raw: &PairedClipManifestV1) -> Result<String, PairedCorpusError> {
    for (field, value) in [
        ("id", &raw.id),
        ("reference", &raw.reference),
        ("provenance", &raw.provenance),
        ("audio", &raw.audio),
    ] {
        require_nonempty(&raw.id, field, value)?;
    }
    if raw.language != ONLY_SUPPORTED_LANGUAGE {
        return Err(PairedCorpusError::UnsupportedLanguage {
            id: raw.id.clone(),
            found: raw.language.clone(),
        });
    }
    if tokens_for(&raw.reference).is_empty() {
        return Err(PairedCorpusError::EmptyField {
            id: raw.id.clone(),
            field: "reference words",
        });
    }
    if Path::new(&raw.audio).is_absolute() {
        return Err(PairedCorpusError::InvalidAudioPath {
            id: raw.id.clone(),
            detail: "audio must be a relative path".into(),
        });
    }
    validate_sha256_format(&raw.id, &raw.sha256)
}

fn require_nonempty(id: &str, field: &'static str, value: &str) -> Result<(), PairedCorpusError> {
    if value.trim().is_empty() {
        return Err(PairedCorpusError::EmptyField {
            id: id.to_string(),
            field,
        });
    }
    Ok(())
}

fn validate_sha256_format(id: &str, raw_sha256: &str) -> Result<String, PairedCorpusError> {
    let trimmed = raw_sha256.trim();
    if trimmed.len() != SHA256_HEX_LEN || !trimmed.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(PairedCorpusError::InvalidSha256Format {
            id: id.to_string(),
            found: raw_sha256.to_string(),
        });
    }
    Ok(trimmed.to_ascii_lowercase())
}

/// One backend's transcription result for one clip.
///
/// `provider_lifecycle_secs` is the caller-measured elapsed time from
/// provider construction through transcription. [`aggregate`] rejects
/// a non-finite or negative value.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ClipMeasurement {
    pub clip_id: String,
    pub backend: String,
    pub hypothesis: String,
    pub provider_lifecycle_secs: f64,
}

/// Errors raised by [`aggregate`].
#[derive(Debug, thiserror::Error)]
pub enum AggregateError {
    #[error("invalid paired corpus: {0}")]
    InvalidCorpus(String),
    #[error("paired measurements overflow finite WER/realtime metrics")]
    NonFiniteMetrics,
    #[error("measurement backend {backend:?} is not a known backend (expected \"whisper-local\" or \"sherpa-onnx\")")]
    UnknownBackend { backend: String },

    #[error("measurement references unknown clip id {clip_id:?}; the corpus has no such clip")]
    UnknownClip { clip_id: String },

    #[error("duplicate measurement for clip {clip_id:?} backend {backend:?}")]
    DuplicateMeasurement { clip_id: String, backend: String },

    #[error("missing measurement for clip {clip_id:?} backend {backend:?}")]
    MissingMeasurement { clip_id: String, backend: String },

    #[error(
        "measurement for clip {clip_id:?} backend {backend:?} has a non-finite or negative provider_lifecycle_secs: {provider_lifecycle_secs}"
    )]
    InvalidProviderLifecycleSecs {
        clip_id: String,
        backend: String,
        provider_lifecycle_secs: f64,
    },
}

/// One clip/backend measurement with the validated audio checksum and provenance.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ClipReport {
    pub clip_id: String,
    pub backend: String,
    pub reference: String,
    pub hypothesis: String,
    pub audio_sha256: String,
    pub provenance: String,
    pub wer: f64,
    pub ref_word_count: usize,
    pub audio_secs: f64,
    pub provider_lifecycle_secs: f64,
    pub realtime_factor: f64,
}

/// Per-backend rollup across every clip in the corpus.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct BackendSummary {
    pub backend: String,
    pub clip_count: usize,
    pub total_ref_words: usize,
    /// Word-count-weighted mean WER: `Σ(edit_distance) / Σ(ref_words)`,
    /// computed as `Σ(wer_i × ref_words_i) / Σ(ref_words_i)` — the
    /// same weighted mean, expressed in terms of the per-clip ratio
    /// `word_error_rate` already returns rather than reaching into
    /// its internal edit-distance numerator.
    pub weighted_wer: f64,
    pub total_audio_secs: f64,
    pub total_provider_lifecycle_secs: f64,
    /// `total_provider_lifecycle_secs / total_audio_secs`, never a zero-denominator fallback.
    pub realtime_factor: f64,
}

/// Serializable, versioned paired-benchmark report.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PairedReport {
    pub report_version: u32,
    pub clips: Vec<ClipReport>,
    pub backends: Vec<BackendSummary>,
}

#[derive(Default)]
struct BackendAccumulator {
    clip_count: usize,
    total_ref_words: usize,
    wer_numerator: f64,
    total_audio_secs: f64,
    total_provider_lifecycle_secs: f64,
}

/// Validate `measurements` against `corpus` and fold them into a
/// [`PairedReport`].
///
/// Requires exactly one measurement per (clip id, backend) pair for
/// every clip in `corpus` crossed with every backend in
/// [`KNOWN_BACKENDS`] — no more, no fewer. Per-clip WER reuses
/// `multilingual::word_error_rate` directly; the per-backend
/// `weighted_wer` denominator reuses `multilingual::tokens_for` so the
/// weighting never diverges from the ratio it is weighting.
///
/// # Errors
///
/// [`AggregateError::UnknownBackend`] / [`AggregateError::UnknownClip`]
/// for a measurement outside the corpus/known-backend set,
/// [`AggregateError::DuplicateMeasurement`] for a repeated
/// (clip, backend) pair, [`AggregateError::MissingMeasurement`] for an
/// absent one, and [`AggregateError::InvalidProviderLifecycleSecs`] for a
/// non-finite or negative `provider_lifecycle_secs`.
#[allow(clippy::cast_precision_loss)]
pub fn aggregate(
    corpus: &PairedCorpus,
    measurements: Vec<ClipMeasurement>,
) -> Result<PairedReport, AggregateError> {
    let clip_by_id: BTreeMap<&str, &PairedClip> = corpus
        .clips
        .iter()
        .map(|clip| (clip.id.as_str(), clip))
        .collect();
    if corpus.clips.is_empty()
        || clip_by_id.len() != corpus.clips.len()
        || corpus.clips.iter().any(|clip| {
            clip.audio.duration.is_zero()
                || clip.audio.samples.is_empty()
                || tokens_for(&clip.reference).is_empty()
        })
    {
        return Err(AggregateError::InvalidCorpus(
            "requires unique nonempty clips with positive audio duration and scoreable references"
                .into(),
        ));
    }

    let by_key = index_measurements(&clip_by_id, measurements)?;

    let mut totals: BTreeMap<String, BackendAccumulator> = KNOWN_BACKENDS
        .iter()
        .map(|&backend| (backend.to_string(), BackendAccumulator::default()))
        .collect();
    let mut clip_reports = Vec::with_capacity(by_key.len());

    for ((clip_id, backend), measurement) in by_key {
        let clip = clip_by_id[clip_id.as_str()];
        let ref_word_count = tokens_for(&clip.reference).len();
        let wer = word_error_rate(&clip.reference, &measurement.hypothesis);
        let audio_secs = clip.audio.duration.as_secs_f64();
        let provider_lifecycle_secs = measurement.provider_lifecycle_secs;
        let realtime_factor = provider_lifecycle_secs / audio_secs;
        if !realtime_factor.is_finite() {
            return Err(AggregateError::NonFiniteMetrics);
        }

        // `entry` always hits: `totals` was seeded from `KNOWN_BACKENDS`
        // and `backend` was validated against that same set above.
        let acc = totals.entry(backend.clone()).or_default();
        acc.clip_count += 1;
        acc.total_ref_words += ref_word_count;
        acc.wer_numerator += wer * ref_word_count as f64;
        acc.total_audio_secs += audio_secs;
        acc.total_provider_lifecycle_secs += provider_lifecycle_secs;
        if !acc.total_provider_lifecycle_secs.is_finite()
            || !acc.total_audio_secs.is_finite()
            || !acc.wer_numerator.is_finite()
        {
            return Err(AggregateError::NonFiniteMetrics);
        }

        clip_reports.push(ClipReport {
            clip_id,
            backend,
            reference: clip.reference.clone(),
            hypothesis: measurement.hypothesis,
            audio_sha256: clip.audio_sha256.clone(),
            provenance: clip.provenance.clone(),
            wer,
            ref_word_count,
            audio_secs,
            provider_lifecycle_secs,
            realtime_factor,
        });
    }

    let backends = KNOWN_BACKENDS
        .iter()
        .map(|&backend| {
            let acc = &totals[backend];
            let weighted_wer = acc.wer_numerator / acc.total_ref_words as f64;
            let realtime_factor = acc.total_provider_lifecycle_secs / acc.total_audio_secs;
            BackendSummary {
                backend: backend.to_string(),
                clip_count: acc.clip_count,
                total_ref_words: acc.total_ref_words,
                weighted_wer,
                total_audio_secs: acc.total_audio_secs,
                total_provider_lifecycle_secs: acc.total_provider_lifecycle_secs,
                realtime_factor,
            }
        })
        .collect();

    Ok(PairedReport {
        report_version: CURRENT_PAIRED_REPORT_VERSION,
        clips: clip_reports,
        backends,
    })
}

fn index_measurements(
    clips: &BTreeMap<&str, &PairedClip>,
    measurements: Vec<ClipMeasurement>,
) -> Result<BTreeMap<(String, String), ClipMeasurement>, AggregateError> {
    let mut by_key = BTreeMap::new();
    for measurement in measurements {
        if !KNOWN_BACKENDS.contains(&measurement.backend.as_str()) {
            return Err(AggregateError::UnknownBackend {
                backend: measurement.backend,
            });
        }
        if !clips.contains_key(measurement.clip_id.as_str()) {
            return Err(AggregateError::UnknownClip {
                clip_id: measurement.clip_id,
            });
        }
        if !measurement.provider_lifecycle_secs.is_finite()
            || measurement.provider_lifecycle_secs < 0.0
        {
            return Err(AggregateError::InvalidProviderLifecycleSecs {
                clip_id: measurement.clip_id,
                backend: measurement.backend,
                provider_lifecycle_secs: measurement.provider_lifecycle_secs,
            });
        }
        let key = (measurement.clip_id.clone(), measurement.backend.clone());
        if let Some(previous) = by_key.insert(key, measurement) {
            return Err(AggregateError::DuplicateMeasurement {
                clip_id: previous.clip_id,
                backend: previous.backend,
            });
        }
    }
    for clip in clips.values() {
        for backend in KNOWN_BACKENDS {
            let key = (clip.id.clone(), backend.to_string());
            if !by_key.contains_key(&key) {
                return Err(AggregateError::MissingMeasurement {
                    clip_id: key.0,
                    backend: key.1,
                });
            }
        }
    }
    Ok(by_key)
}
