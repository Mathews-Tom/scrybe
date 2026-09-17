// Copyright 2026 Mathews Tom
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//     https://www.apache.org/licenses/LICENSE-2.0

//! The recording event contract, reached the way a frontend reaches it.
//!
//! This lives outside `src/` deliberately. Crate-private items are
//! invisible here, so it is the only place that can prove the public,
//! versioned event API — `RecordingEvent`,
//! `RECORDING_EVENT_SCHEMA_VERSION`, and `RecordingEventObserver` — is
//! actually reachable by a consumer. `RecordingController::new` and
//! `with_clock` are crate-private, so `observing`, which consumes
//! `Self`, cannot be called from out here at all; without
//! `subscribe` there was no route to an event and the schema would have
//! shipped with no consumer able to subscribe to it.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::sync::{Arc, Mutex};

use pretty_assertions::assert_eq;
use scrybe_application::recording::{
    RecordingEvent, RecordingState, RECORDING_EVENT_SCHEMA_VERSION,
};
use scrybe_application::{ScrybeApplication, StorageRoot};

#[test]
fn test_a_consumer_can_subscribe_to_the_composed_application_controller() {
    let dir = tempfile::tempdir().unwrap();
    let app = ScrybeApplication::new(StorageRoot::new(dir.path()), dir.path().join("config.toml"));
    let seen: Arc<Mutex<Vec<RecordingEvent>>> = Arc::new(Mutex::new(Vec::new()));

    let recorded = Arc::clone(&seen);
    app.recording()
        .subscribe(Arc::new(move |event: &RecordingEvent| {
            recorded.lock().unwrap().push(event.clone());
        }));
    app.recording().begin_preparing().unwrap();

    let events = seen.lock().unwrap();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].sequence, 1);
    assert_eq!(events[0].schema_version, RECORDING_EVENT_SCHEMA_VERSION);
    assert_eq!(events[0].from, RecordingState::Idle);
    assert_eq!(events[0].to, RecordingState::Preparing);
}
