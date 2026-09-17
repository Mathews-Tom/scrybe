// Copyright 2026 Mathews Tom
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//     https://www.apache.org/licenses/LICENSE-2.0

//! What the model manager does with bytes, and what it refuses to do
//! before it has permission.
//!
//! Every test here runs in a build with no HTTP client compiled in.
//! That is the point of the transport seam: the confirmation gate, the
//! free-space preflight, cancellation, exact size and digest checks,
//! atomic promotion, and the refusal to touch an installed artifact
//! are properties of the manager, not of the network, and they are
//! asserted in the build the default gate runs.
//!
//! The source used here counts its own opens, so "no request was made"
//! is an observation rather than an inference. `tests/model_fetch.rs`
//! makes the same observation against a real local HTTP server in the
//! build that has one.
//!
//! The manifests are small and built here rather than read from the
//! checked-in catalog, whose artifact is half a gigabyte: fabricating
//! that per test is a disk-space problem, not a test. The catalog's
//! own contents are asserted where they belong, in
//! `tests/model_catalog.rs`, and the two are tied together by
//! [`test_the_catalog_entry_plans_without_opening_anything`], which
//! runs the real entry through the real planner.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use scrybe_application::cancellation::CancellationToken;
use scrybe_application::models::{
    ArtifactChunks, DownloadProgress, InstallReport, ModelConfirmation, ModelFailure, ModelManager,
    ModelManifest, ModelSource, ModelState, UnavailableSource, PARTIAL_SUFFIX,
};
use scrybe_application::{ApplicationError, ErrorCode, Result};

// -- sources ----------------------------------------------------------

/// Hands back bytes a test chose, and remembers every URL it served.
struct ScriptedSource {
    body: Vec<u8>,
    opened: Arc<Mutex<Vec<String>>>,
    /// Fails the open rather than serving anything.
    refuse: bool,
    /// Cancelled once the first chunk has been handed over, so a
    /// cancellation lands mid-stream rather than before it starts.
    cancel_after_first_chunk: Option<CancellationToken>,
}

impl ScriptedSource {
    fn serving(body: Vec<u8>) -> Self {
        Self {
            body,
            opened: Arc::new(Mutex::new(Vec::new())),
            refuse: false,
            cancel_after_first_chunk: None,
        }
    }

    fn refusing() -> Self {
        Self {
            refuse: true,
            ..Self::serving(Vec::new())
        }
    }
}

/// Four bytes at a time, so every test artifact arrives in several
/// chunks and the between-chunk checks are actually reached.
const CHUNK: usize = 4;

struct ScriptedChunks {
    body: Vec<u8>,
    offset: usize,
    cancel_after_first_chunk: Option<CancellationToken>,
    served: usize,
}

#[async_trait]
impl ArtifactChunks for ScriptedChunks {
    async fn next_chunk(&mut self) -> Result<Option<Vec<u8>>> {
        if self.offset >= self.body.len() {
            return Ok(None);
        }
        let end = (self.offset + CHUNK).min(self.body.len());
        let chunk = self.body[self.offset..end].to_vec();
        self.offset = end;
        self.served += 1;
        if self.served == 1 {
            if let Some(token) = &self.cancel_after_first_chunk {
                token.cancel();
            }
        }
        Ok(Some(chunk))
    }
}

#[async_trait]
impl ModelSource for ScriptedSource {
    async fn open(&self, url: &str) -> Result<Box<dyn ArtifactChunks>> {
        self.opened.lock().unwrap().push(url.to_string());
        if self.refuse {
            return Err(ApplicationError::new(
                ErrorCode::ModelDownloadUnavailable,
                "the fixture refused the request",
            ));
        }
        Ok(Box::new(ScriptedChunks {
            body: self.body.clone(),
            offset: 0,
            cancel_after_first_chunk: self.cancel_after_first_chunk.clone(),
            served: 0,
        }))
    }
}

/// Counts every attempt to open it, so a test can assert zero rather
/// than infer it from the absence of a side effect.
struct ForbiddenSource {
    opens: Arc<AtomicUsize>,
}

impl ForbiddenSource {
    fn new() -> (Arc<Self>, Arc<AtomicUsize>) {
        let opens = Arc::new(AtomicUsize::new(0));
        (
            Arc::new(Self {
                opens: Arc::clone(&opens),
            }),
            opens,
        )
    }
}

#[async_trait]
impl ModelSource for ForbiddenSource {
    async fn open(&self, _url: &str) -> Result<Box<dyn ArtifactChunks>> {
        self.opens.fetch_add(1, Ordering::SeqCst);
        Err(ApplicationError::new(
            ErrorCode::ModelDownloadUnavailable,
            "the forbidden source was opened",
        ))
    }
}

// -- fixture ----------------------------------------------------------

/// The bytes every scripted artifact is made of.
const ARTIFACT: &[u8] = b"a small stand-in for a GGML model file";

struct Fixture {
    dir: tempfile::TempDir,
}

impl Fixture {
    fn new() -> Self {
        Self {
            dir: tempfile::tempdir().unwrap(),
        }
    }

    fn models_dir(&self) -> PathBuf {
        self.dir.path().join("models")
    }

    fn destination(&self) -> PathBuf {
        self.models_dir().join("ggml-fixture.en.bin")
    }

    fn manager(&self, source: Arc<dyn ModelSource>) -> ModelManager {
        ModelManager::with_source(self.models_dir(), source)
    }

    fn partials(&self) -> Vec<String> {
        let Ok(entries) = std::fs::read_dir(self.models_dir()) else {
            return Vec::new();
        };
        let mut names: Vec<String> = entries
            .flatten()
            .filter_map(|entry| entry.file_name().to_str().map(str::to_string))
            .filter(|name| name.ends_with(PARTIAL_SUFFIX))
            .collect();
        names.sort();
        names
    }

    fn seed_destination(&self, bytes: &[u8]) {
        std::fs::create_dir_all(self.models_dir()).unwrap();
        std::fs::write(self.destination(), bytes).unwrap();
    }
}

/// A manifest describing exactly `ARTIFACT`.
fn manifest() -> ModelManifest {
    manifest_for(ARTIFACT)
}

fn manifest_for(body: &[u8]) -> ModelManifest {
    ModelManifest {
        id: "fixture-en".into(),
        source_url: "https://models.invalid/repo/resolve/\
                     0123456789abcdef0123456789abcdef01234567/ggml-fixture.en.bin"
            .into(),
        source_revision: "0123456789abcdef0123456789abcdef01234567".into(),
        license: "MIT".into(),
        size_bytes: body.len() as u64,
        sha256: sha256_hex(body),
        runtime: "whisper-rs 0.13 (GGML)".into(),
        destination: "ggml-fixture.en.bin".into(),
    }
}

fn confirming(manifest: &ModelManifest) -> ModelConfirmation {
    ModelConfirmation::new(&manifest.id, &manifest.sha256)
}

const fn nothing(_: DownloadProgress) {}

fn install(
    fixture: &Fixture,
    source: Arc<dyn ModelSource>,
    manifest: &ModelManifest,
    confirmation: &ModelConfirmation,
    cancel: &CancellationToken,
) -> Result<InstallReport> {
    let manager = fixture.manager(source);
    runtime().block_on(manager.install_manifest(manifest, confirmation, cancel, &nothing))
}

fn runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
}

fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::Digest as _;
    let mut hasher = sha2::Sha256::new();
    hasher.update(bytes);
    format!("{:x}", hasher.finalize())
}

// -- the confirmation gate --------------------------------------------

#[test]
fn test_no_request_is_made_for_a_digest_the_user_was_not_shown() {
    let fixture = Fixture::new();
    let (source, opens) = ForbiddenSource::new();

    let refused = install(
        &fixture,
        source,
        &manifest(),
        &ModelConfirmation::new("fixture-en", "not the digest that was offered"),
        &CancellationToken::new(),
    )
    .unwrap_err();

    assert_eq!(refused.code(), ErrorCode::ModelConfirmationRequired);
    assert_eq!(
        opens.load(Ordering::SeqCst),
        0,
        "the source was opened for an artifact the user had not confirmed"
    );
}

#[test]
fn test_no_request_is_made_for_a_confirmation_naming_another_model() {
    let fixture = Fixture::new();
    let manifest = manifest();
    let (source, opens) = ForbiddenSource::new();

    let refused = install(
        &fixture,
        source,
        &manifest,
        &ModelConfirmation::new("some-other-model", &manifest.sha256),
        &CancellationToken::new(),
    )
    .unwrap_err();

    assert_eq!(refused.code(), ErrorCode::ModelConfirmationRequired);
    assert_eq!(opens.load(Ordering::SeqCst), 0);
}

#[test]
fn test_planning_opens_nothing_and_reports_everything_a_prompt_needs() {
    let fixture = Fixture::new();
    let (source, opens) = ForbiddenSource::new();
    let manifest = manifest();

    let plan = fixture.manager(source).plan_for(&manifest).unwrap();

    assert_eq!(opens.load(Ordering::SeqCst), 0);
    assert_eq!(plan.state, ModelState::Available);
    assert_eq!(plan.size_bytes, manifest.size_bytes);
    assert_eq!(plan.sha256, manifest.sha256);
    assert_eq!(plan.source_revision, manifest.source_revision);
    assert!(
        plan.required_bytes > plan.size_bytes,
        "the plan asks for no more room than the artifact itself"
    );
}

#[test]
fn test_the_catalog_entry_plans_without_opening_anything() {
    let fixture = Fixture::new();
    let (source, opens) = ForbiddenSource::new();

    let plan = fixture.manager(source).plan("whisper-small-en").unwrap();

    assert_eq!(opens.load(Ordering::SeqCst), 0);
    assert_eq!(plan.state, ModelState::Available);
    assert!(plan.destination_path.ends_with("ggml-small.en.bin"));
}

// -- verification and promotion ---------------------------------------

#[test]
fn test_a_confirmed_artifact_that_verifies_is_promoted() {
    let fixture = Fixture::new();
    let manifest = manifest();

    let report = install(
        &fixture,
        Arc::new(ScriptedSource::serving(ARTIFACT.to_vec())),
        &manifest,
        &confirming(&manifest),
        &CancellationToken::new(),
    )
    .unwrap();

    assert_eq!(report.state, ModelState::Ready);
    assert!(report.promoted);
    assert_eq!(std::fs::read(fixture.destination()).unwrap(), ARTIFACT);
    assert!(
        fixture.partials().is_empty(),
        "a promoted artifact left its partial behind"
    );
}

#[test]
fn test_a_digest_mismatch_promotes_nothing_and_leaves_the_partial() {
    let fixture = Fixture::new();
    let manifest = manifest();
    // The right length, so only the digest can reject it.
    let substituted: Vec<u8> = ARTIFACT.iter().map(|byte| byte ^ 0x20).collect();

    let report = install(
        &fixture,
        Arc::new(ScriptedSource::serving(substituted)),
        &manifest,
        &confirming(&manifest),
        &CancellationToken::new(),
    )
    .unwrap();

    assert!(
        matches!(
            report.state,
            ModelState::Failed {
                reason: ModelFailure::DigestMismatch { .. }
            }
        ),
        "{:?}",
        report.state
    );
    assert!(!report.promoted);
    assert!(
        !fixture.destination().exists(),
        "an unverified artifact reached the destination name"
    );
    assert_eq!(
        fixture.partials().len(),
        1,
        "the partial was not left where the user can see it"
    );
}

#[test]
fn test_a_short_artifact_promotes_nothing() {
    let fixture = Fixture::new();
    let manifest = manifest();

    let report = install(
        &fixture,
        Arc::new(ScriptedSource::serving(ARTIFACT[..8].to_vec())),
        &manifest,
        &confirming(&manifest),
        &CancellationToken::new(),
    )
    .unwrap();

    assert!(
        matches!(
            report.state,
            ModelState::Failed {
                reason: ModelFailure::SizeMismatch { .. }
            }
        ),
        "{:?}",
        report.state
    );
    assert!(!fixture.destination().exists());
}

#[test]
fn test_an_overlong_artifact_promotes_nothing() {
    let fixture = Fixture::new();
    let manifest = manifest();
    let mut overlong = ARTIFACT.to_vec();
    overlong.extend_from_slice(b" and then some more");

    let report = install(
        &fixture,
        Arc::new(ScriptedSource::serving(overlong)),
        &manifest,
        &confirming(&manifest),
        &CancellationToken::new(),
    )
    .unwrap();

    assert!(
        matches!(
            report.state,
            ModelState::Failed {
                reason: ModelFailure::SizeMismatch { .. }
            }
        ),
        "{:?}",
        report.state
    );
    assert!(!fixture.destination().exists());
}

#[test]
fn test_a_transport_failure_promotes_nothing() {
    let fixture = Fixture::new();
    let manifest = manifest();

    let report = install(
        &fixture,
        Arc::new(ScriptedSource::refusing()),
        &manifest,
        &confirming(&manifest),
        &CancellationToken::new(),
    )
    .unwrap();

    assert!(
        matches!(
            report.state,
            ModelState::Failed {
                reason: ModelFailure::Transport { .. }
            }
        ),
        "{:?}",
        report.state
    );
    assert!(!fixture.destination().exists());
    assert!(
        fixture.partials().is_empty(),
        "an open that never produced a byte still created a partial"
    );
}

#[test]
fn test_a_build_without_a_transport_reports_that_rather_than_promoting() {
    let fixture = Fixture::new();
    let manifest = manifest();

    let report = install(
        &fixture,
        Arc::new(UnavailableSource),
        &manifest,
        &confirming(&manifest),
        &CancellationToken::new(),
    )
    .unwrap();

    assert!(
        matches!(
            report.state,
            ModelState::Failed {
                reason: ModelFailure::Transport { .. }
            }
        ),
        "{:?}",
        report.state
    );
    assert!(!fixture.destination().exists());
}

// -- free space --------------------------------------------------------

#[test]
fn test_an_artifact_larger_than_the_disk_is_refused_before_any_request() {
    let fixture = Fixture::new();
    let (source, opens) = ForbiddenSource::new();
    let mut manifest = manifest();
    // Larger than any filesystem will report free, so the preflight
    // decides rather than the write.
    manifest.size_bytes = u64::MAX / 4;
    let confirmation = confirming(&manifest);

    let report = install(
        &fixture,
        source,
        &manifest,
        &confirmation,
        &CancellationToken::new(),
    )
    .unwrap();

    assert!(
        matches!(
            report.state,
            ModelState::Failed {
                reason: ModelFailure::InsufficientSpace { .. }
            }
        ),
        "{:?}",
        report.state
    );
    assert_eq!(
        opens.load(Ordering::SeqCst),
        0,
        "a download with nowhere to land still opened the source"
    );
    assert!(fixture.partials().is_empty());
}

// -- cancellation ------------------------------------------------------

#[test]
fn test_a_cancellation_before_the_stream_starts_promotes_nothing() {
    let fixture = Fixture::new();
    let manifest = manifest();
    let cancel = CancellationToken::new();
    cancel.cancel();
    let (source, opens) = ForbiddenSource::new();

    let report = install(&fixture, source, &manifest, &confirming(&manifest), &cancel).unwrap();

    assert_eq!(report.state, ModelState::Cancelled);
    assert_eq!(
        opens.load(Ordering::SeqCst),
        0,
        "a cancelled install still opened the source"
    );
    assert!(!fixture.destination().exists());
}

#[test]
fn test_a_cancellation_part_way_through_promotes_nothing_and_leaves_the_partial() {
    let fixture = Fixture::new();
    let manifest = manifest();
    let cancel = CancellationToken::new();
    let mut source = ScriptedSource::serving(ARTIFACT.to_vec());
    source.cancel_after_first_chunk = Some(cancel.clone());

    let report = install(
        &fixture,
        Arc::new(source),
        &manifest,
        &confirming(&manifest),
        &cancel,
    )
    .unwrap();

    assert_eq!(report.state, ModelState::Cancelled);
    assert!(!report.promoted);
    assert!(
        !fixture.destination().exists(),
        "a cancelled download reached the destination name"
    );
    assert_eq!(fixture.partials().len(), 1);
}

// -- an artifact that is already there ---------------------------------

#[test]
fn test_an_installed_matching_model_is_ready_without_any_request() {
    let fixture = Fixture::new();
    let manifest = manifest();
    fixture.seed_destination(ARTIFACT);
    let (source, opens) = ForbiddenSource::new();

    let report = install(
        &fixture,
        source,
        &manifest,
        &confirming(&manifest),
        &CancellationToken::new(),
    )
    .unwrap();

    assert_eq!(report.state, ModelState::Ready);
    assert!(
        !report.promoted,
        "an install that changed nothing claimed to have promoted something"
    );
    assert_eq!(
        opens.load(Ordering::SeqCst),
        0,
        "an already-installed model was fetched again"
    );
    assert_eq!(std::fs::read(fixture.destination()).unwrap(), ARTIFACT);
}

#[test]
fn test_an_installed_matching_model_plans_as_ready() {
    let fixture = Fixture::new();
    fixture.seed_destination(ARTIFACT);

    let plan = fixture
        .manager(Arc::new(UnavailableSource))
        .plan_for(&manifest())
        .unwrap();

    assert_eq!(plan.state, ModelState::Ready);
}

/// Deciding whether an installed artifact is the catalog's means
/// hashing half a gigabyte, so the answer is memoised. The memo must
/// not outlive the file it describes.
#[test]
fn test_replacing_the_installed_artifact_is_noticed_rather_than_answered_from_the_last_reading() {
    let fixture = Fixture::new();
    let manifest = manifest();
    fixture.seed_destination(ARTIFACT);
    let manager = fixture.manager(Arc::new(UnavailableSource));
    assert_eq!(
        manager.plan_for(&manifest).unwrap().state,
        ModelState::Ready
    );

    // Same length, different bytes, and the modification time put back
    // exactly where it was. Moving the timestamp forward would prove
    // only that the memo notices a changed timestamp, which is not the
    // property: `rsync -t`, `cp -p`, `tar -x`, `unzip`, and a bare
    // `utimes` all preserve it, so a substitution that leaves the
    // length and the modification time alone is the ordinary case
    // rather than an adversarial one.
    let original = std::fs::metadata(fixture.destination())
        .unwrap()
        .modified()
        .unwrap();
    let substituted: Vec<u8> = ARTIFACT.iter().map(|byte| byte ^ 0x20).collect();
    fixture.seed_destination(&substituted);
    std::fs::OpenOptions::new()
        .write(true)
        .open(fixture.destination())
        .unwrap()
        .set_modified(original)
        .unwrap();
    assert_eq!(
        std::fs::metadata(fixture.destination())
            .unwrap()
            .modified()
            .unwrap(),
        original,
        "the substitution was meant to leave the modification time untouched"
    );

    assert!(
        matches!(
            manager.plan_for(&manifest).unwrap().state,
            ModelState::Failed {
                reason: ModelFailure::InstalledArtifactUnrecognized { .. }
            }
        ),
        "a replaced artifact was reported from the previous reading"
    );
}

#[test]
fn test_an_installed_file_of_the_wrong_length_is_left_exactly_as_it_was() {
    let fixture = Fixture::new();
    let manifest = manifest();
    let existing = b"someone else's file".to_vec();
    fixture.seed_destination(&existing);

    let report = install(
        &fixture,
        Arc::new(ScriptedSource::serving(ARTIFACT.to_vec())),
        &manifest,
        &confirming(&manifest),
        &CancellationToken::new(),
    )
    .unwrap();

    assert!(
        matches!(
            report.state,
            ModelState::Failed {
                reason: ModelFailure::InstalledArtifactUnrecognized { .. }
            }
        ),
        "{:?}",
        report.state
    );
    assert_eq!(std::fs::read(fixture.destination()).unwrap(), existing);
}

#[test]
fn test_an_installed_file_of_the_right_length_that_hashes_wrong_is_left_alone() {
    let fixture = Fixture::new();
    let manifest = manifest();
    let substituted: Vec<u8> = ARTIFACT.iter().map(|byte| byte ^ 0x20).collect();
    fixture.seed_destination(&substituted);
    let (source, opens) = ForbiddenSource::new();

    let report = install(
        &fixture,
        source,
        &manifest,
        &confirming(&manifest),
        &CancellationToken::new(),
    )
    .unwrap();

    assert!(
        matches!(
            report.state,
            ModelState::Failed {
                reason: ModelFailure::InstalledArtifactUnrecognized { .. }
            }
        ),
        "{:?}",
        report.state
    );
    assert_eq!(opens.load(Ordering::SeqCst), 0);
    assert_eq!(std::fs::read(fixture.destination()).unwrap(), substituted);
}

// -- partials ----------------------------------------------------------

#[test]
fn test_a_stale_partial_is_reported_and_not_removed() {
    let fixture = Fixture::new();
    std::fs::create_dir_all(fixture.models_dir()).unwrap();
    let partial = fixture
        .models_dir()
        .join(format!("ggml-fixture.en.bin.7-0-1{PARTIAL_SUFFIX}"));
    std::fs::write(&partial, b"half a model").unwrap();

    let reported = fixture
        .manager(Arc::new(UnavailableSource))
        .stale_partials()
        .unwrap();

    assert_eq!(reported.len(), 1);
    assert_eq!(reported[0].bytes, 12);
    assert!(partial.exists(), "reporting a stale partial deleted it");
}

#[test]
fn test_the_installed_artifact_is_not_reported_as_a_partial() {
    let fixture = Fixture::new();
    fixture.seed_destination(ARTIFACT);

    assert!(fixture
        .manager(Arc::new(UnavailableSource))
        .stale_partials()
        .unwrap()
        .is_empty());
}

#[test]
fn test_a_cancelled_attempt_is_reported_as_a_stale_partial_afterwards() {
    let fixture = Fixture::new();
    let manifest = manifest();
    let cancel = CancellationToken::new();
    let mut source = ScriptedSource::serving(ARTIFACT.to_vec());
    source.cancel_after_first_chunk = Some(cancel.clone());
    install(
        &fixture,
        Arc::new(source),
        &manifest,
        &confirming(&manifest),
        &cancel,
    )
    .unwrap();

    let reported = fixture
        .manager(Arc::new(UnavailableSource))
        .stale_partials()
        .unwrap();

    assert_eq!(reported.len(), 1);
}

#[test]
fn test_two_failed_attempts_do_not_collide_on_one_partial_name() {
    let fixture = Fixture::new();
    let manifest = manifest();
    let substituted: Vec<u8> = ARTIFACT.iter().map(|byte| byte ^ 0x20).collect();

    for _ in 0..2 {
        install(
            &fixture,
            Arc::new(ScriptedSource::serving(substituted.clone())),
            &manifest,
            &confirming(&manifest),
            &CancellationToken::new(),
        )
        .unwrap();
    }

    assert_eq!(
        fixture.partials().len(),
        2,
        "two failed attempts collided on one partial name"
    );
}

// -- progress ----------------------------------------------------------

#[test]
fn test_progress_finishes_at_the_manifest_size() {
    let fixture = Fixture::new();
    let manifest = manifest();
    let seen: Arc<Mutex<Vec<DownloadProgress>>> = Arc::new(Mutex::new(Vec::new()));
    let recorder = Arc::clone(&seen);
    let manager = fixture.manager(Arc::new(ScriptedSource::serving(ARTIFACT.to_vec())));

    runtime()
        .block_on(manager.install_manifest(
            &manifest,
            &confirming(&manifest),
            &CancellationToken::new(),
            &move |progress| recorder.lock().unwrap().push(progress),
        ))
        .unwrap();

    let observed = seen.lock().unwrap().clone();
    assert_eq!(
        observed.last().map(|progress| progress.received_bytes),
        Some(manifest.size_bytes)
    );
}

/// The destination name must never exist before verification has
/// finished, which is what "atomic promotion" means in practice.
#[test]
fn test_the_destination_name_never_appears_before_verification() {
    let fixture = Fixture::new();
    let manifest = manifest();
    let destination = fixture.destination();
    let seen_early = Arc::new(AtomicUsize::new(0));
    let watcher = Arc::clone(&seen_early);
    let manager = fixture.manager(Arc::new(ScriptedSource::serving(ARTIFACT.to_vec())));

    runtime()
        .block_on(manager.install_manifest(
            &manifest,
            &confirming(&manifest),
            &CancellationToken::new(),
            &move |progress| {
                // Progress is reported while bytes are still arriving,
                // so the destination must not exist yet unless the
                // final call is the one running.
                if progress.received_bytes < progress.total_bytes
                    && Path::new(&destination).exists()
                {
                    watcher.fetch_add(1, Ordering::SeqCst);
                }
            },
        ))
        .unwrap();

    assert_eq!(
        seen_early.load(Ordering::SeqCst),
        0,
        "the destination name existed while the artifact was still arriving"
    );
}
