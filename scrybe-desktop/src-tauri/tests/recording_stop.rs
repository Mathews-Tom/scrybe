// Copyright 2026 Mathews Tom
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//     https://www.apache.org/licenses/LICENSE-2.0

//! A stop arriving while capture is still opening must not be dropped.
//!
//! `scrybe-application/tests/recording_stop.rs` proves the primitive —
//! `Stop::request` racing `RecordingController::request_stop` — by
//! calling it directly, which bypasses the desktop wrapper entirely.
//! That is exactly how the wrapper's own bug shipped: `request_stop`
//! answered `NotRecording` for the whole `Preparing` window, because
//! nothing was armed until `open` returned. This drives the desktop
//! entry points themselves — `start_recording_with` and `request_stop`
//! — against one shared state, with `open` held behind a channel so
//! the window is controllable rather than timing-dependent.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Barrier, Mutex};

use scrybe_application::recording::{RecordingState, StopAcceptance, StopSource};
use scrybe_application::StorageRoot;
use scrybe_desktop::recording::{request_stop, start_recording_with, LiveRecording};
use scrybe_desktop::state::Desktop;
use tauri::Manager;

/// Every surface a reader can stop a recording from, other than the one
/// this build attributes to a surface's own crash.
const SURFACES: [StopSource; 6] = [
    StopSource::Window,
    StopSource::Tray,
    StopSource::FloatingWindow,
    StopSource::Hotkey,
    StopSource::Signal,
    StopSource::Termination,
];

/// How many times the race below is replayed. Each attempt drives six
/// threads through one barrier around a stop accepted (or not) inside
/// a channel-controlled `Preparing` window; a single run proves
/// nothing about a race, and this is cheap because the synthetic
/// source it opens generates its frames in-process with no real-time
/// pacing.
const ATTEMPTS: usize = 50;

fn mock_app() -> tauri::App<tauri::test::MockRuntime> {
    tauri::test::mock_builder()
        .build(tauri::test::mock_context(tauri::test::noop_assets()))
        .unwrap()
}

#[test]
fn test_a_stop_during_preparing_is_never_lost_and_prevents_the_recording_proceeding() {
    for attempt in 0..ATTEMPTS {
        let directory = tempfile::tempdir().unwrap();
        let desktop = Desktop::new(
            StorageRoot::new(directory.path()),
            directory.path().join("config.toml"),
        );
        let app = mock_app();
        app.manage(desktop);
        app.manage(Arc::new(LiveRecording::default()));
        let handle = app.handle().clone();

        let controller = Arc::clone(handle.state::<Desktop>().application().recording());
        let transitions = Arc::new(Mutex::new(Vec::new()));
        let recorded = Arc::clone(&transitions);
        controller.subscribe(Arc::new(move |event| {
            recorded.lock().unwrap().push((event.from, event.to));
        }));

        // Holds `open` inside the `Preparing` window until the test
        // says otherwise, and reports the moment it is entered so the
        // barrier group below is released only once the window is
        // actually open — not merely once `start_recording_with` has
        // been called.
        let (entered_tx, entered_rx) = mpsc::channel::<()>();
        let (release_tx, release_rx) = mpsc::channel::<()>();

        let start_handle = handle.clone();
        let starter = std::thread::spawn(move || {
            start_recording_with(&start_handle, None, move |_plan, _registry| {
                entered_tx.send(()).unwrap();
                release_rx.recv().unwrap();
                Ok(Box::pin(scrybe_application::recording::synthetic_frames(1)))
            })
        });

        entered_rx.recv().unwrap();

        let accepted = Arc::new(AtomicUsize::new(0));
        let gate = Arc::new(Barrier::new(SURFACES.len()));
        let threads: Vec<_> = SURFACES
            .iter()
            .map(|source| {
                let handle = handle.clone();
                let gate = Arc::clone(&gate);
                let accepted = Arc::clone(&accepted);
                let source = *source;
                std::thread::spawn(move || {
                    gate.wait();
                    if request_stop(&handle, source) == StopAcceptance::Accepted {
                        accepted.fetch_add(1, Ordering::SeqCst);
                    }
                })
            })
            .collect();
        for thread in threads {
            thread.join().unwrap();
        }

        assert_eq!(
            accepted.load(Ordering::SeqCst),
            1,
            "attempt {attempt}: exactly one surface may be told its stop during \
             `Preparing` counted; the request must never be silently dropped"
        );

        release_tx.send(()).unwrap();
        starter.join().unwrap().unwrap();

        // The session runs on the runtime `start_recording_with` spawns
        // onto, not on this thread; wait for it to settle rather than
        // assuming it already has.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while controller.snapshot().state != RecordingState::Failed
            && controller.snapshot().state != RecordingState::Completed
            && std::time::Instant::now() < deadline
        {
            std::thread::sleep(std::time::Duration::from_millis(5));
        }

        let observed = transitions.lock().unwrap().clone();
        let into_saving = observed
            .iter()
            .filter(|(_, to)| *to == RecordingState::Saving)
            .count();
        assert_eq!(
            into_saving, 1,
            "attempt {attempt}: exactly one transition into saving; observed {observed:?}"
        );
        let finalized = observed
            .iter()
            .filter(|(_, to)| matches!(to, RecordingState::Completed | RecordingState::Failed))
            .count();
        assert_eq!(
            finalized, 1,
            "attempt {attempt}: exactly one finalization; observed {observed:?}"
        );
        // The accepted stop was requested while the controller was
        // still `Preparing` — before `open` had returned — so
        // `mark_recording` honors it by going straight to `Saving`
        // rather than ever entering `Recording`: an honored stop that
        // still started the recording proper would be the same defect
        // in a different shape.
        let saving_transition = observed
            .iter()
            .find(|(_, to)| *to == RecordingState::Saving)
            .copied();
        assert_eq!(
            saving_transition.map(|(from, _)| from),
            Some(RecordingState::Preparing),
            "attempt {attempt}: the accepted stop must skip `Recording` entirely; \
             observed {observed:?}"
        );
    }
}
