// Copyright 2026 Mathews Tom
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//     https://www.apache.org/licenses/LICENSE-2.0

//! Multilingual regression-corpus harness.
//!
//! The 20-clip manifest at `tests/fixtures/multilingual/MANIFEST.toml`
//! lists the clips used to gate WER per language. This module owns the
//! parser and the token-level WER computation; the integration test
//! that loads the manifest, runs Whisper, and asserts WER ≤ 15% lives
//! at `scrybe-core/tests/multilingual_corpus.rs`. The test skips when
//! the env var `SCRYBE_MULTILINGUAL_CORPUS` is unset because the audio
//! itself is not committed to the repo (see the manifest header for
//! the acquisition protocol).
//!
//! Tier-3 internal: this is a harness for the regression suite, not a
//! public API. Stability is best-effort; the sibling test crate is the
//! only consumer.

use std::path::Path;

use serde::{Deserialize, Serialize};

/// Top-level manifest schema. Strict TOML: unknown fields are rejected.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ManifestV1 {
    pub schema_version: u32,
    pub clips: Vec<ClipV1>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ClipV1 {
    pub id: String,
    pub language: String,
    pub duration_secs_max: f64,
    pub expected_text: String,
    pub provenance: String,
}

/// Schema version this build understands. The manifest's
/// `schema_version` is rejected when greater than this.
pub const CURRENT_MANIFEST_VERSION: u32 = 1;

/// Errors raised by [`load_manifest`] and [`word_error_rate`].
#[derive(Debug, thiserror::Error)]
pub enum CorpusError {
    #[error("manifest not found at {path}")]
    NotFound { path: String },

    #[error("manifest io error at {path}: {source}")]
    Io {
        path: String,
        #[source]
        source: std::io::Error,
    },

    #[error("manifest parse error: {0}")]
    Parse(String),

    #[error("manifest schema version {found} cannot be loaded; this build understands ≤ {target}")]
    UnsupportedSchemaVersion { found: u32, target: u32 },

    #[error("manifest contains duplicate clip id {0}")]
    DuplicateClipId(String),

    #[error("manifest contains zero clips; corpus is unusable")]
    Empty,
}

/// Read and validate the manifest at `path`. Validates: schema version,
/// non-empty clips list, unique clip ids.
///
/// # Errors
///
/// `CorpusError::NotFound` if the file is absent, `Parse` for malformed
/// TOML, `UnsupportedSchemaVersion` if `schema_version` exceeds the
/// build's support, `DuplicateClipId` if two clips share an id, and
/// `Empty` for a clip-less manifest.
pub fn load_manifest(path: &Path) -> Result<ManifestV1, CorpusError> {
    let body = std::fs::read_to_string(path).map_err(|e| match e.kind() {
        std::io::ErrorKind::NotFound => CorpusError::NotFound {
            path: path.display().to_string(),
        },
        _ => CorpusError::Io {
            path: path.display().to_string(),
            source: e,
        },
    })?;
    let parsed: ManifestV1 =
        toml::from_str(&body).map_err(|e| CorpusError::Parse(e.to_string()))?;
    validate(&parsed)?;
    Ok(parsed)
}

fn validate(manifest: &ManifestV1) -> Result<(), CorpusError> {
    if manifest.schema_version > CURRENT_MANIFEST_VERSION {
        return Err(CorpusError::UnsupportedSchemaVersion {
            found: manifest.schema_version,
            target: CURRENT_MANIFEST_VERSION,
        });
    }
    if manifest.clips.is_empty() {
        return Err(CorpusError::Empty);
    }
    let mut seen = std::collections::BTreeSet::new();
    for clip in &manifest.clips {
        if !seen.insert(clip.id.clone()) {
            return Err(CorpusError::DuplicateClipId(clip.id.clone()));
        }
    }
    Ok(())
}

/// Word Error Rate between `reference` and `hypothesis`.
///
/// Computed as token-level Levenshtein distance divided by the
/// reference token count. Returns `0.0` on empty reference (caller is
/// responsible for filtering empty refs out of the assertion gate).
/// CJK languages without whitespace fall back to character-level
/// segmentation so Mandarin / Japanese / Thai don't return artificially
/// low WER from one-token references.
#[must_use]
#[allow(clippy::cast_precision_loss)]
pub fn word_error_rate(reference: &str, hypothesis: &str) -> f64 {
    let ref_tokens = tokens_for(reference);
    let hyp_tokens = tokens_for(hypothesis);
    if ref_tokens.is_empty() {
        return 0.0;
    }
    let dist = levenshtein(&ref_tokens, &hyp_tokens);
    let denom = ref_tokens.len() as f64;
    (dist as f64) / denom
}

/// Token sequence for a phrase. Whitespace tokenization first; if that
/// produces a single token *and* the input contains non-Latin script,
/// fall back to character-level segmentation. This handles the
/// no-whitespace languages in the manifest (zh, ja).
fn tokens_for(text: &str) -> Vec<String> {
    let lower = text.to_lowercase();
    let normalized: String = lower
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { ' ' })
        .collect();
    let whitespace_tokens: Vec<String> =
        normalized.split_whitespace().map(str::to_string).collect();
    if whitespace_tokens.len() <= 1 && contains_non_ascii_letter(text) {
        return text
            .chars()
            .filter(|c| c.is_alphanumeric())
            .map(|c| c.to_string())
            .collect();
    }
    whitespace_tokens
}

fn contains_non_ascii_letter(text: &str) -> bool {
    text.chars().any(|c| c.is_alphabetic() && !c.is_ascii())
}

/// Standard token-level Levenshtein distance via two-row DP.
#[allow(clippy::needless_range_loop)]
fn levenshtein(a: &[String], b: &[String]) -> usize {
    if a.is_empty() {
        return b.len();
    }
    if b.is_empty() {
        return a.len();
    }
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    let mut curr: Vec<usize> = vec![0; b.len() + 1];
    for i in 1..=a.len() {
        curr[0] = i;
        for j in 1..=b.len() {
            let cost = usize::from(a[i - 1] != b[j - 1]);
            curr[j] = (prev[j] + 1).min(curr[j - 1] + 1).min(prev[j - 1] + cost);
        }
        std::mem::swap(&mut prev, &mut curr);
    }
    prev[b.len()]
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    fn write_manifest(text: &str) -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("MANIFEST.toml"), text).unwrap();
        dir
    }

    #[test]
    fn test_load_manifest_returns_not_found_for_absent_file() {
        let err = load_manifest(Path::new("/no/such/MANIFEST.toml"))
            .err()
            .unwrap();

        match err {
            CorpusError::NotFound { .. } => {}
            other => panic!("expected NotFound, got {other:?}"),
        }
    }

    #[test]
    fn test_load_manifest_parses_minimal_well_formed_input() {
        let dir = write_manifest(
            r#"
schema_version = 1

[[clips]]
id = "en-01"
language = "en"
duration_secs_max = 6.0
expected_text = "hello world"
provenance = "manual"
"#,
        );

        let manifest = load_manifest(&dir.path().join("MANIFEST.toml")).unwrap();

        assert_eq!(manifest.schema_version, 1);
        assert_eq!(manifest.clips.len(), 1);
        assert_eq!(manifest.clips[0].id, "en-01");
    }

    #[test]
    fn test_load_manifest_rejects_future_schema_version() {
        let dir = write_manifest(
            r#"
schema_version = 99

[[clips]]
id = "en-01"
language = "en"
duration_secs_max = 1.0
expected_text = "x"
provenance = "y"
"#,
        );

        let err = load_manifest(&dir.path().join("MANIFEST.toml"))
            .err()
            .unwrap();

        match err {
            CorpusError::UnsupportedSchemaVersion { found, target } => {
                assert_eq!(found, 99);
                assert_eq!(target, CURRENT_MANIFEST_VERSION);
            }
            other => panic!("expected UnsupportedSchemaVersion, got {other:?}"),
        }
    }

    #[test]
    fn test_load_manifest_rejects_empty_clips_array() {
        let dir = write_manifest("schema_version = 1\nclips = []\n");

        let err = load_manifest(&dir.path().join("MANIFEST.toml"))
            .err()
            .unwrap();

        assert!(matches!(err, CorpusError::Empty));
    }

    #[test]
    fn test_load_manifest_rejects_duplicate_clip_ids() {
        let dir = write_manifest(
            r#"
schema_version = 1

[[clips]]
id = "en-01"
language = "en"
duration_secs_max = 1.0
expected_text = "x"
provenance = "y"

[[clips]]
id = "en-01"
language = "en"
duration_secs_max = 1.0
expected_text = "z"
provenance = "y"
"#,
        );

        let err = load_manifest(&dir.path().join("MANIFEST.toml"))
            .err()
            .unwrap();

        match err {
            CorpusError::DuplicateClipId(id) => assert_eq!(id, "en-01"),
            other => panic!("expected DuplicateClipId, got {other:?}"),
        }
    }

    #[test]
    fn test_load_manifest_rejects_unknown_field_in_clip() {
        let dir = write_manifest(
            r#"
schema_version = 1

[[clips]]
id = "en-01"
language = "en"
duration_secs_max = 1.0
expected_text = "x"
provenance = "y"
unknown_extra = true
"#,
        );

        let err = load_manifest(&dir.path().join("MANIFEST.toml"))
            .err()
            .unwrap();

        assert!(matches!(err, CorpusError::Parse(_)));
    }

    #[test]
    fn test_word_error_rate_returns_zero_for_identical_strings() {
        let wer = word_error_rate("hello world", "hello world");

        assert!((wer - 0.0).abs() < f64::EPSILON);
    }

    #[test]
    fn test_word_error_rate_one_substitution_yields_one_over_n() {
        let wer = word_error_rate("hello there world", "hello here world");

        // 1 substitution / 3 reference tokens = 0.333…
        assert!((wer - 1.0 / 3.0).abs() < 1e-6);
    }

    #[test]
    fn test_word_error_rate_handles_complete_mismatch() {
        let wer = word_error_rate("alpha beta gamma", "x y z");

        // 3 substitutions / 3 reference tokens = 1.0
        assert!((wer - 1.0).abs() < 1e-6);
    }

    #[test]
    fn test_word_error_rate_empty_reference_returns_zero() {
        let wer = word_error_rate("", "anything");

        assert!((wer - 0.0).abs() < f64::EPSILON);
    }

    #[test]
    fn test_word_error_rate_normalizes_punctuation_and_case() {
        let wer = word_error_rate("Hello, world!", "hello world");

        assert!((wer - 0.0).abs() < f64::EPSILON);
    }

    #[test]
    fn test_word_error_rate_falls_back_to_character_segmentation_for_cjk() {
        // Chinese without whitespace: must not return 1.0 from a
        // one-token vs many-token comparison.
        let wer = word_error_rate("我认为我们应该", "我认为我们应该");

        assert!((wer - 0.0).abs() < f64::EPSILON);
    }

    #[test]
    fn test_word_error_rate_cjk_partial_mismatch_scales_with_character_distance() {
        let wer = word_error_rate("我们应该讨论", "我们必须讨论");

        // 6-char reference, 2 character substitutions (应→必, 该→须)
        // → WER = 2/6 = 1/3.
        assert!((wer - 1.0 / 3.0).abs() < 1e-6);
    }

    #[test]
    fn test_real_repo_manifest_parses_and_lists_twenty_clips() {
        let workspace_root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .to_path_buf();
        let manifest_path = workspace_root
            .join("tests")
            .join("fixtures")
            .join("multilingual")
            .join("MANIFEST.toml");

        let manifest = load_manifest(&manifest_path).unwrap();

        assert_eq!(manifest.clips.len(), 20);
        let languages: std::collections::BTreeSet<&str> =
            manifest.clips.iter().map(|c| c.language.as_str()).collect();
        assert_eq!(languages.len(), 10);
    }
}
