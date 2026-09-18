// Copyright 2026 Mathews Tom
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//     https://www.apache.org/licenses/LICENSE-2.0

//! What every native indicator renders from.
//!
//! One rendering-independent snapshot, so the status item, the floating
//! panel, and anything else showing a live recording cannot disagree
//! about what they are showing. It carries no dependency on the service
//! layer: whichever frontend owns a recording projects its own state
//! model onto this, and the widgets never see a controller.

use std::time::Duration;

/// The recording lifecycle a native indicator shows.
///
/// Two states, not six. An indicator the size of a menu-bar item has
/// room for "this is running" and "this is finishing", and a reader
/// watching one wants exactly that distinction; the full state model
/// belongs to the surface that has room to render it.
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum ShellState {
    Recording,
    Saving,
}

/// One snapshot, as every native indicator sees it.
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub struct ShellView {
    pub state: ShellState,
    pub elapsed: Duration,
    pub stop_enabled: bool,
}

impl ShellView {
    /// `HH:MM:SS`, or `MM:SS` under an hour.
    #[must_use]
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
