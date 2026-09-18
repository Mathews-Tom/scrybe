// Copyright 2026 Mathews Tom
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//     https://www.apache.org/licenses/LICENSE-2.0

//! Starting and stopping one recording from this window.
//!
//! Thin, deliberately. Resolution, the preflight, the providers, the
//! pipeline, and the state model are all `scrybe-application`'s; what
//! lives here is the translation between the `WebView`'s IPC and them,
//! plus the one thing that is genuinely this host's — which capture
//! adapters this binary linked.
//!
//! The recording itself runs on Tauri's runtime rather than on the
//! thread a command arrived on. A command that awaited a whole session
//! would hold the IPC boundary open for the length of a meeting.

// Tauri resolves a command's handle by value, so the two commands below
// take one that way whether or not they consume it.
#![allow(clippy::needless_pass_by_value)]

use std::sync::{Arc, Mutex, PoisonError};

use scrybe_application::recording::{
    CaptureCapability, CaptureFrames, CaptureRegistry, CaptureSource, CaptureSupport,
    RecordingOverrides, RecordingPlan, RecordingProgress, RecordingRun, SettledConsent, Stop,
    StopAcceptance, StopSource,
};
use scrybe_application::{ApplicationError, ErrorCode};
use tauri::{Emitter, Manager};

use crate::contract::{CommandFailure, RecordingStatus, PROGRESS_EVENT};
use crate::state::Desktop;

/// How long the synthetic source runs before ending on its own.
///
/// Bounded rather than unbounded so a recording nobody stops still
/// finishes.
const SYNTHETIC_SECONDS: u64 = 60;

/// How long one synthetic frame represents.
///
/// The generator produces its frames in-process with no pacing, which
/// is right for a test that wants determinism and wrong for a window: a
/// reader who presses record is shown a recording, and an unpaced
/// source finishes a minute of audio before they can reach the stop
/// control. Real capture arrives at real wall-clock pace, so this
/// source is paced to match. 1,600 samples at 16 kHz is 100 ms.
const SYNTHETIC_FRAME: std::time::Duration = std::time::Duration::from_millis(100);

/// The stop of the recording currently in flight, if there is one.
///
/// Held beside the services rather than inside them: which window's
/// control is bound to this recording is a fact about this host, and
/// the service layer is shared with a command-line tool that has no
/// window. The controller still owns whether a stop is accepted — this
/// is only the route from an accepted stop to the capture holding it
/// open.
#[derive(Default)]
pub struct LiveRecording {
    stop: Mutex<Option<Stop>>,
}

impl LiveRecording {
    fn arm(&self, stop: Stop) {
        *self.stop.lock().unwrap_or_else(PoisonError::into_inner) = Some(stop);
    }

    fn disarm(&self) {
        *self.stop.lock().unwrap_or_else(PoisonError::into_inner) = None;
    }

    fn current(&self) -> Option<Stop> {
        self.stop
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }
}

/// What this host linked.
///
/// Every field is a statement about this binary, written under the same
/// conditions as the capture construction below, so preflight cannot
/// clear a source [`open`] then refuses. The transcription runtime is
/// absent because this crate depends on none: a configured model is
/// refused rather than silently replaced by the stub.
#[must_use]
pub const fn support() -> CaptureSupport {
    CaptureSupport {
        capture: if cfg!(feature = "mic-capture") {
            CaptureCapability::Microphone
        } else {
            CaptureCapability::SyntheticOnly
        },
        transcription_model: false,
        notes_provider: cfg!(feature = "notes-generation"),
    }
}

/// Opens the source `plan` names.
///
/// # Errors
///
/// [`ErrorCode::CaptureUnavailable`] when the device cannot be opened,
/// which on macOS is where the platform raises its own permission
/// prompt — the grant preflight said it had not checked.
fn open(
    plan: &RecordingPlan,
    registry: &CaptureRegistry,
) -> Result<CaptureFrames, ApplicationError> {
    match plan.source {
        CaptureSource::Synthetic => Ok(Box::pin(futures::stream::StreamExt::then(
            scrybe_application::recording::synthetic_frames(SYNTHETIC_SECONDS),
            |frame| async move {
                tokio::time::sleep(SYNTHETIC_FRAME).await;
                frame
            },
        ))),
        #[cfg(feature = "mic-capture")]
        CaptureSource::Mic => scrybe_application::recording::microphone_frames(registry),
        // Unreachable through `start`: preflight refuses a source this
        // build cannot open before anything gets here. Stated rather
        // than unwrapped, so a future build that widens `support`
        // without widening this gets a refusal instead of a panic.
        other => {
            let _ = registry;
            Err(ApplicationError::new(
                ErrorCode::CaptureUnavailable,
                format!(
                    "this build cannot open source {}; nothing here should have cleared it",
                    other.as_str()
                ),
            ))
        }
    }
}

/// Resolves, checks, and starts one recording.
///
/// Returns as soon as the recording is under way — the session runs on
/// the runtime, and the window learns what happened from the transition
/// events it is already subscribed to.
///
/// # Errors
///
/// The preflight refusal, a state conflict when a recording is already
/// in flight, or a capture device that could not be opened.
#[tauri::command(async)]
pub fn start_recording(
    app: tauri::AppHandle,
    title: Option<String>,
) -> Result<RecordingStatus, CommandFailure> {
    start(&app, title).map_err(Into::into)
}

/// Resolves, checks, and starts one recording, for any surface.
///
/// The command above is one caller; the tray item and the global hotkey
/// are the others. They share this rather than each assembling a start,
/// so a recording begun from the menu bar is the same recording begun
/// from the window.
///
/// # Errors
///
/// The preflight refusal, a state conflict when a recording is already
/// in flight, or a capture device that could not be opened.
pub fn start(
    app: &tauri::AppHandle,
    title: Option<String>,
) -> Result<RecordingStatus, ApplicationError> {
    let desktop = app.state::<Desktop>();
    let live = app.state::<Arc<LiveRecording>>();
    let controller = Arc::clone(desktop.application().recording());

    let plan = scrybe_application::recording::begin(
        &controller,
        desktop.application().config(),
        crate::state::home_directory().as_deref(),
        support(),
        None,
        &RecordingOverrides {
            title,
            ..RecordingOverrides::default()
        },
    )
    .map_err(|refusal| refusal.error)?;

    let registry = CaptureRegistry::default();
    let frames = match open(&plan, &registry) {
        Ok(frames) => frames,
        Err(error) => {
            // The controller is `Preparing` and nothing is on disk;
            // settle it so the next attempt can start.
            settle_failed(&controller);
            return Err(error);
        }
    };

    let (stop, watch) = Stop::new();
    live.arm(stop);
    let config = desktop.application().config().load().inspect_err(|_| {
        settle_failed(&controller);
    })?;
    let status = controller.snapshot().into();

    let handle = app.clone();
    let progress_handle = app.clone();
    let session_controller = Arc::clone(&controller);
    tauri::async_runtime::spawn(async move {
        let outcome = scrybe_application::recording::run(
            RecordingRun {
                plan: &plan,
                config: &config,
                id: scrybe_core::types::SessionId::new(),
                started_at: chrono::Utc::now(),
                user: whoami(),
                // A window cannot raise a terminal prompt. The decision
                // is the reader's press of the record control, which
                // has already happened; the attestation the session
                // writes is the same one a terminal's prompt produces.
                prompter: &SettledConsent::new(true),
                controller: Some(Arc::clone(&session_controller)),
                on_progress: Some(Arc::new(move |progress: RecordingProgress| {
                    if let Err(error) = progress_handle.emit(PROGRESS_EVENT, progress) {
                        eprintln!("scrybe-desktop: could not emit recording progress: {error}");
                    }
                })),
                // Deliberately none. The pipeline's own progress stream
                // carries transcript text, and nothing in this window
                // renders it.
                on_session_event: None,
            },
            Box::pin(futures::stream::StreamExt::take_until(frames, watch.wait())),
        )
        .await;

        if let Err(error) = registry.stop_all() {
            eprintln!("scrybe-desktop: stopping capture after the session ended failed: {error}");
        }
        handle.state::<Arc<LiveRecording>>().disarm();
        match outcome {
            Ok(_) => settle_completed(&session_controller),
            Err(error) => {
                eprintln!("scrybe-desktop: the recording session failed: {error}");
                settle_failed(&session_controller);
            }
        }
    });

    Ok(status)
}

/// Asks the recording in flight to stop and save.
///
/// Idempotent by construction: the controller decides under one lock
/// whether this request is the one that counts, and capture is torn
/// down only if it said yes.
#[must_use]
#[tauri::command]
pub fn stop_recording(app: tauri::AppHandle) -> RecordingStatus {
    let _ = request_stop(&app, StopSource::Window);
    app.state::<Desktop>()
        .application()
        .recording()
        .snapshot()
        .into()
}

/// Requests a stop from `source`, through the one route every surface
/// uses.
///
/// Returns what the controller decided, so a caller that has to act on
/// the answer — a quit handler deciding whether to wait — can.
#[must_use]
pub fn request_stop(app: &tauri::AppHandle, source: StopSource) -> StopAcceptance {
    let Some(stop) = app.state::<Arc<LiveRecording>>().current() else {
        return StopAcceptance::NotRecording;
    };
    stop.request(app.state::<Desktop>().application().recording(), source)
}

/// Settles a completed recording back to idle.
fn settle_completed(controller: &scrybe_application::recording::RecordingController) {
    if let Err(conflict) = controller.complete() {
        eprintln!("scrybe-desktop: a completed recording could not settle: {conflict}");
        return;
    }
    if let Err(conflict) = controller.acknowledge() {
        eprintln!("scrybe-desktop: a completed recording could not return to idle: {conflict}");
    }
}

/// Settles a failed recording back to idle.
///
/// The summary is fixed. A `RecordingFailure` is a `Serialize` field of
/// every transition event, and the detailed error carries paths and
/// device identities through its source chain; it goes to stderr, where
/// a maintainer reads it, and not into an event a window renders.
fn settle_failed(controller: &scrybe_application::recording::RecordingController) {
    if controller
        .fail(scrybe_application::recording::PREFLIGHT_FAILURE_SUMMARY)
        .is_err()
    {
        return;
    }
    if let Err(conflict) = controller.acknowledge() {
        eprintln!("scrybe-desktop: a failed recording could not return to idle: {conflict}");
    }
}

/// Who the session is attributed to in `meta.toml`.
fn whoami() -> String {
    std::env::var("USER")
        .ok()
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| "scrybe-user".to_string())
}
