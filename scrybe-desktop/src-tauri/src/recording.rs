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
//!
//! The commands below are generic over `R: tauri::Runtime` rather than
//! fixed to the concrete `Wry` runtime `tauri::AppHandle` defaults to.
//! Tauri still dispatches the real, concrete instantiation at the IPC
//! boundary; the generic parameter is what lets
//! `scrybe-desktop/src-tauri/tests/recording_stop.rs` drive them
//! directly against `tauri::test::MockRuntime`, which a build against a
//! fixed `Wry` could not do without a real `WebView`.

// Tauri resolves a command's handle by value, so the two commands below
// take one that way whether or not they consume it.
#![allow(clippy::needless_pass_by_value)]

#[cfg(all(target_os = "macos", feature = "system-capture-mac"))]
use futures::stream;
use std::sync::{Arc, Mutex, PoisonError};

#[cfg(all(target_os = "macos", feature = "system-capture-mac"))]
use scrybe_application::recording::SystemBackend;
use scrybe_application::recording::{
    CaptureCapability, CaptureFrames, CaptureRegistry, CaptureSource, CaptureSupport,
    RecordingOverrides, RecordingPlan, RecordingProgress, RecordingRun, SettledConsent, Stop,
    StopAcceptance, StopSource,
};
use scrybe_application::{ApplicationError, ErrorCode};
#[cfg(all(target_os = "macos", feature = "system-capture-mac"))]
use scrybe_capture_mac::{input_devices, InputDevice, MacCapture, NativeMicCapture, SckCapture};
#[cfg(all(target_os = "macos", feature = "system-capture-mac"))]
use scrybe_core::capture::AudioCapture;
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

/// The summary a transition event carries when the recording fails
/// after capture had already opened.
///
/// Distinct from [`scrybe_application::recording::PREFLIGHT_FAILURE_SUMMARY`]
/// on purpose: that one is true only while nothing has been written yet,
/// and a failure here may follow minutes of captured audio. Mirrors the
/// command-line recorder's own `RECORDING_FAILURE_SUMMARY`
/// (`scrybe-cli/src/commands/rec.rs`), which draws the same line rather
/// than inventing a third form of words for the same fact.
const RECORDING_FAILURE_SUMMARY: &str = "recording session failed";
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
/// clear a source [`open`] then refuses.
///
/// Each is read from a feature rather than asserted. `transcription_model`
/// was once a hardcoded `false`, which was true while this crate
/// depended on no runtime — and became a lie the moment it did, because
/// a preflight that refuses a model the build can load is as wrong as
/// one that clears a model it cannot. A reader with the model installed
/// and every permission granted was told the build carried no runtime.
#[must_use]
pub const fn support() -> CaptureSupport {
    CaptureSupport {
        capture: if cfg!(all(target_os = "macos", feature = "system-capture-mac")) {
            CaptureCapability::MicrophoneAndSystemAudio
        } else if cfg!(feature = "mic-capture") {
            CaptureCapability::Microphone
        } else {
            CaptureCapability::SyntheticOnly
        },
        transcription_model: cfg!(feature = "whisper-local"),
        notes_provider: cfg!(feature = "notes-generation"),
    }
}

#[cfg(all(target_os = "macos", feature = "system-capture-mac"))]
fn input_device(plan: &RecordingPlan) -> Result<InputDevice, ApplicationError> {
    let devices = input_devices().map_err(|source| {
        ApplicationError::new(
            ErrorCode::CaptureUnavailable,
            "macOS Core Audio input devices could not be enumerated",
        )
        .with_source(source)
    })?;
    if let Some(uid) = plan.input_device.as_deref() {
        return devices
            .into_iter()
            .find(|device| device.uid == uid)
            .ok_or_else(|| {
                ApplicationError::new(
                    ErrorCode::CaptureUnavailable,
                    format!("configured Core Audio input device `{uid}` was not found"),
                )
            });
    }
    devices
        .into_iter()
        .find(|device| device.is_default)
        .ok_or_else(|| {
            ApplicationError::new(
                ErrorCode::CaptureUnavailable,
                "macOS has no default Core Audio input device",
            )
        })
}

#[cfg(all(target_os = "macos", feature = "system-capture-mac"))]
fn start_capture<T>(
    registry: &CaptureRegistry,
    capture: T,
    unavailable: String,
) -> Result<CaptureFrames, ApplicationError>
where
    T: AudioCapture,
{
    let capture = registry.register(capture);
    let mut capture = capture.lock().map_err(|_| {
        ApplicationError::new(
            ErrorCode::CaptureUnavailable,
            "the capture registry's adapter lock was poisoned",
        )
    })?;
    if let Err(source) = capture.start() {
        let error =
            ApplicationError::new(ErrorCode::CaptureUnavailable, unavailable).with_source(source);
        drop(capture);
        let _ = registry.stop_all();
        return Err(error);
    }
    Ok(Box::pin(capture.frames()))
}

#[cfg(all(target_os = "macos", feature = "system-capture-mac"))]
fn macos_microphone_frames(
    plan: &RecordingPlan,
    registry: &CaptureRegistry,
) -> Result<CaptureFrames, ApplicationError> {
    let device = input_device(plan)?;
    let unavailable = format!(
        "the selected input device {} ({}) could not be opened; grant Microphone permission in System Settings if prompted",
        device.name, device.uid
    );
    start_capture(
        registry,
        NativeMicCapture::new(device.uid, plan.aec),
        unavailable,
    )
}

#[cfg(all(target_os = "macos", feature = "system-capture-mac"))]
fn macos_system_frames(
    plan: &RecordingPlan,
    registry: &CaptureRegistry,
) -> Result<CaptureFrames, ApplicationError> {
    let frames = match plan.system_backend {
        SystemBackend::Sck => start_capture(
            registry,
            SckCapture::new(),
            "ScreenCaptureKit system audio could not be opened; grant Screen & System Audio Recording permission in System Settings if prompted".to_string(),
        )?,
        SystemBackend::Tap => start_capture(
            registry,
            MacCapture::new(),
            "Core Audio Tap system audio could not be opened; grant System Audio Recording Only permission in System Settings if prompted".to_string(),
        )?,
    };
    let microphone = match macos_microphone_frames(plan, registry) {
        Ok(frames) => frames,
        Err(error) => {
            let _ = registry.stop_all();
            return Err(error);
        }
    };
    Ok(Box::pin(stream::select(microphone, frames)))
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
        #[cfg(all(target_os = "macos", feature = "system-capture-mac"))]
        CaptureSource::Mic => macos_microphone_frames(plan, registry),
        #[cfg(all(target_os = "macos", feature = "system-capture-mac"))]
        CaptureSource::MicSystem => macos_system_frames(plan, registry),
        #[cfg(all(
            feature = "mic-capture",
            not(all(target_os = "macos", feature = "system-capture-mac"))
        ))]
        CaptureSource::Mic => scrybe_application::recording::microphone_frames(registry),
        // Unreachable through `start`: preflight refuses a source this
        // build cannot open before anything gets here. Stated rather
        // than unwrapped, so a future build that widens `support`
        // without widening this gets a refusal instead of a panic.
        #[cfg(not(all(target_os = "macos", feature = "system-capture-mac")))]
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

/// Resolves, checks, and starts one recording, opening capture through
/// `open` rather than always the real one.
///
/// Extracted from [`start`] — the shared entry every surface reaches —
/// so a test can hold capture open behind a controllable gate instead of
/// racing a real device or a real permission prompt. Production always
/// passes [`open`] itself; `scrybe-desktop/src-tauri/tests/recording_stop.rs`
/// passes one that blocks on a channel, which is what makes the
/// `Preparing` window below reproducible rather than timing-dependent.
///
/// # Errors
///
/// The preflight refusal, a state conflict when a recording is already
/// in flight, or a capture device that could not be opened.
pub fn start_recording_with<R, F>(
    app: &tauri::AppHandle<R>,
    title: Option<String>,
    open: F,
) -> Result<RecordingStatus, CommandFailure>
where
    R: tauri::Runtime,
    F: FnOnce(&RecordingPlan, &CaptureRegistry) -> Result<CaptureFrames, ApplicationError>,
{
    let desktop = app.state::<Desktop>();
    let live = app.state::<Arc<LiveRecording>>();
    let controller = Arc::clone(desktop.application().recording());
    // A reader who never explicitly dismissed the last recording's
    // outcome — no surface called `acknowledge_recording`, or none had
    // rendered it yet — must not have that block this one.
    // `begin_preparing` only accepts `Idle`, so clearing a leftover
    // terminal state here is what keeps a new attempt reachable rather
    // than refused with a conflict the reader has no way to resolve.
    acknowledge_if_terminal(&controller);

    let plan = scrybe_application::recording::begin(
        &controller,
        desktop.application().config(),
        crate::state::home_directory().as_deref(),
        support(),
        None,
        &RecordingOverrides {
            title,
            // The root this host already reads through, rather than
            // whatever the configuration file happens to name. They are
            // the same value for a real installation, because that root
            // was built from that configuration — but a caller that
            // constructs a `Desktop` over one root and no configuration
            // file would otherwise write somewhere else entirely, which
            // is how a test suite came to leave recordings in a
            // reader's own storage root on every run.
            root: Some(desktop.application().sessions().root().path().to_path_buf()),
            ..RecordingOverrides::default()
        },
    )
    .map_err(|refusal| refusal.error)?;

    // Armed before capture opens, not after. `open` is where macOS
    // raises its own permission prompt — a wait bounded only by the
    // reader answering a dialog — and arming afterward left a stop
    // requested in that window with nothing to reach: `request_stop`
    // found no armed `Stop` and answered `NotRecording`, a false
    // answer that dropped the request for the whole window. Arming
    // first means `request_stop` reaches this same `Stop` — the one
    // route every surface uses — whether or not capture exists yet;
    // the controller still decides under its own lock whether the stop
    // counts, and if it does, the signal it flips here is the one
    // `watch.wait()` below observes the moment the stream starts.
    let (stop, watch) = Stop::new();
    live.arm(stop);

    let registry = CaptureRegistry::default();
    let frames = match open(&plan, &registry) {
        Ok(frames) => frames,
        Err(error) => {
            // The controller is `Preparing` and nothing is on disk;
            // settle it so the next attempt can start.
            live.disarm();
            settle_failed(
                &controller,
                scrybe_application::recording::PREFLIGHT_FAILURE_SUMMARY,
            );
            return Err(error.into());
        }
    };

    let config = desktop.application().config().load().map_err(|error| {
        live.disarm();
        settle_failed(
            &controller,
            scrybe_application::recording::PREFLIGHT_FAILURE_SUMMARY,
        );
        CommandFailure::from(error)
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
                settle_failed(&session_controller, RECORDING_FAILURE_SUMMARY);
            }
        }
    });

    Ok(status)
}

/// Resolves, checks, and starts one recording, for any surface.
///
/// The command below is one caller; the tray item and the global hotkey
/// are the others. They share this rather than each assembling a start,
/// so a recording begun from the menu bar is the same recording begun
/// from the window.
///
/// # Errors
///
/// The preflight refusal, a state conflict when a recording is already
/// in flight, or a capture device that could not be opened.
pub fn start<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    title: Option<String>,
) -> Result<RecordingStatus, CommandFailure> {
    start_recording_with(app, title, open)
}

/// Starts one recording from this window's own command.
///
/// Thin: [`start`] is the shared entry every surface reaches, so a
/// recording begun here is the same recording the tray item and the
/// global hotkey would begin.
///
/// # Errors
///
/// Whatever [`start`] refuses with.
#[tauri::command(async)]
pub fn start_recording<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    title: Option<String>,
) -> Result<RecordingStatus, CommandFailure> {
    start(&app, title)
}

/// Asks the recording in flight to stop and save.
///
/// Idempotent by construction: the controller decides under one lock
/// whether this request is the one that counts, and capture is torn
/// down only if it said yes.
#[must_use]
#[tauri::command]
pub fn stop_recording<R: tauri::Runtime>(app: tauri::AppHandle<R>) -> RecordingStatus {
    let _ = request_stop(&app, StopSource::Window);
    app.state::<Desktop>()
        .application()
        .recording()
        .snapshot()
        .into()
}

/// Acknowledges a terminal recording once a surface has rendered its
/// outcome, returning the controller to idle.
///
/// `settle_completed` and `settle_failed` deliberately no longer do
/// this themselves: calling it in the same breath as `complete` or
/// `fail` was what let the controller settle back to `Idle` before any
/// surface's re-query of `recording_status` could observe the terminal
/// state a transition event had just announced — a completion the
/// reader was never shown, and a failure shown beside a `Ready` label
/// that had already moved on. A surface calls this once it has
/// rendered the outcome; [`start_recording_with`] also calls it before
/// beginning a new attempt, so a reader who never explicitly
/// acknowledges cannot wedge the next recording behind one nobody
/// dismissed.
///
/// Idempotent: called when nothing is terminal, it changes nothing
/// rather than reporting a conflict a caller has no use for.
#[must_use]
#[tauri::command]
pub fn acknowledge_recording<R: tauri::Runtime>(app: tauri::AppHandle<R>) -> RecordingStatus {
    let desktop = app.state::<Desktop>();
    let controller = desktop.application().recording();
    acknowledge_if_terminal(controller);
    controller.snapshot().into()
}

/// Returns `controller` to idle if it is sitting in a terminal state;
/// changes nothing otherwise.
fn acknowledge_if_terminal(controller: &scrybe_application::recording::RecordingController) {
    if controller.snapshot().state.is_terminal() {
        let _ = controller.acknowledge();
    }
}

/// Requests a stop from `source`, through the one route every surface
/// uses.
///
/// Returns what the controller decided, so a caller that has to act on
/// the answer — a quit handler deciding whether to wait — can.
#[must_use]
pub fn request_stop<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    source: StopSource,
) -> StopAcceptance {
    let Some(stop) = app.state::<Arc<LiveRecording>>().current() else {
        return StopAcceptance::NotRecording;
    };
    stop.request(app.state::<Desktop>().application().recording(), source)
}

/// Settles a completed recording, leaving it observable in
/// [`scrybe_application::recording::RecordingState::Completed`] until a
/// surface calls [`acknowledge_recording`].
fn settle_completed(controller: &scrybe_application::recording::RecordingController) {
    if let Err(conflict) = controller.complete() {
        eprintln!("scrybe-desktop: a completed recording could not settle: {conflict}");
    }
}

/// Settles a failed recording with `summary`, leaving it observable in
/// [`scrybe_application::recording::RecordingState::Failed`] until a
/// surface calls [`acknowledge_recording`].
///
/// The summary is fixed by each call site rather than derived here. A
/// `RecordingFailure` is a `Serialize` field of every transition event,
/// and the detailed error carries paths and device identities through
/// its source chain; it goes to stderr, where a maintainer reads it,
/// and not into an event a window renders.
fn settle_failed(controller: &scrybe_application::recording::RecordingController, summary: &str) {
    let _ = controller.fail(summary);
}

/// Who the session is attributed to in `meta.toml`.
fn whoami() -> String {
    std::env::var("USER")
        .ok()
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| "scrybe-user".to_string())
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use scrybe_application::recording::{RecordingFailureKind, RecordingState};
    use scrybe_application::{ScrybeApplication, StorageRoot};

    use super::*;

    fn recording_application(directory: &std::path::Path) -> ScrybeApplication {
        ScrybeApplication::new(StorageRoot::new(directory), directory.join("config.toml"))
    }
    /// The shipped macOS configuration defaults to `mic+system`. The
    /// application must advertise the same source it can construct, or
    /// every recording control remains disabled before macOS can ask
    /// for either permission.
    #[cfg(all(target_os = "macos", feature = "system-capture-mac"))]
    #[test]
    fn test_shipped_macos_build_accepts_mic_and_system_audio_preflight() {
        let directory = tempfile::tempdir().unwrap();
        let plan = RecordingPlan {
            root: directory.path().join("sessions"),
            title: None,
            source: CaptureSource::MicSystem,
            system_backend: SystemBackend::Sck,
            input_device: None,
            transcription: scrybe_application::recording::TranscriptionModel::Stub,
            notes: scrybe_application::recording::NotesBackend::Stub,
            consent: scrybe_core::types::ConsentMode::Quick,
            aec: false,
        };

        let report = scrybe_application::recording::preflight(&plan, support(), None);

        assert!(report.can_record(), "{:?}", report.blocking());
    }

    /// Before capture opens, a failure is a preflight failure — nothing
    /// was written — and must carry the wording that says so.
    #[test]
    fn test_settle_failed_before_capture_opened_writes_the_preflight_summary() {
        let directory = tempfile::tempdir().unwrap();
        let application = recording_application(directory.path());
        let controller = application.recording();
        controller.begin_preparing().unwrap();

        settle_failed(
            controller,
            scrybe_application::recording::PREFLIGHT_FAILURE_SUMMARY,
        );

        let snapshot = controller.snapshot();
        assert_eq!(snapshot.state, RecordingState::Failed);
        let failure = snapshot.failure.unwrap();
        assert_eq!(failure.kind, RecordingFailureKind::Preflight);
        assert_eq!(
            failure.summary,
            scrybe_application::recording::PREFLIGHT_FAILURE_SUMMARY
        );
    }

    /// Once capture has begun, a failure is a failure of the recording
    /// itself, not of starting one — audio may already exist — and must
    /// not claim the recording never started.
    #[test]
    fn test_settle_failed_after_capture_began_writes_the_recording_failed_summary() {
        let directory = tempfile::tempdir().unwrap();
        let application = recording_application(directory.path());
        let controller = application.recording();
        controller.begin_preparing().unwrap();
        controller.mark_recording().unwrap();

        settle_failed(controller, RECORDING_FAILURE_SUMMARY);

        let snapshot = controller.snapshot();
        assert_eq!(snapshot.state, RecordingState::Failed);
        let failure = snapshot.failure.unwrap();
        assert_eq!(failure.kind, RecordingFailureKind::Capture);
        assert_eq!(failure.summary, RECORDING_FAILURE_SUMMARY);
    }

    /// A failure during finalization gets the same recording-failed
    /// wording as one during capture: both are after capture opened,
    /// and neither is a claim that nothing started.
    #[test]
    fn test_settle_failed_during_finalization_writes_the_recording_failed_summary() {
        let directory = tempfile::tempdir().unwrap();
        let application = recording_application(directory.path());
        let controller = application.recording();
        controller.begin_preparing().unwrap();
        controller.mark_recording().unwrap();
        controller.begin_saving().unwrap();

        settle_failed(controller, RECORDING_FAILURE_SUMMARY);

        let snapshot = controller.snapshot();
        let failure = snapshot.failure.unwrap();
        assert_eq!(failure.kind, RecordingFailureKind::Finalization);
        assert_eq!(failure.summary, RECORDING_FAILURE_SUMMARY);
    }

    /// The terminal state must stay observable until something
    /// explicitly acknowledges it — settling must not self-acknowledge.
    #[test]
    fn test_settle_completed_leaves_the_terminal_state_observable() {
        let directory = tempfile::tempdir().unwrap();
        let application = recording_application(directory.path());
        let controller = application.recording();
        controller.begin_preparing().unwrap();
        controller.mark_recording().unwrap();
        controller.begin_saving().unwrap();

        settle_completed(controller);

        assert_eq!(controller.snapshot().state, RecordingState::Completed);
    }

    /// As above, for a failure: nothing here may return the controller
    /// to idle on its own.
    #[test]
    fn test_settle_failed_leaves_the_terminal_state_observable() {
        let directory = tempfile::tempdir().unwrap();
        let application = recording_application(directory.path());
        let controller = application.recording();
        controller.begin_preparing().unwrap();

        settle_failed(
            controller,
            scrybe_application::recording::PREFLIGHT_FAILURE_SUMMARY,
        );

        assert_eq!(controller.snapshot().state, RecordingState::Failed);
    }

    /// The only thing that may return a terminal controller to idle.
    #[test]
    fn test_acknowledge_if_terminal_returns_a_completed_controller_to_idle() {
        let directory = tempfile::tempdir().unwrap();
        let application = recording_application(directory.path());
        let controller = application.recording();
        controller.begin_preparing().unwrap();
        controller.mark_recording().unwrap();
        controller.begin_saving().unwrap();
        controller.complete().unwrap();

        acknowledge_if_terminal(controller);

        assert_eq!(controller.snapshot().state, RecordingState::Idle);
    }

    /// Idempotent: nothing terminal means nothing changes, rather than
    /// a conflict a caller would have to ignore.
    #[test]
    fn test_acknowledge_if_terminal_does_nothing_outside_a_terminal_state() {
        let directory = tempfile::tempdir().unwrap();
        let application = recording_application(directory.path());
        let controller = application.recording();
        controller.begin_preparing().unwrap();
        controller.mark_recording().unwrap();

        acknowledge_if_terminal(controller);

        assert_eq!(controller.snapshot().state, RecordingState::Recording);
    }
}
