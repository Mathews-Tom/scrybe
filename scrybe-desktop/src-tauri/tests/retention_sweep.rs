// Copyright 2026 Mathews Tom
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//     https://www.apache.org/licenses/LICENSE-2.0

//! The sweep as the host reaches it.
//!
//! The service's own tests cover which folders a sweep removes. These
//! cover the wiring the host adds: that the sweep resolves its services
//! from the handle setup registers them on, that it reads the window
//! from the configuration a reader actually wrote, and that a handle
//! with no services is not an error.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use chrono::{DateTime, TimeZone as _, Utc};
use scrybe_application::sessions::{Destination, RetentionService, TRASH_DIR};
use scrybe_application::{SessionRef, StorageRoot};
use scrybe_desktop::state::Desktop;
use tauri::Manager as _;

fn mock_app() -> tauri::App<tauri::test::MockRuntime> {
    tauri::test::mock_builder()
        .build(tauri::test::mock_context(tauri::test::noop_assets()))
        .unwrap()
}

fn at(day: u32) -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 5, day, 12, 0, 0).unwrap()
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

#[test]
fn test_the_host_sweeps_through_the_services_setup_registered() {
    let directory = tempfile::tempdir().unwrap();
    session(directory.path(), "2026-04-29-1430-standup");

    // Trashed eleven days before the sweep, against its persisted
    // seven-day deadline.
    RetentionService::new(StorageRoot::new(directory.path()))
        .retain(
            &SessionRef::parse("2026-04-29-1430-standup").unwrap(),
            Destination::Trash,
            at(1),
            7,
        )
        .unwrap();

    let app = mock_app();
    app.manage(Desktop::new(
        StorageRoot::new(directory.path()),
        directory.path().join("config.toml"),
    ));

    let outcome = scrybe_desktop::retention::sweep(app.handle(), at(12))
        .expect("services are registered, so a sweep must be attempted")
        .expect("the sweep must succeed");

    assert_eq!(outcome.removed, vec!["2026-04-29-1430-standup".to_string()]);
    assert!(!directory
        .path()
        .join(TRASH_DIR)
        .join("2026-04-29-1430-standup")
        .exists());
}

/// A later configuration edit must not shorten the deadline a reader
/// confirmed when deleting the session.
#[test]
fn test_the_host_honors_the_deadline_persisted_with_each_entry() {
    let directory = tempfile::tempdir().unwrap();
    let config = directory.path().join("config.toml");
    std::fs::write(
        &config,
        format!(
            "schema_version = 1\n\n\
             [storage]\n\
             root = \"{}\"\n\
             trash_retention_days = 1\n",
            directory.path().display()
        ),
    )
    .unwrap();
    session(directory.path(), "2026-04-29-1430-standup");
    RetentionService::new(StorageRoot::new(directory.path()))
        .retain(
            &SessionRef::parse("2026-04-29-1430-standup").unwrap(),
            Destination::Trash,
            at(1),
            30,
        )
        .unwrap();

    let app = mock_app();
    app.manage(Desktop::new(StorageRoot::new(directory.path()), config));

    // Eleven days in: past the newly configured one-day window, still
    // inside the thirty days confirmed for this entry.
    let outcome = scrybe_desktop::retention::sweep(app.handle(), at(12))
        .expect("services are registered")
        .expect("the sweep must succeed");

    assert!(
        outcome.removed.is_empty(),
        "a persisted thirty-day deadline must keep an eleven-day-old session: {outcome:?}"
    );
    assert!(directory
        .path()
        .join(TRASH_DIR)
        .join("2026-04-29-1430-standup")
        .is_dir());
}

#[test]
fn test_a_handle_with_no_services_is_not_a_failure() {
    let app = mock_app();
    assert!(
        scrybe_desktop::retention::sweep(app.handle(), at(12)).is_none(),
        "no services means no storage root to sweep, which is not an error"
    );
}
