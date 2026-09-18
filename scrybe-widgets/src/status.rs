// Copyright 2026 Mathews Tom
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//     https://www.apache.org/licenses/LICENSE-2.0

//! What a status-bar indicator draws, decided arithmetically.
//!
//! The geometry, the animation sequence, and the colour blend — none of
//! which needs a menu bar to exercise. The tray *object* that renders
//! them is not here: the command-line binary builds one through
//! `tray-icon` and the desktop host owns a Tauri-native one, and both
//! draw from these.

use std::time::Duration;

use crate::view::{ShellState, ShellView};

/// The waveform's side, in points.
pub const WAVEFORM_SIZE: u32 = 18;
/// The brand mark's side, in points.
pub const BRAND_SIZE: u32 = 18;
/// The gap between the waveform and the brand mark.
pub const BRAND_GAP: u32 = 3;

/// How long one waveform frame is shown. Eight frames at this period is
/// a 4 Hz cycle — visible as motion, slow enough not to read as flicker.
pub const FRAME_PERIOD: Duration = Duration::from_millis(250);

/// The frame held when motion is suppressed.
///
/// A middle frame rather than the first: a paused waveform at rest
/// reads as a signal that has stopped, and this one reads as a signal
/// that is simply not being animated.
pub const STATIC_FRAME_INDEX: usize = 4;

/// The eight waveform frames, as bar heights in points.
pub const WAVEFORM_HEIGHTS: [[u8; 5]; 8] = [
    [4, 8, 14, 8, 4],
    [6, 12, 8, 16, 6],
    [10, 6, 4, 12, 16],
    [6, 12, 16, 12, 6],
    [4, 10, 16, 8, 12],
    [12, 6, 10, 16, 8],
    [16, 10, 6, 12, 4],
    [8, 14, 10, 6, 12],
];

/// How much recording red each frame carries, as a 0–255 mix.
pub const RED_MIX: [u8; 8] = [0, 64, 128, 192, 255, 192, 128, 64];

/// The red a running recording pulses toward.
pub const RECORDING_RED: [u8; 3] = [215, 68, 56];

/// Which waveform frame `view` is on.
///
/// A recording animates on the one monotonic elapsed time it already
/// carries, so the animation has no clock of its own to drift against.
/// A suppressed or saving indicator holds [`STATIC_FRAME_INDEX`].
#[must_use]
pub fn frame_index(view: ShellView, reduce_motion: bool, show_waveform: bool) -> usize {
    if !show_waveform {
        return 0;
    }
    if reduce_motion || view.state == ShellState::Saving {
        return STATIC_FRAME_INDEX;
    }
    let frame = (view.elapsed.as_millis() / FRAME_PERIOD.as_millis())
        % u128::try_from(WAVEFORM_HEIGHTS.len()).unwrap_or(1);
    usize::try_from(frame).unwrap_or(STATIC_FRAME_INDEX)
}

/// How wide the status image is, for the parts that are shown.
#[must_use]
pub const fn status_width(show_waveform: bool, show_brand: bool) -> u32 {
    match (show_waveform, show_brand) {
        (true, true) => WAVEFORM_SIZE + BRAND_GAP + BRAND_SIZE,
        (true, false) => WAVEFORM_SIZE,
        (false, true) => BRAND_SIZE,
        (false, false) => 0,
    }
}

/// One waveform bar's colour, blended from the menu bar's own
/// foreground toward [`RECORDING_RED`].
///
/// Blending from the adaptive foreground rather than using a fixed
/// colour is what keeps the indicator legible in both menu-bar
/// appearances without the process having to pick one.
#[must_use]
pub fn waveform_color(adaptive_foreground: [u8; 3], red_mix: u8) -> [u8; 4] {
    let red_mix = u16::from(red_mix);
    let adaptive_mix = 255 - red_mix;
    let blend = |adaptive: u8, red: u8| {
        u8::try_from((u16::from(adaptive) * adaptive_mix + u16::from(red) * red_mix) / 255)
            .unwrap_or(red)
    };
    [
        blend(adaptive_foreground[0], RECORDING_RED[0]),
        blend(adaptive_foreground[1], RECORDING_RED[1]),
        blend(adaptive_foreground[2], RECORDING_RED[2]),
        255,
    ]
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    fn view(state: ShellState, millis: u64) -> ShellView {
        ShellView {
            state,
            elapsed: Duration::from_millis(millis),
            stop_enabled: state == ShellState::Recording,
        }
    }

    #[test]
    fn test_the_waveform_runs_a_fixed_eight_frame_four_hertz_sequence() {
        assert_eq!(WAVEFORM_HEIGHTS.len(), 8);
        assert_eq!(FRAME_PERIOD, Duration::from_millis(250));
        assert_eq!(frame_index(view(ShellState::Recording, 0), false, true), 0);
        assert_eq!(
            frame_index(view(ShellState::Recording, 249), false, true),
            0
        );
        assert_eq!(
            frame_index(view(ShellState::Recording, 250), false, true),
            1
        );
        // Two seconds is eight frames, so the sequence is back at its
        // start: this is what makes it a cycle rather than a ramp.
        assert_eq!(
            frame_index(view(ShellState::Recording, 2_000), false, true),
            0
        );
    }

    #[test]
    fn test_no_two_adjacent_waveform_frames_are_identical() {
        for pair in WAVEFORM_HEIGHTS.windows(2) {
            assert_ne!(
                pair[0], pair[1],
                "two identical adjacent frames would read as a dropped frame"
            );
        }
    }

    #[test]
    fn test_reduce_motion_and_saving_hold_the_static_middle_frame() {
        assert_eq!(
            frame_index(view(ShellState::Recording, 1_750), true, true),
            STATIC_FRAME_INDEX
        );
        assert_eq!(
            frame_index(view(ShellState::Saving, 1_750), false, true),
            STATIC_FRAME_INDEX
        );
        assert_eq!(
            frame_index(view(ShellState::Recording, 1_750), false, false),
            0
        );
    }

    #[test]
    fn test_the_status_image_is_only_as_wide_as_what_it_shows() {
        assert_eq!(status_width(true, true), 39);
        assert_eq!(status_width(false, true), BRAND_SIZE);
        assert_eq!(status_width(true, false), WAVEFORM_SIZE);
        assert_eq!(status_width(false, false), 0);
    }

    #[test]
    fn test_the_waveform_colour_runs_from_the_adaptive_foreground_to_recording_red() {
        assert_eq!(waveform_color([255, 255, 255], 0), [255, 255, 255, 255]);
        assert_eq!(
            waveform_color([255, 255, 255], 255),
            [RECORDING_RED[0], RECORDING_RED[1], RECORDING_RED[2], 255]
        );
    }

    /// The mix is a cycle, not a ramp: it must return to where it
    /// started, or the indicator would settle on red and stay there.
    #[test]
    fn test_the_red_mix_is_a_cycle_over_the_same_eight_frames() {
        assert_eq!(RED_MIX.len(), WAVEFORM_HEIGHTS.len());
        assert_eq!(RED_MIX[0], 0);
        assert_eq!(*RED_MIX.iter().max().unwrap(), 255);
        assert_eq!(RED_MIX[1], RED_MIX[RED_MIX.len() - 1]);
    }

    #[test]
    fn test_an_elapsed_label_gains_an_hours_field_only_at_an_hour() {
        assert_eq!(view(ShellState::Recording, 65_000).elapsed_label(), "01:05");
        assert_eq!(
            view(ShellState::Recording, 3_661_000).elapsed_label(),
            "01:01:01"
        );
    }
}
