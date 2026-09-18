// Copyright 2026 Mathews Tom
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//     https://www.apache.org/licenses/LICENSE-2.0

//! Main-thread shell driver for `scrybe record --shell`.
//!
//! Native tray, panel, and hotkey objects are main-thread-bound. This
//! module constructs every configured surface before capture, spawns the
//! existing recording task on Tokio, and pumps platform events until its
//! existing finalization path returns. All controls share one monotonic
//! lifecycle and one idempotent stop signal.

use std::sync::mpsc::{self, Receiver};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use scrybe_application::recording::{
    RecordingController, RecordingSnapshot, RecordingState, StopAcceptance, StopSource,
};
use scrybe_core::config::{ShellConfig, ShellIndicator};
use tokio::runtime::Runtime;
use tokio::sync::watch;

#[cfg(target_os = "macos")]
use scrybe_widgets::floating_panel::{prepare_application, reduce_motion_enabled, FloatingPanel};
use scrybe_widgets::hotkey::{HotkeyEvent, HotkeyListener, DEFAULT_STOP_ACCELERATOR};
use scrybe_widgets::{ShellState, ShellView};

use crate::commands::rec::{
    begin_recording, monitor_signals, run_with_stop, Args, RECORDING_FAILURE_SUMMARY,
};
use crate::runtime::{application, config_service};
use crate::tray::{RecordingIndicator, TrayCommand};

/// Pump interval for the polling loop. On macOS this is the duration
/// passed to `CFRunLoopRunInMode`; on other platforms it bounds the
/// `thread::sleep` between event polls.
const POLL_INTERVAL: Duration = Duration::from_millis(20);

/// Projects the shared recording state onto what a native indicator
/// renders.
///
/// A free function rather than a `From` impl: `ShellView` belongs to
/// `scrybe-widgets`, which deliberately depends on no service layer, so
/// neither type is this crate's to write a conversion between.
/// `Preparing` reads as recording, not as saving. The controller sits
/// in it for the whole of the preflight, and these surfaces are already
/// up and being rendered during it, so projecting it as saving would
/// show a `Saving…` tray label, a pinned waveform, and a dimmed pill dot
/// until capture starts. It also accepts a stop, which is what
/// `stop_enabled` reports, and the surfaces' fixtures take
/// `stop_enabled` to imply a live recording.
///
/// Everything after capture — finalizing, completed, settled — reads as
/// saving, because that is what the surfaces show until they are torn
/// down.
const fn shell_view(snapshot: &RecordingSnapshot) -> ShellView {
    ShellView {
        state: match snapshot.state {
            RecordingState::Preparing | RecordingState::Recording => ShellState::Recording,
            _ => ShellState::Saving,
        },
        elapsed: Duration::from_millis(snapshot.elapsed_ms),
        stop_enabled: snapshot.stop_enabled(),
    }
}

/// Bridges the process-wide recording controller to the stop channel
/// the recording task waits on.
///
/// The controller decides whether a stop is accepted; this only
/// forwards the one accepted request onto the channel, so a second
/// tray click cannot signal the recording task twice.
struct ShellStop {
    controller: Arc<RecordingController>,
    stop_tx: watch::Sender<bool>,
}

impl ShellStop {
    const fn new(controller: Arc<RecordingController>, stop_tx: watch::Sender<bool>) -> Self {
        Self {
            controller,
            stop_tx,
        }
    }

    fn request(&self, source: StopSource) {
        if self.controller.request_stop(source) != StopAcceptance::Accepted {
            return;
        }
        let _ = self.stop_tx.send(true);
        tracing::debug!(
            source = source.label(),
            elapsed = self.view().elapsed_label(),
            "recording stop accepted"
        );
    }

    fn view(&self) -> ShellView {
        shell_view(&self.controller.snapshot())
    }
}

#[derive(Copy, Clone, Debug, Eq, PartialEq)]
struct SurfacePlan {
    waveform: bool,
    label: bool,
    floating: bool,
}

impl SurfacePlan {
    fn from_config(config: &ShellConfig) -> Self {
        Self {
            waveform: config.is_enabled(ShellIndicator::MenuBarWaveform),
            label: config.is_enabled(ShellIndicator::MenuBarLabel),
            floating: config.is_enabled(ShellIndicator::FloatingWindow),
        }
    }

    const fn has_tray(self) -> bool {
        self.waveform || self.label
    }
}

struct NativeSurfaces {
    tray: Option<RecordingIndicator>,
    #[cfg(target_os = "macos")]
    floating: Option<FloatingPanel>,
}

impl NativeSurfaces {
    fn start(config: &ShellConfig, accelerator: &str, initial: ShellView) -> Result<Self> {
        let plan = SurfacePlan::from_config(config);

        #[cfg(not(target_os = "macos"))]
        if plan.floating {
            anyhow::bail!(
                "`floating-window` is supported only on macOS; choose a menu-bar-only \
                 `[shell].indicators` subset on this platform"
            );
        }

        #[cfg(target_os = "macos")]
        prepare_application().context("initializing AppKit recording shell")?;

        #[cfg(target_os = "macos")]
        let reduce_motion = plan.waveform && reduce_motion_enabled();
        #[cfg(not(target_os = "macos"))]
        let reduce_motion = false;

        let tray = if plan.has_tray() {
            Some(
                RecordingIndicator::start(
                    plan.waveform,
                    plan.label,
                    accelerator,
                    initial,
                    reduce_motion,
                )
                .context("starting configured status-bar indicators")?,
            )
        } else {
            None
        };

        #[cfg(target_os = "macos")]
        let floating = if plan.floating {
            Some(FloatingPanel::start(initial).context("starting configured floating window")?)
        } else {
            None
        };

        Ok(Self {
            tray,
            #[cfg(target_os = "macos")]
            floating,
        })
    }

    fn render(&mut self, view: ShellView) -> Result<()> {
        if let Some(tray) = &mut self.tray {
            tray.render(view)?;
        }
        #[cfg(target_os = "macos")]
        if let Some(floating) = &mut self.floating {
            floating.render(view);
        }
        Ok(())
    }

    fn poll_stop(&self) -> Option<StopSource> {
        if self
            .tray
            .as_ref()
            .and_then(RecordingIndicator::poll)
            .is_some_and(|command| command == TrayCommand::Stop)
        {
            return Some(StopSource::Tray);
        }
        #[cfg(target_os = "macos")]
        if self.floating.as_ref().is_some_and(FloatingPanel::poll_stop) {
            return Some(StopSource::FloatingWindow);
        }
        None
    }
}

/// Run a recording session under a desktop shell.
///
/// # Errors
///
/// Surfaces tray-icon construction errors, hotkey registration
/// errors (including malformed accelerators), config-load errors, and
/// any error returned by the recording task itself.
pub fn run_record_with_shell(args: Args, runtime: &Runtime) -> Result<()> {
    let cfg = config_service()?.load()?;
    let accelerator = cfg
        .capture
        .hotkey
        .clone()
        .unwrap_or_else(|| DEFAULT_STOP_ACCELERATOR.to_string());

    let (stop_tx, stop_rx) = watch::channel(false);
    // One state model per process, obtained from the composition root
    // rather than constructed here; see `rec::run` for the same.
    let controller = Arc::clone(application(args.root.as_deref())?.recording());

    // The shared preflight first — resolution and the seven checks —
    // and only then this shell's own two: constructing the native
    // surfaces and registering the hotkey. Both run while the
    // controller is `Preparing`, and a failure in either leaves no
    // session folder behind, because none is created until the session
    // run further down.
    let plan = begin_recording(&controller, &args)?;
    let stop = ShellStop::new(Arc::clone(&controller), stop_tx);
    let mut surfaces = match NativeSurfaces::start(&cfg.shell, &accelerator, stop.view()) {
        Ok(surfaces) => surfaces,
        Err(error) => return Err(settle_failure(&controller, error)),
    };
    let hotkey = match HotkeyListener::start(&accelerator) {
        Ok(hotkey) => hotkey,
        Err(error) => {
            return Err(settle_failure(
                &controller,
                error.context("starting global hotkey listener"),
            ))
        }
    };

    let (signal_tx, signal_rx) = mpsc::channel();
    let signal_handle = runtime.spawn(monitor_signals(move || {
        let _ = signal_tx.send(());
    }));
    // No `mark_recording` here. The controller stays `Preparing` until
    // the pipeline publishes `SessionProgress::Recording`, which is the
    // real capture boundary; `run_with_stop` drives both that and the
    // entry into `Saving` from the controller it is handed. Marking
    // recording at this point made every preflight failure inside
    // `run_with_stop` look like a capture failure.
    let task = runtime.spawn(run_with_stop(
        args,
        plan,
        stop_rx,
        Some(Arc::clone(&controller)),
    ));

    let surface_result = pump_until_finished(&mut surfaces, &hotkey, &signal_rx, &stop, &task);
    let recording_result = runtime.block_on(task);
    signal_handle.abort();
    // Every error exit from here settles the controller. A bare `?`
    // would return with the controller stuck wherever it was, so every
    // later snapshot would report a recording that is no longer running
    // and the process could never start another.
    let recording_result = match recording_result.context("joining recording task") {
        Ok(result) => result,
        Err(error) => return Err(settle_failure(&controller, error)),
    };
    if let Err(recording_error) = recording_result {
        if let Err(surface_error) = surface_result {
            tracing::warn!(%surface_error, "shell surface also failed during recording teardown");
        }
        return Err(settle_failure(
            &controller,
            recording_error.context("recording session failed"),
        ));
    }
    settle_success(&controller);
    surface_result
}

/// Records a failure against the controller and settles it back to
/// idle, returning the error the caller should surface.
///
/// The controller labels the failure from the state it was in, so the
/// shell never has to decide whether a failure was preflight, capture,
/// or finalization.
fn settle_failure(controller: &RecordingController, error: anyhow::Error) -> anyhow::Error {
    // A fixed literal rather than `error.to_string()`: the summary is a
    // `Serialize` field of `RecordingEvent` and `RecordingSnapshot`,
    // and a surface-construction or hotkey-registration error can carry
    // a device or accelerator identity. The detailed error is still
    // returned to the caller and traced.
    tracing::error!(%error, "recording attempt failed");
    if let Err(conflict) = controller.fail(RECORDING_FAILURE_SUMMARY) {
        tracing::debug!(%conflict, "recording failure arrived in a state that cannot fail");
    }
    if let Err(conflict) = controller.acknowledge() {
        tracing::debug!(%conflict, "recording controller could not settle to idle");
    }
    error
}

/// Walks the controller through its completion transitions.
fn settle_success(controller: &RecordingController) {
    if controller.snapshot().state == RecordingState::Recording {
        // The capture source ended on its own rather than by request.
        if let Err(conflict) = controller.begin_saving() {
            tracing::debug!(%conflict, "recording controller could not enter saving");
        }
    }
    if let Err(conflict) = controller.complete() {
        tracing::debug!(%conflict, "recording controller could not complete");
    }
    if let Err(conflict) = controller.acknowledge() {
        tracing::debug!(%conflict, "recording controller could not settle to idle");
    }
}

fn pump_until_finished(
    surfaces: &mut NativeSurfaces,
    hotkey: &HotkeyListener,
    signal_rx: &Receiver<()>,
    stop: &ShellStop,
    task: &tokio::task::JoinHandle<Result<()>>,
) -> Result<()> {
    let mut surface_error = None;
    while !task.is_finished() {
        if let Some(source) = surfaces.poll_stop() {
            stop.request(source);
        }
        if matches!(hotkey.poll(), Some(HotkeyEvent::StopRequested)) {
            stop.request(StopSource::Hotkey);
        }
        poll_signal_stop(signal_rx, stop);

        if surface_error.is_none() {
            if let Err(error) = surfaces.render(stop.view()) {
                stop.request(StopSource::SurfaceFailure);
                surface_error = Some(error);
            }
        }
        if let Err(error) = pump_platform(POLL_INTERVAL) {
            stop.request(StopSource::SurfaceFailure);
            surface_error.get_or_insert(error);
            std::thread::sleep(POLL_INTERVAL);
        }
    }
    surface_error.map_or(Ok(()), Err)
}

fn poll_signal_stop(signal_rx: &Receiver<()>, stop: &ShellStop) {
    if signal_rx.try_recv().is_ok() {
        stop.request(StopSource::Signal);
    }
}

#[cfg(target_os = "macos")]
#[allow(unsafe_code)]
fn pump_platform(interval: Duration) -> Result<()> {
    use objc2::MainThreadMarker;
    use objc2_app_kit::{NSApplication, NSEventMask};
    use objc2_foundation::{NSDate, NSDefaultRunLoopMode};

    let mtm = MainThreadMarker::new()
        .ok_or_else(|| anyhow::anyhow!("recording shell event pump left the macOS main thread"))?;
    let application = NSApplication::sharedApplication(mtm);
    // SAFETY: `NSDefaultRunLoopMode` is an immutable process-wide Foundation
    // string constant with static lifetime.
    let run_loop_mode = unsafe { NSDefaultRunLoopMode };
    let deadline = NSDate::dateWithTimeIntervalSinceNow(interval.as_secs_f64());
    if let Some(event) = application.nextEventMatchingMask_untilDate_inMode_dequeue(
        NSEventMask::Any,
        Some(&deadline),
        run_loop_mode,
        true,
    ) {
        application.sendEvent(&event);
    }
    application.updateWindows();
    Ok(())
}

#[cfg(not(target_os = "macos"))]
fn pump_platform(interval: Duration) -> Result<()> {
    std::thread::sleep(interval);
    Ok(())
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use std::path::Path;

    use scrybe_core::config::Config;

    #[test]
    fn test_poll_interval_is_at_most_50_ms_to_keep_ui_responsive() {
        assert!(POLL_INTERVAL <= Duration::from_millis(50));
    }

    #[test]
    fn all_indicator_subsets_map_to_only_the_requested_native_surfaces() {
        let cases = [
            (
                "\"menu-bar-waveform\"",
                SurfacePlan {
                    waveform: true,
                    label: false,
                    floating: false,
                },
            ),
            (
                "\"menu-bar-label\"",
                SurfacePlan {
                    waveform: false,
                    label: true,
                    floating: false,
                },
            ),
            (
                "\"floating-window\"",
                SurfacePlan {
                    waveform: false,
                    label: false,
                    floating: true,
                },
            ),
            (
                "\"menu-bar-waveform\", \"menu-bar-label\"",
                SurfacePlan {
                    waveform: true,
                    label: true,
                    floating: false,
                },
            ),
            (
                "\"menu-bar-waveform\", \"floating-window\"",
                SurfacePlan {
                    waveform: true,
                    label: false,
                    floating: true,
                },
            ),
            (
                "\"menu-bar-label\", \"floating-window\"",
                SurfacePlan {
                    waveform: false,
                    label: true,
                    floating: true,
                },
            ),
            (
                "\"menu-bar-waveform\", \"menu-bar-label\", \"floating-window\"",
                SurfacePlan {
                    waveform: true,
                    label: true,
                    floating: true,
                },
            ),
        ];

        for (input, expected) in cases {
            let config = Config::from_toml_str(
                &format!("schema_version = 1\n[shell]\nindicators = [{input}]"),
                Path::new("config.toml"),
            )
            .unwrap();
            let plan = SurfacePlan::from_config(&config.shell);
            assert_eq!(plan, expected);
            assert_eq!(plan.has_tray(), plan.waveform || plan.label);
        }
    }

    #[test]
    fn elapsed_label_uses_minutes_then_hours_without_allocating_state() {
        let view = ShellView {
            state: ShellState::Recording,
            elapsed: Duration::from_secs(65),
            stop_enabled: true,
        };
        assert_eq!(view.elapsed_label(), "01:05");
        assert_eq!(
            ShellView {
                elapsed: Duration::from_secs(3_661),
                ..view
            }
            .elapsed_label(),
            "01:01:01"
        );
    }

    /// A controller already in `Recording`, which is the state every
    /// shell surface is constructed against.
    fn recording() -> (Arc<RecordingController>, ShellStop, watch::Receiver<bool>) {
        let dir = tempfile::tempdir().unwrap();
        // The same accessor the shell itself uses; there is no other
        // way to obtain a controller from outside the services crate.
        let controller = Arc::clone(application(Some(dir.path())).unwrap().recording());
        controller.begin_preparing().unwrap();
        controller.mark_recording().unwrap();
        let (stop_tx, stop_rx) = watch::channel(false);
        let stop = ShellStop::new(Arc::clone(&controller), stop_tx);
        (controller, stop, stop_rx)
    }

    #[test]
    fn only_the_first_stop_request_reaches_the_recording_task() {
        let (_controller, stop, mut stop_rx) = recording();

        stop.request(StopSource::Tray);
        stop.request(StopSource::Tray);
        stop.request(StopSource::Hotkey);

        assert!(stop_rx.has_changed().unwrap());
        assert!(*stop_rx.borrow_and_update());
        assert!(
            !stop_rx.has_changed().unwrap(),
            "a repeated stop must not signal the recording task again"
        );
    }

    #[test]
    fn the_first_stop_source_wins_and_disables_every_control() {
        let (controller, stop, _stop_rx) = recording();

        stop.request(StopSource::Tray);
        stop.request(StopSource::Hotkey);

        assert_eq!(controller.snapshot().stop_source, Some(StopSource::Tray));
        let view = stop.view();
        assert_eq!(view.state, ShellState::Saving);
        assert!(!view.stop_enabled);
    }

    #[test]
    fn every_user_stop_source_reaches_the_same_saving_transition() {
        for source in [
            StopSource::Tray,
            StopSource::FloatingWindow,
            StopSource::Hotkey,
            StopSource::Signal,
        ] {
            let (controller, stop, _stop_rx) = recording();

            stop.request(source);

            assert_eq!(controller.snapshot().stop_source, Some(source));
            assert_eq!(stop.view().state, ShellState::Saving);
            assert!(!stop.view().stop_enabled);
        }
    }

    #[test]
    fn signal_event_reaches_the_shared_saving_transition() {
        let (controller, stop, mut stop_rx) = recording();
        let (signal_tx, signal_rx) = mpsc::channel();
        signal_tx.send(()).unwrap();

        poll_signal_stop(&signal_rx, &stop);

        assert_eq!(controller.snapshot().stop_source, Some(StopSource::Signal));
        assert!(*stop_rx.borrow_and_update());
        assert_eq!(stop.view().state, ShellState::Saving);
    }

    #[test]
    fn a_live_recording_shows_a_running_timer_and_an_enabled_stop_control() {
        let (_controller, stop, _stop_rx) = recording();

        let view = stop.view();

        assert_eq!(view.state, ShellState::Recording);
        assert!(view.stop_enabled);
    }

    #[test]
    fn preflight_renders_as_a_live_recording_with_the_stop_control_enabled() {
        // `run_with_stop` sits in `Preparing` for the whole preflight —
        // config load, storage root creation, capture-source and
        // provider resolution, notes-runtime model load, device
        // enumeration, capture start — and `NativeSurfaces::start` is
        // handed that view as its initial state. Projecting it as
        // `Saving` pins the waveform, labels the tray `Saving…  00:00`,
        // and dims the floating pill's dot for the whole window.
        let dir = tempfile::tempdir().unwrap();
        let controller = Arc::clone(application(Some(dir.path())).unwrap().recording());
        controller.begin_preparing().unwrap();

        let view = shell_view(&controller.snapshot());

        assert_eq!(view.state, ShellState::Recording);
        // `Preparing` satisfies `accepts_stop`, so projecting it as
        // `Saving` also broke the stop_enabled-implies-Recording
        // invariant the surfaces' own fixtures encode.
        assert!(view.stop_enabled);
    }

    #[test]
    fn a_surface_failure_stops_the_recording_through_the_same_controller() {
        let (controller, stop, mut stop_rx) = recording();

        stop.request(StopSource::SurfaceFailure);

        assert_eq!(
            controller.snapshot().stop_source,
            Some(StopSource::SurfaceFailure)
        );
        assert!(*stop_rx.borrow_and_update());
    }

    #[test]
    fn a_completed_recording_settles_the_controller_back_to_idle() {
        let (controller, stop, _stop_rx) = recording();
        stop.request(StopSource::Tray);

        settle_success(&controller);

        assert_eq!(controller.snapshot().state, RecordingState::Idle);
    }

    #[test]
    fn a_capture_source_that_ends_on_its_own_still_settles_to_idle() {
        let (controller, _stop, _stop_rx) = recording();

        settle_success(&controller);

        assert_eq!(controller.snapshot().state, RecordingState::Idle);
    }

    #[test]
    fn a_failed_recording_settles_to_idle_and_returns_its_error() {
        let (controller, _stop, _stop_rx) = recording();

        let error = settle_failure(&controller, anyhow::anyhow!("capture device disappeared"));

        assert!(error.to_string().contains("capture device disappeared"));
        assert_eq!(controller.snapshot().state, RecordingState::Idle);
    }
}
