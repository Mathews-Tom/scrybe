// Copyright 2026 Mathews Tom
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//     https://www.apache.org/licenses/LICENSE-2.0

//! Main-thread shell driver for `scrybe record --shell`.
//!
//! `tray_icon::TrayIcon` and `global_hotkey::GlobalHotKeyManager` are
//! `!Send` and, on macOS specifically, must be created on the main
//! thread; their event delivery depends on that same thread pumping a
//! `CFRunLoop`. To honour that constraint while still using a tokio
//! runtime for the recording pipeline, this module owns the main
//! thread for the duration of a `--shell` session:
//!
//! 1. Builds the `RecordingIndicator` and `HotkeyListener` on the
//!    calling thread (which `main()` keeps as the main thread).
//! 2. Spawns the recording task on the supplied tokio runtime via
//!    [`record::run_with_stop`].
//! 3. Polls tray and hotkey receivers in a tight, non-async loop on
//!    the main thread, pumping `CFRunLoopRunInMode` on macOS so
//!    Carbon hotkey events and `NSStatusItem` menu callbacks land.
//! 4. Routes tray and hotkey stop requests through one idempotent
//!    coordinator shared by every native control.
//! 5. Joins the task once it finishes.
//!
//! ## Platform validation
//!
//! macOS is the validated target for v0.1; the `CFRunLoopRunInMode`
//! pump is the load-bearing piece. Linux and Windows are *compile*
//! targets — the same module builds and links — but event delivery
//! has not been hardware-validated. `tray-icon 0.23` on Linux uses
//! `libappindicator-rs`, which spawns its own GTK thread, and
//! `global-hotkey 0.7` on Linux taps X11 / Wayland directly; both
//! *should* deliver events without an explicit driver, but this is
//! unverified in CI. Treat Linux / Windows shell support as a
//! best-effort path until self-hosted Tier-3 runners come online
//! (`.docs/development-plan.md` §11, §15.5).

use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use tokio::runtime::Runtime;
use tokio::sync::watch;

use crate::commands::rec::{run_with_stop, Args};
use crate::hotkey::{HotkeyEvent, HotkeyListener, DEFAULT_TOGGLE_ACCELERATOR};
use crate::runtime::load_or_default_config;
use crate::tray::{IndicatorState, RecordingIndicator, TrayCommand};

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
    Hotkey,
}

impl StopSource {
    const fn label(self) -> &'static str {
        match self {
            Self::Tray => "tray",
            Self::Hotkey => "hotkey",
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
        .unwrap_or_else(|| DEFAULT_TOGGLE_ACCELERATOR.to_string());

    let indicator =
        RecordingIndicator::start(IndicatorState::Idle).context("starting tray indicator")?;
    let hotkey = HotkeyListener::start(&accelerator).context("starting global hotkey listener")?;

    indicator.set_state(IndicatorState::Recording);

    let (stop_tx, stop_rx) = watch::channel(false);
    let mut stop = StopCoordinator::new(Instant::now(), stop_tx);
    let task = runtime.spawn(run_with_stop(args, stop_rx));

    pump_until_finished(&indicator, &hotkey, &mut stop, &task);

    indicator.set_state(IndicatorState::Idle);

    runtime
        .block_on(task)
        .context("joining recording task")?
        .context("recording session failed")
}

fn pump_until_finished(
    indicator: &RecordingIndicator,
    hotkey: &HotkeyListener,
    stop: &mut StopCoordinator,
    task: &tokio::task::JoinHandle<Result<()>>,
) {
    while !task.is_finished() {
        if matches!(indicator.poll(), Some(TrayCommand::Quit)) {
            apply_stop_request(stop, indicator, StopSource::Tray);
        }
        if matches!(hotkey.poll(), Some(HotkeyEvent::Toggle)) {
            apply_stop_request(stop, indicator, StopSource::Hotkey);
        }
        pump_platform(POLL_INTERVAL);
    }
}

fn apply_stop_request(
    stop: &mut StopCoordinator,
    indicator: &RecordingIndicator,
    source: StopSource,
) {
    if let Some(view) = stop.request_stop(source, Instant::now()) {
        debug_assert_eq!(view.state, ShellState::Saving);
        debug_assert!(!view.stop_enabled);
        indicator.set_state(IndicatorState::Saving);
        tracing::debug!(
            source = source.label(),
            elapsed = view.elapsed_label(),
            "recording stop accepted"
        );
    }
}

#[cfg(target_os = "macos")]
#[allow(unsafe_code)]
fn pump_platform(interval: Duration) {
    use core_foundation::runloop::{kCFRunLoopDefaultMode, CFRunLoopRunInMode};
    let seconds = interval.as_secs_f64();
    // SAFETY: `kCFRunLoopDefaultMode` is a 'static `CFStringRef`
    // exported by Core Foundation. `CFRunLoopRunInMode` runs the
    // current thread's run loop for at most `seconds` and returns; we
    // call it from the main thread (the only thread `--shell` ever
    // pumps from), so the Carbon hotkey handler and NSStatusItem
    // callbacks installed during `start()` get the dispatch they
    // need. The third argument `0` (Boolean false) lets multiple
    // events fire in a single tick.
    unsafe {
        let _ = CFRunLoopRunInMode(kCFRunLoopDefaultMode, seconds, 0);
    }
}

#[cfg(not(target_os = "macos"))]
fn pump_platform(interval: Duration) {
    std::thread::sleep(interval);
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    #[test]
    fn test_poll_interval_is_at_most_50_ms_to_keep_ui_responsive() {
        assert!(POLL_INTERVAL <= Duration::from_millis(50));
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
