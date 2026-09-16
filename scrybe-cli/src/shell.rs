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
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use scrybe_core::config::{ShellConfig, ShellIndicator};
use tokio::runtime::Runtime;
use tokio::sync::watch;

use crate::commands::rec::{monitor_signals, run_with_stop, Args};
#[cfg(target_os = "macos")]
use crate::floating_panel::{prepare_application, reduce_motion_enabled, FloatingPanel};
use crate::hotkey::{HotkeyEvent, HotkeyListener, DEFAULT_STOP_ACCELERATOR};
use crate::runtime::load_or_default_config;
use crate::tray::{RecordingIndicator, TrayCommand};

/// Pump interval for the polling loop. On macOS this is the duration
/// passed to `CFRunLoopRunInMode`; on other platforms it bounds the
/// `thread::sleep` between event polls.
const POLL_INTERVAL: Duration = Duration::from_millis(20);

/// Recording-shell lifecycle shown by every configured surface.
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum ShellState {
    Recording,
    Saving,
}

/// Native control that won the stop race.
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum StopSource {
    Tray,
    FloatingWindow,
    SurfaceFailure,
    Hotkey,
    Signal,
}

impl StopSource {
    const fn label(self) -> &'static str {
        match self {
            Self::Tray => "tray",
            Self::FloatingWindow => "floating-window",
            Self::SurfaceFailure => "surface-failure",
            Self::Hotkey => "hotkey",
            Self::Signal => "signal",
        }
    }
}

/// One rendering-independent snapshot for all shell surfaces.
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub struct ShellView {
    pub state: ShellState,
    pub elapsed: Duration,
    pub stop_enabled: bool,
}

impl ShellView {
    pub fn elapsed_label(self) -> String {
        let total_seconds = self.elapsed.as_secs();
        let seconds = total_seconds % 60;
        let minutes = (total_seconds / 60) % 60;
        let hours = total_seconds / 3_600;
        if hours == 0 {
            format!("{minutes:02}:{seconds:02}")
        } else {
            format!("{hours:02}:{minutes:02}:{seconds:02}")
        }
    }
}

/// Owns the only Recording -> Saving transition and stop signal.
struct StopCoordinator {
    started_at: Instant,
    frozen_elapsed: Option<Duration>,
    first_source: Option<StopSource>,
    stop_tx: watch::Sender<bool>,
}

impl StopCoordinator {
    const fn new(started_at: Instant, stop_tx: watch::Sender<bool>) -> Self {
        Self {
            started_at,
            frozen_elapsed: None,
            first_source: None,
            stop_tx,
        }
    }

    fn request_stop(&mut self, source: StopSource, now: Instant) -> Option<ShellView> {
        if self.frozen_elapsed.is_some() {
            return None;
        }
        self.frozen_elapsed = Some(now.saturating_duration_since(self.started_at));
        self.first_source = Some(source);
        let _ = self.stop_tx.send(true);
        Some(self.view(now))
    }

    fn view(&self, now: Instant) -> ShellView {
        let elapsed = self
            .frozen_elapsed
            .unwrap_or_else(|| now.saturating_duration_since(self.started_at));
        let state = if self.frozen_elapsed.is_some() {
            ShellState::Saving
        } else {
            ShellState::Recording
        };
        ShellView {
            state,
            elapsed,
            stop_enabled: state == ShellState::Recording,
        }
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
    let cfg = load_or_default_config()?;
    let accelerator = cfg
        .capture
        .hotkey
        .clone()
        .unwrap_or_else(|| DEFAULT_STOP_ACCELERATOR.to_string());

    let (stop_tx, stop_rx) = watch::channel(false);
    let mut stop = StopCoordinator::new(Instant::now(), stop_tx);
    let initial = stop.view(Instant::now());
    let mut surfaces = NativeSurfaces::start(&cfg.shell, &accelerator, initial)?;
    let hotkey = HotkeyListener::start(&accelerator).context("starting global hotkey listener")?;
    let (signal_tx, signal_rx) = mpsc::channel();
    let signal_handle = runtime.spawn(monitor_signals(move || {
        let _ = signal_tx.send(());
    }));
    let task = runtime.spawn(run_with_stop(args, stop_rx));

    let surface_result = pump_until_finished(&mut surfaces, &hotkey, &signal_rx, &mut stop, &task);
    let recording_result = runtime.block_on(task);
    signal_handle.abort();
    let recording_result = recording_result.context("joining recording task")?;
    if let Err(recording_error) = recording_result {
        if let Err(surface_error) = surface_result {
            tracing::warn!(%surface_error, "shell surface also failed during recording teardown");
        }
        return Err(recording_error).context("recording session failed");
    }
    surface_result
}

fn pump_until_finished(
    surfaces: &mut NativeSurfaces,
    hotkey: &HotkeyListener,
    signal_rx: &Receiver<()>,
    stop: &mut StopCoordinator,
    task: &tokio::task::JoinHandle<Result<()>>,
) -> Result<()> {
    let mut surface_error = None;
    while !task.is_finished() {
        if let Some(source) = surfaces.poll_stop() {
            apply_stop_request(stop, source);
        }
        if matches!(hotkey.poll(), Some(HotkeyEvent::StopRequested)) {
            apply_stop_request(stop, StopSource::Hotkey);
        }
        poll_signal_stop(signal_rx, stop);

        if surface_error.is_none() {
            let view = stop.view(Instant::now());
            if let Err(error) = surfaces.render(view) {
                apply_stop_request(stop, StopSource::SurfaceFailure);
                surface_error = Some(error);
            }
        }
        if let Err(error) = pump_platform(POLL_INTERVAL) {
            apply_stop_request(stop, StopSource::SurfaceFailure);
            surface_error.get_or_insert(error);
            std::thread::sleep(POLL_INTERVAL);
        }
    }
    surface_error.map_or(Ok(()), Err)
}

fn poll_signal_stop(signal_rx: &Receiver<()>, stop: &mut StopCoordinator) {
    if signal_rx.try_recv().is_ok() {
        apply_stop_request(stop, StopSource::Signal);
    }
}

fn apply_stop_request(stop: &mut StopCoordinator, source: StopSource) {
    if let Some(view) = stop.request_stop(source, Instant::now()) {
        debug_assert_eq!(view.state, ShellState::Saving);
        debug_assert!(!view.stop_enabled);
        tracing::debug!(
            source = source.label(),
            elapsed = view.elapsed_label(),
            "recording stop accepted"
        );
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

    #[test]
    fn first_stop_source_freezes_elapsed_and_disables_every_control() {
        let started_at = Instant::now();
        let (stop_tx, mut stop_rx) = watch::channel(false);
        let mut stop = StopCoordinator::new(started_at, stop_tx);

        assert!(stop
            .request_stop(StopSource::Tray, started_at + Duration::from_secs(7))
            .is_some());
        assert!(stop
            .request_stop(StopSource::Tray, started_at + Duration::from_secs(9))
            .is_none());
        assert!(stop
            .request_stop(StopSource::Hotkey, started_at + Duration::from_secs(11))
            .is_none());
        assert!(stop_rx.has_changed().unwrap());
        assert!(*stop_rx.borrow_and_update());
        assert!(!stop_rx.has_changed().unwrap());
        assert_eq!(stop.first_source, Some(StopSource::Tray));
        let view = stop.view(started_at + Duration::from_secs(20));
        assert_eq!(view.state, ShellState::Saving);
        assert_eq!(view.elapsed, Duration::from_secs(7));
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
            let started_at = Instant::now();
            let (stop_tx, _stop_rx) = watch::channel(false);
            let mut stop = StopCoordinator::new(started_at, stop_tx);
            let view = stop
                .request_stop(source, started_at + Duration::from_secs(3))
                .unwrap();

            assert_eq!(stop.first_source, Some(source));
            assert_eq!(view.state, ShellState::Saving);
            assert_eq!(view.elapsed, Duration::from_secs(3));
            assert!(!view.stop_enabled);
        }
    }

    #[test]
    fn signal_event_reaches_the_shared_saving_transition() {
        let started_at = Instant::now();
        let (stop_tx, mut stop_rx) = watch::channel(false);
        let mut stop = StopCoordinator::new(started_at, stop_tx);
        let (signal_tx, signal_rx) = mpsc::channel();
        signal_tx.send(()).unwrap();

        poll_signal_stop(&signal_rx, &mut stop);

        assert_eq!(stop.first_source, Some(StopSource::Signal));
        assert!(*stop_rx.borrow_and_update());
        assert_eq!(stop.view(Instant::now()).state, ShellState::Saving);
    }

    #[test]
    fn recording_elapsed_advances_until_the_first_stop() {
        let started_at = Instant::now();
        let (stop_tx, _stop_rx) = watch::channel(false);
        let stop = StopCoordinator::new(started_at, stop_tx);

        assert_eq!(
            stop.view(started_at + Duration::from_secs(4)),
            ShellView {
                state: ShellState::Recording,
                elapsed: Duration::from_secs(4),
                stop_enabled: true,
            }
        );
    }
}
