// Copyright 2026 Mathews Tom
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//     https://www.apache.org/licenses/LICENSE-2.0

//! Status-bar indicator for an in-progress recording.
//!
//! Compiled only with the `cli-shell` cargo feature. Wraps
//! `tray_icon::TrayIcon` (`!Send`, must live on the thread that
//! created it — main thread on macOS) and exposes platform events as
//! a `crossbeam_channel::Receiver` so the main-thread shell driver
//! (`scrybe-cli::shell`) can poll without holding the indicator
//! across `await` points.

use anyhow::{Context, Result};
use crossbeam_channel::Receiver;
use tray_icon::{
    menu::{Menu, MenuEvent, MenuId, MenuItem},
    TrayIcon, TrayIconBuilder,
};

/// Commands surfaced by the tray menu to the recording loop.
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum TrayCommand {
    /// User picked "Quit" from the tray menu.
    Quit,
}

/// Visible state shown on the tray.
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum IndicatorState {
    Idle,
    Recording,
}

impl IndicatorState {
    /// Tooltip text rendered on the platform tray.
    #[must_use]
    pub const fn tooltip(self) -> &'static str {
        match self {
            Self::Idle => "scrybe — idle",
            Self::Recording => "scrybe — recording",
        }
    }
}

/// Status-bar indicator wrapping the platform tray. Constructed on the
/// thread that owns the platform run loop; never moved across threads.
pub struct RecordingIndicator {
    tray: TrayIcon,
    quit_id: MenuId,
    menu_events: Receiver<MenuEvent>,
}

impl RecordingIndicator {
    /// Build the platform indicator on the calling thread.
    ///
    /// # Errors
    ///
    /// Propagates errors from `tray_icon::TrayIconBuilder::build` (icon
    /// creation, menu wiring, platform handshake). On macOS this also
    /// returns an error if invoked off the main thread.
    pub fn start(initial: IndicatorState) -> Result<Self> {
        let menu = Menu::new();
        let quit_item = MenuItem::new("Quit scrybe", true, None);
        let quit_id = quit_item.id().clone();
        menu.append(&quit_item)
            .context("appending quit menu item")?;

        let tray = TrayIconBuilder::new()
            .with_menu(Box::new(menu))
            .with_tooltip(initial.tooltip())
            .with_title("scrybe")
            .build()
            .context("building tray icon")?;

        let menu_events = MenuEvent::receiver().clone();
        Ok(Self {
            tray,
            quit_id,
            menu_events,
        })
    }

    /// Update the visible state. `&self` because `tray_icon::TrayIcon`
    /// uses interior mutability.
    pub fn set_state(&self, state: IndicatorState) {
        let _ = self.tray.set_tooltip(Some(state.tooltip()));
    }

    /// Drain pending menu events without blocking, returning the
    /// first `TrayCommand` they translate to. Returns `None` when
    /// the queue is empty or only carries menu items the recorder
    /// does not subscribe to.
    pub fn poll(&self) -> Option<TrayCommand> {
        while let Ok(event) = self.menu_events.try_recv() {
            if event.id == self.quit_id {
                return Some(TrayCommand::Quit);
            }
        }
        None
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    #[test]
    fn test_indicator_state_idle_returns_idle_tooltip() {
        assert_eq!(IndicatorState::Idle.tooltip(), "scrybe — idle");
    }

    #[test]
    fn test_indicator_state_recording_returns_recording_tooltip() {
        assert_eq!(IndicatorState::Recording.tooltip(), "scrybe — recording");
    }
}
