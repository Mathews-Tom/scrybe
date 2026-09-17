// Copyright 2026 Mathews Tom
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//     https://www.apache.org/licenses/LICENSE-2.0

//! What happens when several surfaces stop one recording at once.
//!
//! Sequential stops prove almost nothing here: the interesting claim is
//! about simultaneity, so these tests release every surface at the same
//! instant through a barrier and assert on what the controller did.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Barrier, Mutex};

use scrybe_application::recording::{
    RecordingState, Stop, StopAcceptance, StopSource, RECORDING_EVENT_SCHEMA_VERSION,
};
use scrybe_application::{ScrybeApplication, StorageRoot};

/// Every surface a reader can stop a recording from.
const SURFACES: [StopSource; 6] = [
    StopSource::Window,
    StopSource::Tray,
    StopSource::FloatingWindow,
    StopSource::Hotkey,
    StopSource::Signal,
    StopSource::Termination,
];

fn recording_application(directory: &std::path::Path) -> ScrybeApplication {
    let application =
        ScrybeApplication::new(StorageRoot::new(directory), directory.join("config.toml"));
    application.recording().begin_preparing().unwrap();
    application.recording().mark_recording().unwrap();
    application
}

/// Six threads, one barrier, one recording. Exactly one surface may be
/// told its stop was the one that counted, exactly one
/// `Recording -> Saving` transition may be emitted, and the snapshot
/// must name the surface that won.
#[test]
fn test_six_surfaces_stopping_at_once_produce_one_transition_and_one_acceptance() {
    for attempt in 0..200 {
        let directory = tempfile::tempdir().unwrap();
        let application = recording_application(directory.path());
        let controller = Arc::clone(application.recording());
        let transitions = Arc::new(Mutex::new(Vec::new()));
        let recorded = Arc::clone(&transitions);
        controller.subscribe(Arc::new(move |event| {
            recorded.lock().unwrap().push((event.from, event.to));
        }));
        let (stop, _watch) = Stop::new();
        let accepted = Arc::new(AtomicUsize::new(0));
        let gate = Arc::new(Barrier::new(SURFACES.len()));

        let threads: Vec<_> = SURFACES
            .iter()
            .map(|source| {
                let controller = Arc::clone(&controller);
                let stop = stop.clone();
                let accepted = Arc::clone(&accepted);
                let gate = Arc::clone(&gate);
                let source = *source;
                std::thread::spawn(move || {
                    gate.wait();
                    if stop.request(&controller, source) == StopAcceptance::Accepted {
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
            "attempt {attempt}: exactly one surface may be told its stop counted"
        );
        let observed = transitions.lock().unwrap().clone();
        assert_eq!(
            observed,
            vec![(RecordingState::Recording, RecordingState::Saving)],
            "attempt {attempt}: exactly one transition into saving"
        );
        let snapshot = controller.snapshot();
        assert!(snapshot.stop_requested);
        assert!(
            SURFACES.contains(&snapshot.stop_source.unwrap()),
            "attempt {attempt}: the accepted source must be one that asked"
        );
        assert!(
            stop.accepted(),
            "attempt {attempt}: capture must be torn down"
        );
    }
}

/// The accepted source is fixed at acceptance and never overwritten by
/// a later surface. A reader told "stopped from the tray" must not see
/// it become "stopped from the hotkey" a moment later.
#[test]
fn test_the_surface_that_won_is_the_one_the_snapshot_keeps() {
    for _ in 0..200 {
        let directory = tempfile::tempdir().unwrap();
        let application = recording_application(directory.path());
        let controller = Arc::clone(application.recording());
        let (stop, _watch) = Stop::new();
        let winner = Arc::new(Mutex::new(None));
        let gate = Arc::new(Barrier::new(SURFACES.len()));

        let threads: Vec<_> = SURFACES
            .iter()
            .map(|source| {
                let controller = Arc::clone(&controller);
                let stop = stop.clone();
                let winner = Arc::clone(&winner);
                let gate = Arc::clone(&gate);
                let source = *source;
                std::thread::spawn(move || {
                    gate.wait();
                    if stop.request(&controller, source) == StopAcceptance::Accepted {
                        *winner.lock().unwrap() = Some(source);
                    }
                })
            })
            .collect();
        for thread in threads {
            thread.join().unwrap();
        }

        assert_eq!(controller.snapshot().stop_source, *winner.lock().unwrap());
    }
}

/// Finalization must run once. Six surfaces stopping at once may not
/// produce six entries into saving, and the sequence numbers must have
/// no gap — a surface that sees one knows it missed an event.
#[test]
fn test_concurrent_stops_finalize_once_and_leave_no_gap_in_the_sequence() {
    let directory = tempfile::tempdir().unwrap();
    let application = recording_application(directory.path());
    let controller = Arc::clone(application.recording());
    let events = Arc::new(Mutex::new(Vec::new()));
    let recorded = Arc::clone(&events);
    controller.subscribe(Arc::new(move |event| {
        recorded
            .lock()
            .unwrap()
            .push((event.schema_version, event.sequence, event.to));
    }));
    let (stop, _watch) = Stop::new();
    let gate = Arc::new(Barrier::new(SURFACES.len()));

    let threads: Vec<_> = SURFACES
        .iter()
        .map(|source| {
            let controller = Arc::clone(&controller);
            let stop = stop.clone();
            let gate = Arc::clone(&gate);
            let source = *source;
            std::thread::spawn(move || {
                gate.wait();
                stop.request(&controller, source);
            })
        })
        .collect();
    for thread in threads {
        thread.join().unwrap();
    }
    // The one finalization the accepted stop is responsible for.
    controller.complete().unwrap();

    let observed = events.lock().unwrap().clone();
    assert_eq!(
        observed
            .iter()
            .filter(|(_, _, to)| *to == RecordingState::Saving)
            .count(),
        1
    );
    assert_eq!(
        observed
            .iter()
            .filter(|(_, _, to)| *to == RecordingState::Completed)
            .count(),
        1
    );
    let sequences: Vec<u64> = observed.iter().map(|(_, sequence, _)| *sequence).collect();
    assert_eq!(sequences, vec![3, 4], "{sequences:?}");
    assert!(observed
        .iter()
        .all(|(version, _, _)| *version == RECORDING_EVENT_SCHEMA_VERSION));
}

/// A stop from a surface that arrives after finalization has begun is
/// told so, rather than being silently dropped or starting a second
/// teardown.
#[test]
fn test_a_stop_arriving_after_saving_has_begun_is_told_it_is_too_late() {
    let directory = tempfile::tempdir().unwrap();
    let application = recording_application(directory.path());
    let controller = application.recording();
    let (stop, _watch) = Stop::new();

    assert_eq!(
        stop.request(controller, StopSource::Window),
        StopAcceptance::Accepted
    );

    for source in SURFACES {
        assert_eq!(
            stop.request(controller, source),
            StopAcceptance::AlreadyStopping,
            "{source:?} arrived after saving began"
        );
    }
}

/// Capture is held open until a stop is accepted, and released the
/// moment one is.
#[tokio::test]
async fn test_capture_is_released_the_moment_a_stop_is_accepted() {
    let directory = tempfile::tempdir().unwrap();
    let application = recording_application(directory.path());
    let controller = Arc::clone(application.recording());
    let (stop, watch) = Stop::new();
    let held = tokio::spawn(watch.wait());

    assert!(!held.is_finished());
    stop.request(&controller, StopSource::Hotkey);

    tokio::time::timeout(std::time::Duration::from_secs(5), held)
        .await
        .expect("an accepted stop must release capture")
        .unwrap();
}

/// A stop nobody is listening for is not an error: capture that has
/// already finished on its own leaves no receiver, and the recording is
/// over either way.
#[tokio::test]
async fn test_a_stop_with_no_capture_left_to_release_still_reports_what_it_decided() {
    let directory = tempfile::tempdir().unwrap();
    let application = recording_application(directory.path());
    let (stop, watch) = Stop::new();
    drop(watch);

    assert_eq!(
        stop.request(application.recording(), StopSource::Termination),
        StopAcceptance::Accepted
    );
}
