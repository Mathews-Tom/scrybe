// Copyright 2026 Mathews Tom
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//     https://www.apache.org/licenses/LICENSE-2.0

//! What survives each way a recording can fail.
//!
//! One fixture per boundary the failure contract names — preflight,
//! capture, and finalization — driven through the real orchestration
//! rather than by calling `RecordingController::fail` directly. The
//! point of each is not that an error came back; it is what is on disk
//! afterwards, and whether the label a surface renders matches it.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::sync::{Arc, Mutex};

use futures::stream;
use scrybe_application::config::ConfigService;
use scrybe_application::error::ErrorCode;
use scrybe_application::recording::{
    CaptureCapability, CaptureSupport, RecordingFailureKind, RecordingOverrides, RecordingPlan,
    RecordingRun, RecordingState, SettledConsent,
};
use scrybe_application::{ScrybeApplication, StorageRoot};
use scrybe_core::config::Config;
use scrybe_core::error::CaptureError;
use scrybe_core::types::{AudioFrame, FrameSource, SessionId};

/// A build carrying everything, so a fixture fails for the reason it
/// was built to fail for.
const fn everything() -> CaptureSupport {
    CaptureSupport {
        capture: CaptureCapability::MicrophoneAndSystemAudio,
        transcription_model: true,
        notes_provider: true,
    }
}

/// Every session folder directly under `root`.
fn sessions(root: &std::path::Path) -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(root) else {
        return Vec::new();
    };
    let mut names: Vec<String> = entries
        .filter_map(Result::ok)
        .filter(|entry| entry.path().is_dir())
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}

/// The failure kinds a controller recorded, in order.
fn failure_kinds(
    controller: &scrybe_application::recording::RecordingController,
) -> Arc<Mutex<Vec<RecordingFailureKind>>> {
    let kinds = Arc::new(Mutex::new(Vec::new()));
    let recorded = Arc::clone(&kinds);
    controller.subscribe(Arc::new(move |event| {
        if let Some(failure) = &event.failure {
            recorded.lock().unwrap().push(failure.kind);
        }
    }));
    kinds
}

fn plan_over(root: &std::path::Path) -> RecordingPlan {
    RecordingPlan::resolve(
        &Config::default(),
        None,
        &RecordingOverrides {
            root: Some(root.to_path_buf()),
            title: Some("fixture".to_string()),
            ..RecordingOverrides::default()
        },
    )
    .unwrap()
}

/// A preflight failure leaves nothing: no session, and no storage root
/// either, because preflight does not write.
///
/// What would have to break for this to fail: `preflight::storage`
/// creating the root it measures, or `begin` reaching `RecordingRun`
/// on a refusal.
#[test]
fn test_a_preflight_failure_leaves_no_session_and_no_storage_root() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("sessions");
    let config_path = directory.path().join("config.toml");
    std::fs::write(
        &config_path,
        format!(
            "[storage]\nroot = \"{}\"\n[record]\nsource = \"mic\"\n",
            root.display()
        ),
    )
    .unwrap();
    let application = ScrybeApplication::new(StorageRoot::new(&root), &config_path);
    let kinds = failure_kinds(application.recording());

    let refusal = scrybe_application::recording::begin(
        application.recording(),
        &ConfigService::new(&config_path),
        None,
        CaptureSupport {
            capture: CaptureCapability::SyntheticOnly,
            ..everything()
        },
        None,
        &RecordingOverrides::default(),
    )
    .unwrap_err();

    assert_eq!(refusal.error.payload().code, ErrorCode::PreflightFailed);
    assert_eq!(
        *kinds.lock().unwrap(),
        vec![RecordingFailureKind::Preflight]
    );
    assert!(!root.exists());
}

/// A capture source that fails part-way through does **not** abort the
/// session.
///
/// This is what the pipeline actually does, and it took a fixture to
/// establish: the frames already captured are finalised in full —
/// transcript, notes, encoded audio and `meta.toml` all written — and
/// the capture error is surfaced afterwards, as the returned error's
/// source. So a reader whose microphone is unplugged mid-meeting keeps
/// everything that was said before it was.
///
/// What would have to break for this to fail: the session abandoning
/// what it had captured when the source errored, or swallowing the
/// error and reporting success.
#[tokio::test]
async fn test_a_capture_source_that_fails_part_way_still_finalises_what_it_captured() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("sessions");
    let plan = plan_over(&root);

    let outcome = scrybe_application::recording::run(
        RecordingRun {
            plan: &plan,
            config: &Config::default(),
            id: SessionId::new(),
            started_at: chrono::Utc::now(),
            user: "fixture".to_string(),
            prompter: &SettledConsent::new(true),
            controller: None,
            on_progress: None,
            on_session_event: None,
        },
        Box::pin(stream::iter(vec![
            Ok(frame(0)),
            Ok(frame(1)),
            Err(CaptureError::Platform(Box::new(std::io::Error::other(
                "the fixture's capture device went away",
            )))),
        ])),
    )
    .await;

    let error = outcome.expect_err("the capture error must still reach the caller");
    assert_eq!(error.payload().code, ErrorCode::RecordingFailed);
    assert!(
        std::error::Error::source(&error).is_some(),
        "the capture error itself must survive as the source, where a log can read it"
    );

    let folders = sessions(&root);
    assert_eq!(folders.len(), 1, "{folders:?}");
    let folder = root.join(&folders[0]);
    for artifact in ["transcript.md", "notes.md", "meta.toml", "audio.opus"] {
        assert!(
            folder.join(artifact).is_file(),
            "{artifact} must survive a capture source that failed part-way"
        );
    }
}

/// The failure is labelled from where the controller was when it
/// happened, and a capture source that fails part-way is already past
/// the finalisation boundary by then — so it is a `Finalization`
/// failure, whose documented meaning is exactly right: audio exists and
/// the session is repairable.
///
/// `RecordingFailureKind::Capture` is not unreachable, but a failing
/// capture *stream* is not what produces it, because the pipeline
/// tolerates one. It is what a failure between `SessionProgress::
/// Recording` and the first finalisation event is labelled — a live
/// transcription or journal-write failure.
///
/// What would have to break for this to fail: `progress::drive` no
/// longer entering saving at the pipeline's first finalisation event,
/// which would leave this labelled `Capture` and tell a reader their
/// audio might not exist when it does.
#[tokio::test]
async fn test_a_capture_source_failing_part_way_is_labelled_from_where_the_controller_was() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("sessions");
    let application = ScrybeApplication::new(
        StorageRoot::new(&root),
        directory.path().join("config.toml"),
    );
    let controller = Arc::clone(application.recording());
    controller.begin_preparing().unwrap();
    let plan = plan_over(&root);

    let outcome = scrybe_application::recording::run(
        RecordingRun {
            plan: &plan,
            config: &Config::default(),
            id: SessionId::new(),
            started_at: chrono::Utc::now(),
            user: "fixture".to_string(),
            prompter: &SettledConsent::new(true),
            controller: Some(Arc::clone(&controller)),
            on_progress: None,
            on_session_event: None,
        },
        Box::pin(stream::iter(vec![
            Ok(frame(0)),
            Err(CaptureError::Platform(Box::new(std::io::Error::other(
                "the fixture's capture device went away",
            )))),
        ])),
    )
    .await;

    assert!(outcome.is_err());
    assert_eq!(controller.snapshot().state, RecordingState::Saving);
    let failed = controller.fail("recording could not finish").unwrap();
    assert_eq!(
        failed.failure.map(|failure| failure.kind),
        Some(RecordingFailureKind::Finalization)
    );
}

/// The one kind a failing capture stream does not produce, produced.
///
/// A failure while the controller is `Recording` — before any
/// finalisation event — is a `Capture` failure. Driven on the
/// controller rather than through a fixture source, because the
/// pipeline reaches finalisation from any stream that ends, so there is
/// no capture source that leaves it here.
///
/// What would have to break for this to fail: `RecordingController::
/// fail` no longer deriving the kind from the state it is called in,
/// which is the property that keeps the label from disagreeing with
/// where the controller actually was.
#[test]
fn test_a_failure_while_the_controller_is_recording_is_labelled_capture() {
    let directory = tempfile::tempdir().unwrap();
    let application = ScrybeApplication::new(
        StorageRoot::new(directory.path()),
        directory.path().join("config.toml"),
    );
    let controller = application.recording();
    controller.begin_preparing().unwrap();
    controller.mark_recording().unwrap();

    let failed = controller.fail("recording could not finish").unwrap();

    assert_eq!(
        failed.failure.map(|failure| failure.kind),
        Some(RecordingFailureKind::Capture)
    );
}

/// A recording that a consent refusal stops never reaches capture, so
/// it has nothing to recover — and it must not leave a half-built
/// folder that a session scan would later report as repairable.
///
/// What would have to break for this to fail: the session creating its
/// folder before running consent.
#[tokio::test]
async fn test_a_refused_consent_leaves_no_session_to_repair() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("sessions");
    let plan = plan_over(&root);

    let outcome = scrybe_application::recording::run(
        RecordingRun {
            plan: &plan,
            config: &Config::default(),
            id: SessionId::new(),
            started_at: chrono::Utc::now(),
            user: "fixture".to_string(),
            prompter: &SettledConsent::new(false),
            controller: None,
            on_progress: None,
            on_session_event: None,
        },
        Box::pin(stream::iter(vec![Ok(frame(0))])),
    )
    .await;

    assert!(outcome.is_err(), "a declined consent must stop the session");
    assert_eq!(sessions(&root), Vec::<String>::new());
}

/// The successful path, for contrast: a recording that completes leaves
/// one folder holding everything a reader opens.
///
/// What would have to break for this to fail: any artifact the session
/// is supposed to write not being written, which is what makes this the
/// control the failures above are read against.
#[tokio::test]
async fn test_a_completed_recording_leaves_every_artifact_a_reader_opens() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("sessions");
    let plan = plan_over(&root);

    let outputs = scrybe_application::recording::run(
        RecordingRun {
            plan: &plan,
            config: &Config::default(),
            id: SessionId::new(),
            started_at: chrono::Utc::now(),
            user: "fixture".to_string(),
            prompter: &SettledConsent::new(true),
            controller: None,
            on_progress: None,
            on_session_event: None,
        },
        Box::pin(scrybe_application::recording::synthetic_frames(1)),
    )
    .await
    .unwrap();

    assert!(outputs.transcript_path.is_file());
    assert!(outputs.notes_path.is_file());
    assert!(outputs.meta_path.is_file());
    assert_eq!(sessions(&root).len(), 1);
}

/// Saving is four steps, reported in the pipeline's own order, and a
/// reader watching a window close on a long meeting is shown which one
/// is running.
///
/// What would have to break for this to fail: a step being skipped, or
/// the order changing without the projection changing with it.
#[tokio::test]
async fn test_a_completed_recording_reports_its_four_saving_steps_in_order() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("sessions");
    let plan = plan_over(&root);
    let steps = Arc::new(Mutex::new(Vec::new()));
    let recorded = Arc::clone(&steps);

    scrybe_application::recording::run(
        RecordingRun {
            plan: &plan,
            config: &Config::default(),
            id: SessionId::new(),
            started_at: chrono::Utc::now(),
            user: "fixture".to_string(),
            prompter: &SettledConsent::new(true),
            controller: None,
            on_progress: Some(Arc::new(move |progress| {
                recorded.lock().unwrap().push(progress);
            })),
            on_session_event: None,
        },
        Box::pin(scrybe_application::recording::synthetic_frames(1)),
    )
    .await
    .unwrap();

    let observed = steps.lock().unwrap().clone();
    let indices: Vec<u32> = observed.iter().map(|progress| progress.index).collect();
    assert_eq!(indices, vec![1, 2, 3, 4], "{observed:?}");
    assert!(observed.iter().all(|progress| progress.total == 4));
}

fn frame(index: u64) -> AudioFrame {
    AudioFrame {
        samples: Arc::from(vec![0.2_f32; 1_600]),
        channels: 1,
        sample_rate: 16_000,
        timestamp_ns: index * 100_000_000,
        source: FrameSource::Mic,
    }
}
