// Copyright 2026 Mathews Tom
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//     https://www.apache.org/licenses/LICENSE-2.0

//! The two retention commands, as a frontend reaches them.
//!
//! The service's tests cover what a move does. These cover what the
//! command adds: resolution before any move, the recording-state
//! refusal at every state that writes a session folder, and an outcome
//! carrying the window a confirmation needs.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use scrybe_application::recording::{RecordingState, StopSource};
use scrybe_application::sessions::{ARCHIVE_DIR, TRASH_DIR};
use scrybe_application::{SessionRef, StorageRoot};
use scrybe_desktop::contract::RetentionDestination;
use scrybe_desktop::state::Desktop;
use tauri::Manager as _;

fn mock_app() -> tauri::App<tauri::test::MockRuntime> {
    tauri::test::mock_builder()
        .build(tauri::test::mock_context(tauri::test::noop_assets()))
        .unwrap()
}

/// A folder that classifies as a complete session.
fn session(parent: &std::path::Path, folder: &str) {
    let path = parent.join(folder);
    std::fs::create_dir_all(&path).unwrap();
    std::fs::write(
        path.join("meta.toml"),
        "session_id = \"01AAA\"\n\
         title = \"a meeting\"\n\
         started_at = \"2026-04-29T14:30:00Z\"\n\
         ended_at = \"2026-04-29T15:00:00Z\"\n\
         duration_secs = 1800\n",
    )
    .unwrap();
    std::fs::write(path.join("audio.opus"), b"").unwrap();
}

/// A mock host managing services over a root holding one session.
///
/// The commands are called through `app.state()` rather than a
/// test-only entry point, so what these tests exercise is the same
/// function Tauri dispatches at the IPC boundary.
fn host_with_a_session(
    directory: &tempfile::TempDir,
    folder: &str,
) -> tauri::App<tauri::test::MockRuntime> {
    session(directory.path(), folder);
    let app = mock_app();
    app.manage(Desktop::new(
        StorageRoot::new(directory.path()),
        directory.path().join("config.toml"),
    ));
    app
}

fn id(text: &str) -> SessionRef {
    SessionRef::parse(text).unwrap()
}

#[test]
fn test_deleting_reports_where_it_went_and_how_long_it_is_kept() {
    let directory = tempfile::tempdir().unwrap();
    let app = host_with_a_session(&directory, "2026-04-29-1430-standup");

    let outcome =
        scrybe_desktop::commands::delete_session(app.state(), id("2026-04-29-1430-standup"), 7)
            .unwrap();

    assert_eq!(outcome.destination, RetentionDestination::Trash);
    assert_eq!(
        outcome.retention_days, 7,
        "the default window, so a confirmation can name it"
    );
    assert!(directory
        .path()
        .join(TRASH_DIR)
        .join("2026-04-29-1430-standup")
        .is_dir());
}

#[test]
fn test_deleting_rejects_a_confirmation_for_a_stale_retention_window() {
    let directory = tempfile::tempdir().unwrap();
    let app = host_with_a_session(&directory, "2026-04-29-1430-standup");
    std::fs::write(
        directory.path().join("config.toml"),
        format!(
            "[storage]\nroot = \"{}\"\ntrash_retention_days = 30\n",
            directory.path().display()
        ),
    )
    .unwrap();

    let refusal =
        scrybe_desktop::commands::delete_session(app.state(), id("2026-04-29-1430-standup"), 7);

    assert!(refusal.is_err());
    assert!(directory.path().join("2026-04-29-1430-standup").is_dir());
    assert!(!directory.path().join(TRASH_DIR).exists());
}

#[test]
fn test_archiving_reports_the_archive() {
    let directory = tempfile::tempdir().unwrap();
    let app = host_with_a_session(&directory, "2026-04-29-1430-standup");

    let outcome =
        scrybe_desktop::commands::archive_session(app.state(), id("2026-04-29-1430-standup"))
            .unwrap();

    assert_eq!(outcome.destination, RetentionDestination::Archive);
    assert!(directory
        .path()
        .join(ARCHIVE_DIR)
        .join("2026-04-29-1430-standup")
        .is_dir());
}

/// Every state that has, or is writing, a session folder must refuse.
///
/// `Saving` is the one that matters most and the one an implementation
/// checking only `Recording` would miss: finalization is the window
/// between a reader's stop and a durable session, and moving the folder
/// during it corrupts what is being written.
#[test]
fn test_no_session_moves_while_a_recording_is_in_flight() {
    for state in [
        RecordingState::Preparing,
        RecordingState::Recording,
        RecordingState::Saving,
    ] {
        let directory = tempfile::tempdir().unwrap();
        let app = host_with_a_session(&directory, "2026-04-29-1430-standup");
        let desktop = app.state::<Desktop>();
        let controller = desktop.application().recording();

        controller.begin_preparing().unwrap();
        if state != RecordingState::Preparing {
            controller.mark_recording().unwrap();
        }
        if state == RecordingState::Saving {
            // `request_stop` enters `Saving` itself; finalization
            // begins from there.
            controller.request_stop(StopSource::Window);
        }
        assert_eq!(
            controller.snapshot().state,
            state,
            "the controller must be in the state under test"
        );

        assert!(
            scrybe_desktop::commands::delete_session(
                app.state(),
                id("2026-04-29-1430-standup"),
                7,
            )
            .is_err(),
            "{state:?} must refuse a delete"
        );
        assert!(
            scrybe_desktop::commands::archive_session(app.state(), id("2026-04-29-1430-standup"))
                .is_err(),
            "{state:?} must refuse an archive"
        );

        assert!(
            directory.path().join("2026-04-29-1430-standup").is_dir(),
            "{state:?}: the session must stay where it was"
        );
        assert!(
            !directory.path().join(TRASH_DIR).exists(),
            "{state:?}: and nothing may be created for it"
        );
    }
}

#[test]
fn test_a_settled_recording_does_not_block_a_move() {
    let directory = tempfile::tempdir().unwrap();
    let app = host_with_a_session(&directory, "2026-04-29-1430-standup");
    let controller = app.state::<Desktop>().application().recording().clone();
    controller.begin_preparing().unwrap();
    controller.mark_recording().unwrap();
    controller.request_stop(StopSource::Window);
    controller.complete().unwrap();

    let outcome =
        scrybe_desktop::commands::delete_session(app.state(), id("2026-04-29-1430-standup"), 7);

    assert!(
        outcome.is_ok(),
        "a completed recording is exactly when a reader deletes one: {outcome:?}"
    );
}

#[test]
fn test_an_identity_no_session_answers_to_moves_nothing() {
    let directory = tempfile::tempdir().unwrap();
    let app = host_with_a_session(&directory, "2026-04-29-1430-standup");

    let refusal =
        scrybe_desktop::commands::delete_session(app.state(), id("2026-04-29-1430-absent"), 7);

    assert!(refusal.is_err());
    assert!(
        !directory.path().join(TRASH_DIR).exists(),
        "resolution comes first, so nothing is created for a name that resolves to nothing"
    );
}
