// Copyright 2026 Mathews Tom
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//     https://www.apache.org/licenses/LICENSE-2.0

//! Black-box `scrybe bench` / `scrybe bench stt` contract tests: CLI
//! parsing, explicit feature-gate rejection, and unchanged existing
//! Criterion-harvest behavior, driven against the real compiled binary
//! (same convention as `tests/repair_sigkill.rs`). Inline unit tests in
//! `scrybe-cli/src/commands/bench_stt.rs::tests` cover the lifecycle timing
//! boundary with stub providers because real model weights are intentionally
//! absent from the test suite.
//!
//! `SHERPA_ONNX_LIB_DIR` contract coverage needs a binary built with both
//! `whisper-local` and `stt-sherpa`; that test is gated accordingly and runs
//! only when both features are enabled.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::path::Path;
use std::process::Command;

const fn scrybe_bin() -> &'static str {
    env!("CARGO_BIN_EXE_scrybe")
}

fn write_estimate(dir: &Path, id_path: &str, mean: f64, median: f64, std_dev: f64) {
    let leaf = dir.join(id_path).join("new");
    std::fs::create_dir_all(&leaf).unwrap();
    std::fs::write(
        leaf.join("estimates.json"),
        format!(
            r#"{{"mean":{{"point_estimate":{mean}}},"median":{{"point_estimate":{median}}},"std_dev":{{"point_estimate":{std_dev}}}}}"#
        ),
    )
    .unwrap();
}

#[test]
fn test_bench_stt_help_shows_required_flags() {
    let output = Command::new(scrybe_bin())
        .args(["bench", "stt", "--help"])
        .output()
        .unwrap();

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    for flag in ["--corpus", "--whisper-model", "--sherpa-model"] {
        assert!(stdout.contains(flag), "--help missing {flag}: {stdout}");
    }
}

#[test]
fn test_bench_help_lists_stt_subcommand_without_dropping_existing_flags() {
    let output = Command::new(scrybe_bin())
        .args(["bench", "--help"])
        .output()
        .unwrap();

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    for token in ["--criterion-dir", "--root", "--git-sha", "--print", "stt"] {
        assert!(stdout.contains(token), "--help missing {token}: {stdout}");
    }
}

#[test]
fn test_bench_stt_requires_every_model_and_corpus_argument() {
    for missing_flag in ["--corpus", "--whisper-model", "--sherpa-model"] {
        let mut args = vec!["bench", "stt"];
        if missing_flag != "--corpus" {
            args.extend(["--corpus", "corpus.toml"]);
        }
        if missing_flag != "--whisper-model" {
            args.extend(["--whisper-model", "model.bin"]);
        }
        if missing_flag != "--sherpa-model" {
            args.extend(["--sherpa-model", "sherpa-dir"]);
        }

        let output = Command::new(scrybe_bin()).args(args).output().unwrap();

        assert!(!output.status.success());
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(stderr.contains(missing_flag), "stderr: {stderr}");
    }
}

#[cfg(not(feature = "whisper-local"))]
#[test]
fn test_bench_stt_requires_whisper_local_feature_when_binary_built_without_it() {
    let output = Command::new(scrybe_bin())
        .args([
            "bench",
            "stt",
            "--corpus",
            "/tmp/no-such-corpus.toml",
            "--whisper-model",
            "/tmp/no-such-model.bin",
            "--sherpa-model",
            "/tmp/no-such-sherpa-dir",
        ])
        .output()
        .unwrap();

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("whisper-local"), "stderr: {stderr}");
    assert!(stderr.contains("bench stt"), "stderr: {stderr}");
}

#[test]
fn test_bench_rejects_criterion_flags_with_stt_mode() {
    let output = Command::new(scrybe_bin())
        .args([
            "bench",
            "--print",
            "stt",
            "--corpus",
            "corpus.toml",
            "--whisper-model",
            "model.bin",
            "--sherpa-model",
            "sherpa-dir",
        ])
        .output()
        .unwrap();

    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("--print"));
}

#[test]
fn test_bench_print_criterion_mode_still_works_unchanged() {
    let workdir = tempfile::tempdir().unwrap();
    let crit = workdir.path().join("crit");
    write_estimate(&crit, "vad/decide/1", 100.0, 99.0, 5.0);

    let output = Command::new(scrybe_bin())
        .arg("bench")
        .arg("--criterion-dir")
        .arg(&crit)
        .args(["--git-sha", "abcdef", "--print"])
        .output()
        .unwrap();

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    let parsed: serde_json::Value = serde_json::from_str(&stdout).expect("valid JSON snapshot");
    assert_eq!(parsed["git_sha"], "abcdef");
    assert_eq!(parsed["benches"].as_array().unwrap().len(), 1);
}

/// Needs both `whisper-local` and `stt-sherpa` compiled in: `run()` builds
/// the whisper provider first, so a binary without `whisper-local` never
/// reaches the `SHERPA_ONNX_LIB_DIR` check this exercises. Not part of the
/// current CI matrix (which tests default and `--features stt-sherpa`
/// separately, never combined); run locally with
/// `cargo test -p scrybe-cli --features whisper-local,stt-sherpa`.
#[cfg(all(feature = "whisper-local", feature = "stt-sherpa"))]
#[test]
fn test_bench_stt_rejects_unset_sentinel_and_unprovisioned_sherpa_runtime_dir() {
    let base_args = [
        "bench",
        "stt",
        "--corpus",
        "/tmp/no-such-corpus.toml",
        "--whisper-model",
        "/tmp/no-such-model.bin",
        "--sherpa-model",
        "/tmp/no-such-sherpa-model-dir",
    ];

    let unset = Command::new(scrybe_bin())
        .args(base_args)
        .env_remove("SHERPA_ONNX_LIB_DIR")
        .output()
        .unwrap();
    assert!(!unset.status.success());
    assert!(String::from_utf8_lossy(&unset.stderr).contains("is not set"));

    let sentinel = Command::new(scrybe_bin())
        .args(base_args)
        .env(
            "SHERPA_ONNX_LIB_DIR",
            "__scrybe_requires_explicit_sherpa_runtime__",
        )
        .output()
        .unwrap();
    assert!(!sentinel.status.success());
    assert!(String::from_utf8_lossy(&sentinel.stderr).contains("sentinel"));

    let empty_dir = tempfile::tempdir().unwrap();
    let unprovisioned = Command::new(scrybe_bin())
        .args(base_args)
        .env("SHERPA_ONNX_LIB_DIR", empty_dir.path())
        .output()
        .unwrap();
    assert!(!unprovisioned.status.success());
    assert!(
        String::from_utf8_lossy(&unprovisioned.stderr).contains("does not contain a provisioned")
    );
}
