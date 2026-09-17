// Copyright 2026 Mathews Tom
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//     https://www.apache.org/licenses/LICENSE-2.0

//! Driving model acquisition from the qualification harness.
//!
//! The checked-in catalog names a half-gigabyte artifact on a public
//! host. A qualification run must not request it, and could not
//! usefully assert anything about half a gigabyte arriving if it did —
//! so the harness serves its own artifact on loopback and hands the
//! application a manifest describing it. Everything below that is the
//! real thing: the real `ModelManager`, the real confirmation gate, the
//! real free-space preflight, the real `.partial`, the real size and
//! digest verification, and the real rename.
//!
//! What this adds is a way in, not a second implementation. There is no
//! branch here that a production path does not also take, and nothing
//! here decides an outcome — the manager does, and the outcome is
//! written to the lifecycle record for the harness to read.
//!
//! `#[cfg(debug_assertions)]` in full, like the channel it is reached
//! through. `scripts/qualify-desktop-app.py` asserts the verb names
//! below are absent from a release binary, which is what "compiled
//! out" means rather than "present but unreachable".

use std::sync::Arc;

use scrybe_application::models::{
    DownloadProgress, InstallReport, ModelConfirmation, ModelFailure, ModelManifest, ModelState,
};
use tauri::Manager as _;

use crate::state::Desktop;

// The `probe-` prefix is not decoration. The harness proves these are
// absent from a release binary by searching its string pool, and the
// generated access-control manifest carries permission names like
// `allow-cancel-model-install` and `allow-model-offer` — so a verb
// named `model-install` would be found in every release build, and the
// check would fail against a binary that is in fact clean.
/// Read the plan for a harness-supplied manifest without fetching it.
pub const OFFER: &str = "probe-model-offer";
/// Fetch a harness-supplied manifest, with or without a confirmation.
pub const INSTALL: &str = "probe-model-install";
/// Ask whichever fetch is running to stop.
pub const CANCEL: &str = "probe-model-cancel";

/// The confirmation mode the harness asks for.
enum Mode {
    /// Confirm the digest the manifest names, which is what a user who
    /// read the offer would hand back.
    Confirmed,
    /// Confirm something else, which is what a caller that skipped the
    /// offer could produce. Must be refused before anything is opened.
    Unconfirmed,
}

/// Whether `verb` is one of this module's, and runs it if so.
///
/// Returns `false` for anything else, so the caller can fall through to
/// the tray identities it dispatches by default.
#[must_use]
pub fn run(app: &tauri::AppHandle, verb: &str) -> bool {
    let mut words = verb.split_whitespace();
    match words.next() {
        Some(OFFER) => {
            offer(app);
            true
        }
        Some(INSTALL) => {
            install(app, &words.collect::<Vec<_>>());
            true
        }
        Some(CANCEL) => {
            cancel(app);
            true
        }
        _ => false,
    }
}

/// Reads the catalog plan and records it.
///
/// The harness asserts its fixture server saw nothing after this, which
/// is how "the offer opens no connection" is observed on the running
/// application rather than inferred from the unit tests.
fn offer(app: &tauri::AppHandle) {
    let desktop = app.state::<Desktop>();
    let models = desktop.application().models();
    match models.catalog() {
        Ok(plans) => {
            let summary = plans
                .iter()
                .map(|plan| format!("{}:{}", plan.id, discriminant(&plan.state)))
                .collect::<Vec<_>>()
                .join(",");
            crate::note!(app, OFFER, &summary);
        }
        Err(error) => crate::note!(app, "probe-model-offer-failed", error.message()),
    }
}

/// Fetches `url`, judged against `size` and `digest`.
///
/// Arguments, in order: the artifact URL, its exact byte count, its
/// lowercase SHA-256, the destination filename, and the confirmation
/// mode. Every one of them comes from the harness, which built the
/// artifact and therefore knows all of them — and can deliberately get
/// one wrong, which is how the failure paths are exercised.
fn install(app: &tauri::AppHandle, arguments: &[&str]) {
    let [url, size, digest, destination, mode] = arguments else {
        crate::note!(
            app,
            "probe-model-install-failed",
            "expected: <url> <size> <sha256> <destination> confirmed|unconfirmed"
        );
        return;
    };
    let Ok(size_bytes) = size.parse::<u64>() else {
        crate::note!(
            app,
            "probe-model-install-failed",
            "the size is not a number"
        );
        return;
    };
    let mode = match *mode {
        "confirmed" => Mode::Confirmed,
        "unconfirmed" => Mode::Unconfirmed,
        other => {
            crate::note!(app, "probe-model-install-failed", other);
            return;
        }
    };

    let manifest = ModelManifest {
        id: "harness-fixture".to_string(),
        source_url: (*url).to_string(),
        source_revision: "0000000000000000000000000000000000000000".to_string(),
        license: "MIT".to_string(),
        size_bytes,
        sha256: (*digest).to_string(),
        runtime: "whisper-rs 0.13 (GGML)".to_string(),
        destination: (*destination).to_string(),
    };
    let confirmation = match mode {
        Mode::Confirmed => ModelConfirmation::new(&manifest.id, &manifest.sha256),
        // A digest the offer never showed. The manager compares it
        // against the manifest before it opens anything, so this is the
        // shape a caller that skipped the offer would produce.
        Mode::Unconfirmed => ModelConfirmation::new(&manifest.id, "0".repeat(64)),
    };

    let token = app.state::<Arc<crate::setup::ModelAcquisition>>().begin();
    let handle = app.clone();
    tauri::async_runtime::spawn(async move {
        let progress = |progress: DownloadProgress| {
            // Coarse, and only so the harness can tell a download that
            // started from one that never did.
            let _ = progress;
        };
        let outcome = handle
            .state::<Desktop>()
            .application()
            .models()
            .install_manifest(&manifest, &confirmation, &token, &progress)
            .await;
        handle
            .state::<Arc<crate::setup::ModelAcquisition>>()
            .finish();
        match outcome {
            Ok(report) => crate::note!(&handle, INSTALL, &describe(&report)),
            Err(error) => {
                crate::note!(&handle, INSTALL, &format!("refused:{}", error.code()));
            }
        }
    });
}

fn cancel(app: &tauri::AppHandle) {
    let running = app.state::<Arc<crate::setup::ModelAcquisition>>().cancel();
    crate::note!(app, CANCEL, if running { "running" } else { "idle" });
}

/// The outcome, as one line the harness compares exactly.
///
/// A failure carries which failure. Collapsing every one to `failed`
/// made the free-space, digest, size and transport outcomes
/// indistinguishable in the scenario's evidence, so four checks that
/// exercise four different refusals all asserted the same string and
/// none of them would have noticed the manager taking the wrong one.
fn describe(report: &InstallReport) -> String {
    match &report.state {
        ModelState::Failed { reason } => format!(
            "failed:{}:promoted={}",
            failure_kind(reason),
            report.promoted
        ),
        state => format!("{}:promoted={}", discriminant(state), report.promoted),
    }
}

const fn discriminant(state: &ModelState) -> &'static str {
    match state {
        ModelState::Available => "available",
        ModelState::Downloading { .. } => "downloading",
        ModelState::Verifying => "verifying",
        ModelState::Ready => "ready",
        ModelState::Cancelled => "cancelled",
        ModelState::Failed { .. } => "failed",
    }
}

/// Which refusal a failure was. The summaries the variants carry are
/// prose a person reads; this is the discriminant a gate compares.
const fn failure_kind(reason: &ModelFailure) -> &'static str {
    match reason {
        ModelFailure::SizeMismatch { .. } => "size_mismatch",
        ModelFailure::DigestMismatch { .. } => "digest_mismatch",
        ModelFailure::InsufficientSpace { .. } => "insufficient_space",
        ModelFailure::Transport { .. } => "transport",
        ModelFailure::Storage { .. } => "storage",
        ModelFailure::InstalledArtifactUnrecognized { .. } => "installed_artifact_unrecognized",
    }
}
