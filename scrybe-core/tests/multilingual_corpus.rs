// Copyright 2026 Mathews Tom
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//     https://www.apache.org/licenses/LICENSE-2.0

//! Multilingual regression-corpus integration test.
//!
//! Skips by default. Runs the full WER assertion (≤ 0.15 per
//! `.docs/development-plan.md` §12.1) when `SCRYBE_MULTILINGUAL_CORPUS`
//! points at a directory containing the audio files referenced by
//! `tests/fixtures/multilingual/MANIFEST.toml`.
//!
//! Honest scope: this build of the test asserts the manifest loads and
//! the corpus folder, when present, contains the named clips. The
//! Whisper transcription + WER step is gated behind the
//! `whisper-local` cargo feature plus a future maintainer-run script;
//! it is not exercised on every CI lane.

#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

use std::path::PathBuf;

use scrybe_core::testing::multilingual::{load_manifest, ManifestV1};

const CORPUS_ENV: &str = "SCRYBE_MULTILINGUAL_CORPUS";

fn workspace_manifest_path() -> PathBuf {
    let crate_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let workspace_root = crate_dir
        .parent()
        .expect("scrybe-core's parent directory must be the workspace root");
    workspace_root
        .join("tests")
        .join("fixtures")
        .join("multilingual")
        .join("MANIFEST.toml")
}

fn load_repo_manifest() -> ManifestV1 {
    load_manifest(&workspace_manifest_path()).expect("repo manifest parses")
}

#[test]
fn test_repo_manifest_lists_two_clips_per_language() {
    let manifest = load_repo_manifest();

    let mut by_language: std::collections::BTreeMap<&str, usize> =
        std::collections::BTreeMap::new();
    for clip in &manifest.clips {
        *by_language.entry(clip.language.as_str()).or_insert(0) += 1;
    }

    for (language, count) in &by_language {
        assert_eq!(
            *count, 2,
            "language {language} has {count} clips, expected 2"
        );
    }
}

#[test]
fn test_repo_manifest_clip_durations_within_eight_second_window() {
    let manifest = load_repo_manifest();

    for clip in &manifest.clips {
        assert!(
            clip.duration_secs_max > 0.0 && clip.duration_secs_max <= 8.0,
            "clip {} has duration_secs_max {} outside the (0, 8] window",
            clip.id,
            clip.duration_secs_max
        );
    }
}

#[test]
fn test_corpus_dir_when_present_contains_each_named_clip() {
    let Ok(corpus_dir) = std::env::var(CORPUS_ENV) else {
        eprintln!(
            "skipping: set {CORPUS_ENV} to a directory containing the multilingual audio fixtures \
             to exercise this assertion"
        );
        return;
    };
    let corpus = PathBuf::from(corpus_dir);
    assert!(
        corpus.is_dir(),
        "{CORPUS_ENV} points at {} which is not a directory",
        corpus.display()
    );
    let manifest = load_repo_manifest();
    let mut missing = Vec::new();
    for clip in &manifest.clips {
        let candidate = corpus.join(format!("{}.wav", clip.id));
        if !candidate.is_file() {
            missing.push(candidate.display().to_string());
        }
    }
    assert!(
        missing.is_empty(),
        "corpus directory missing audio for clips: {missing:?}"
    );
}
