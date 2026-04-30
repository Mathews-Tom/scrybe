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
//! 4. Translates a tray `Quit` or hotkey `Toggle` into a `watch`
//!    stop signal consumed by the recording task.
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

use std::time::Duration;

use anyhow::{Context, Result};
use tokio::runtime::Runtime;
use tokio::sync::watch;

use crate::commands::record::{run_with_stop, Args};
use crate::hotkey::{HotkeyEvent, HotkeyListener, DEFAULT_TOGGLE_ACCELERATOR};
use crate::runtime::load_or_default_config;
use crate::tray::{IndicatorState, RecordingIndicator, TrayCommand};

/// Pump interval for the polling loop. On macOS this is the duration
/// passed to `CFRunLoopRunInMode`; on other platforms it bounds the
/// `thread::sleep` between event polls.
const POLL_INTERVAL: Duration = Duration::from_millis(20);

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
    let task = runtime.spawn(run_with_stop(args, stop_rx));

    pump_until_finished(&indicator, &hotkey, &stop_tx, &task);

    indicator.set_state(IndicatorState::Idle);

    runtime
        .block_on(task)
        .context("joining recording task")?
        .context("recording session failed")
}

fn pump_until_finished(
    indicator: &RecordingIndicator,
    hotkey: &HotkeyListener,
    stop_tx: &watch::Sender<bool>,
    task: &tokio::task::JoinHandle<Result<()>>,
) {
    while !task.is_finished() {
        if matches!(indicator.poll(), Some(TrayCommand::Quit)) {
            let _ = stop_tx.send(true);
        }
        if matches!(hotkey.poll(), Some(HotkeyEvent::Toggle)) {
            let _ = stop_tx.send(true);
        }
        pump_platform(POLL_INTERVAL);
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
}
