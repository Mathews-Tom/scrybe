// Copyright 2026 Mathews Tom
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//     https://www.apache.org/licenses/LICENSE-2.0

//! `SIGKILL`-then-repair integration test (M2 PR-5 verification,
//! `.docs/EXECUTION_PROMPTS.md` M2 PR-5).
//!
//! Spawns the real `scrybe` binary recording a `--source synthetic`
//! session, `SIGKILL`s it mid-stream (no chance to run any shutdown
//! code — the same abrupt-death shape a `SIGKILL`, power loss, or OS
//! process-tree cleanup produces), then runs `scrybe repair` against
//! the killed session's folder and asserts the audio is recovered.
//!
//! `SCRYBE_TEST_SYNTHETIC_FRAME_DELAY_MS` (see
//! `scrybe-cli/src/commands/rec.rs::synthetic_frame_delay`) paces the
//! otherwise-instant synthetic generator so the subprocess is
//! reliably still mid-recording when this test sends the kill signal,
//! without relying on a wall-clock race against an unpaced generator.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

const fn scrybe_bin() -> &'static str {
    env!("CARGO_BIN_EXE_scrybe")
}

/// Finds the single session folder created under `root`, if the
/// process has created it yet. There is at most one because each
/// test uses its own fresh temp root.
fn try_find_session_folder(root: &Path) -> Option<PathBuf> {
    std::fs::read_dir(root)
        .ok()?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .find(|p| p.is_dir())
}

/// True once `journal/manifest.toml` exists and at least one
/// `journal/*.f32` segment has bytes on disk. Both are required for
/// `scrybe repair` to succeed: `manifest.toml` (written by the async
/// task via `atomic_replace` — create temp file, write, fsync,
/// rename) and a segment's bytes (written by the journal writer's
/// own OS thread) land through two independent code paths with no
/// ordering guarantee relative to each other. Polling for only one
/// of them risks a real race: killing after the segment has bytes
/// but before the manifest's rename lands leaves `scrybe repair`
/// unable to even read the anchor it needs.
fn journal_ready_for_repair(journal_dir: &Path) -> bool {
    if !journal_dir.join("manifest.toml").exists() {
        return false;
    }
    let Ok(entries) = std::fs::read_dir(journal_dir) else {
        return false;
    };
    entries.filter_map(Result::ok).any(|entry| {
        entry.path().extension().is_some_and(|ext| ext == "f32")
            && entry.metadata().is_ok_and(|m| m.len() > 0)
    })
}

#[test]
fn test_sigkill_mid_recording_then_repair_recovers_audio() {
    let root_dir = tempfile::tempdir().unwrap();
    let cfg_dir = tempfile::tempdir().unwrap();

    let mut child = Command::new(scrybe_bin())
        .args([
            "rec",
            "--title",
            "sigkill-test",
            "--root",
            root_dir.path().to_str().unwrap(),
            "--yes",
            "--source",
            "synthetic",
            // Long enough that a real 15ms-per-frame pace keeps the
            // process alive well past the sleep below; the process is
            // killed long before this would ever complete.
            "--synthetic-secs",
            "120",
        ])
        .env(
            "SCRYBE_CONFIG",
            cfg_dir.path().join("nonexistent-config.toml"),
        )
        .env("SCRYBE_TEST_SYNTHETIC_FRAME_DELAY_MS", "15")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("failed to spawn scrybe rec subprocess");

    // Poll until the journal has everything `scrybe repair` needs
    // (manifest.toml plus a non-empty segment) rather than sleeping a
    // fixed duration and hoping. System load (parallel test/compile
    // activity) makes any fixed sleep an unreliable proxy for "the
    // journal writer has started"; polling removes the race
    // regardless of actual startup latency, well before
    // `--synthetic-secs 120` could ever finish on its own.
    let poll_deadline = std::time::Instant::now() + Duration::from_secs(10);
    let session_root = loop {
        if let Some(folder) = try_find_session_folder(root_dir.path()) {
            if journal_ready_for_repair(&folder.join("journal")) {
                break folder;
            }
        }
        if let Ok(Some(status)) = child.try_wait() {
            panic!(
                "scrybe rec exited on its own before any journal bytes appeared ({status}); \
                 the frame-delay pacing did not keep it alive as expected"
            );
        }
        assert!(
            std::time::Instant::now() <= poll_deadline,
            "no non-empty journal segment appeared within 10s of spawning scrybe rec"
        );
        std::thread::sleep(Duration::from_millis(10));
    };

    child.kill().expect("failed to SIGKILL scrybe rec");
    let exit = child.wait().expect("failed to reap killed child");
    assert!(!exit.success(), "a killed process must not report success");

    let folder = session_root;
    assert!(
        folder.join("journal").is_dir(),
        "killed session must leave journal/ behind"
    );
    assert!(
        !folder.join("audio.opus").exists(),
        "killed session must never have reached the offline merge"
    );

    let repair_output = Command::new(scrybe_bin())
        .args(["repair", folder.to_str().unwrap()])
        .env(
            "SCRYBE_CONFIG",
            cfg_dir.path().join("nonexistent-config.toml"),
        )
        .stdin(Stdio::null())
        .output()
        .expect("failed to spawn scrybe repair subprocess");
    assert!(
        repair_output.status.success(),
        "scrybe repair must succeed against a SIGKILLed session's journal; stdout: {}\nstderr: {}",
        String::from_utf8_lossy(&repair_output.stdout),
        String::from_utf8_lossy(&repair_output.stderr)
    );

    assert!(
        folder.join("audio.opus").exists(),
        "scrybe repair must have written audio.opus"
    );
    assert!(
        !folder.join("journal").exists(),
        "scrybe repair must delete journal/ after a verified merge"
    );
    let audio_bytes = std::fs::metadata(folder.join("audio.opus")).unwrap().len();
    assert!(
        audio_bytes > 0,
        "recovered audio.opus must contain the samples pushed before the kill"
    );
}
