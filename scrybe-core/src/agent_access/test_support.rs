// Copyright 2026 Mathews Tom
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//     https://www.apache.org/licenses/LICENSE-2.0

//! Fixture-building and tree-snapshot helpers shared by
//! `agent_access`'s `reader` and `protocol` unit tests.
//!
//! Test-only; gated on `#[cfg(test)]` at the `mod test_support;`
//! declaration in `agent_access::mod`.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use serde::Serialize;

/// Minimal, real-`toml`-crate-encoded stand-in for `MetaTomlV1`
/// (`session.rs`). Generated through `toml::to_string` rather than a
/// hand-written literal so the on-disk datetime format always matches
/// whatever the `toml` crate version in use actually emits.
#[derive(Serialize)]
struct FixtureMeta {
    session_id: String,
    title: Option<String>,
    started_at: DateTime<Utc>,
    ended_at: DateTime<Utc>,
    duration_secs: u64,
}

/// Writes a complete "finished" session folder: `meta.toml`,
/// `notes.md`, and `transcript.md`, with no leftover `journal/`.
pub(super) fn write_finished_session(
    root: &Path,
    folder: &str,
    title: &str,
    duration_secs: u64,
    notes: &str,
    transcript: &str,
) {
    let dir = root.join(folder);
    std::fs::create_dir_all(&dir).expect("create session dir");
    let now = Utc::now();
    let meta = FixtureMeta {
        session_id: folder.to_string(),
        title: Some(title.to_string()),
        started_at: now,
        ended_at: now,
        duration_secs,
    };
    let body = toml::to_string(&meta).expect("encode fixture meta.toml");
    std::fs::write(dir.join("meta.toml"), body).expect("write meta.toml");
    std::fs::write(dir.join("notes.md"), notes).expect("write notes.md");
    std::fs::write(dir.join("transcript.md"), transcript).expect("write transcript.md");
}

/// Writes an "unfinished" session folder per the M2 invariant:
/// `journal/` present, no `audio.opus`, no `meta.toml`.
pub(super) fn write_unfinished_session(root: &Path, folder: &str) {
    let journal = root.join(folder).join("journal");
    std::fs::create_dir_all(&journal).expect("create journal dir");
    std::fs::write(journal.join("mic.f32"), [0_u8; 8]).expect("write journal segment");
}

/// Recursively snapshots every regular file under `root` as a map from
/// its path relative to `root` to its exact byte content. Two
/// snapshots of the same tree are equal only if the file set and every
/// file's bytes are unchanged — any write, delete, create, or rename
/// anywhere under `root` changes the result.
pub(super) fn snapshot_tree(root: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    let mut out = BTreeMap::new();
    walk(root, root, &mut out);
    out
}

fn walk(root: &Path, dir: &Path, out: &mut BTreeMap<PathBuf, Vec<u8>>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            walk(root, &path, out);
        } else if let Ok(bytes) = std::fs::read(&path) {
            let rel = path.strip_prefix(root).unwrap_or(&path).to_path_buf();
            out.insert(rel, bytes);
        }
    }
}
