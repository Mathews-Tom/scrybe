// Copyright 2026 Mathews Tom
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//     https://www.apache.org/licenses/LICENSE-2.0

//! Deterministic status-bar rendering for an active recording shell.

use std::{io::Cursor, time::Duration};

use anyhow::{ensure, Context, Result};
use crossbeam_channel::Receiver;
use tray_icon::{
    menu::{Menu, MenuEvent, MenuId, MenuItem},
    Icon, TrayIcon, TrayIconBuilder,
};

use crate::shell::{ShellState, ShellView};

const WAVEFORM_SIZE: u32 = 18;
const BRAND_SIZE: u32 = 18;
const BRAND_GAP: u32 = 3;
const FRAME_PERIOD: Duration = Duration::from_millis(250);
const STATIC_FRAME_INDEX: usize = 4;
const WAVEFORM_HEIGHTS: [[u8; 5]; 8] = [
    [4, 8, 14, 8, 4],
    [6, 12, 8, 16, 6],
    [10, 6, 4, 12, 16],
    [6, 12, 16, 12, 6],
    [4, 10, 16, 8, 12],
    [12, 6, 10, 16, 8],
    [16, 10, 6, 12, 4],
    [8, 14, 10, 6, 12],
];
const RED_MIX: [u8; 8] = [0, 64, 128, 192, 255, 192, 128, 64];
const RECORDING_RED: [u8; 3] = [215, 68, 56];
const BRAND_PNG: &[u8] = include_bytes!("../assets/scrybe-status.png");

/// Commands surfaced by the status-bar menu.
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum TrayCommand {
    Stop,
}

/// Status-bar indicator wrapping the platform tray.
pub struct RecordingIndicator {
    tray: TrayIcon,
    state_item: MenuItem,
    stop_item: MenuItem,
    stop_id: MenuId,
    menu_events: Receiver<MenuEvent>,
    frames: Vec<Icon>,
    show_waveform: bool,
    reduce_motion: bool,
    last_state: ShellState,
    last_elapsed_second: u64,
    last_frame: usize,
}

impl RecordingIndicator {
    /// Build the configured status-bar surface on the calling thread.
    ///
    /// # Errors
    ///
    /// Returns an error if menu, icon, or native tray construction fails.
    pub fn start(
        show_waveform: bool,
        show_brand: bool,
        accelerator: &str,
        initial: ShellView,
        reduce_motion: bool,
    ) -> Result<Self> {
        let frames = build_status_frames(show_waveform, show_brand, adaptive_status_foreground())?;
        let initial_frame = frame_index(initial, reduce_motion, show_waveform);
        let state_item = MenuItem::new(menu_state_text(initial), false, None);
        let stop_item = MenuItem::new(
            format!("Stop & save    {accelerator}"),
            initial.stop_enabled,
            None,
        );
        let stop_id = stop_item.id().clone();
        let menu = Menu::new();
        menu.append(&state_item)
            .context("appending recording state menu item")?;
        menu.append(&stop_item)
            .context("appending stop menu item")?;

        let tray = TrayIconBuilder::new()
            .with_menu(Box::new(menu))
            .with_tooltip(tooltip(initial.state))
            .with_icon(frames[initial_frame].clone())
            .with_icon_as_template(false)
            .build()
            .context("building status-bar indicator")?;

        Ok(Self {
            tray,
            state_item,
            stop_item,
            stop_id,
            menu_events: MenuEvent::receiver().clone(),
            frames,
            show_waveform,
            reduce_motion,
            last_state: initial.state,
            last_elapsed_second: initial.elapsed.as_secs(),
            last_frame: initial_frame,
        })
    }

    /// Render a shared shell snapshot without reading captured audio.
    ///
    /// # Errors
    ///
    /// Returns an error if the platform rejects an icon update.
    pub fn render(&mut self, view: ShellView) -> Result<()> {
        let state_changed = view.state != self.last_state;
        if state_changed {
            self.tray.set_tooltip(Some(tooltip(view.state)))?;
            self.last_state = view.state;
        }

        let elapsed_second = view.elapsed.as_secs();
        if elapsed_second != self.last_elapsed_second || state_changed {
            self.state_item.set_text(menu_state_text(view));
            self.last_elapsed_second = elapsed_second;
        }
        self.stop_item.set_enabled(view.stop_enabled);

        let frame = frame_index(view, self.reduce_motion, self.show_waveform);
        if self.last_frame != frame {
            self.tray
                .set_icon_with_as_template(Some(self.frames[frame].clone()), false)
                .context("updating status-bar recording indicator")?;
            self.last_frame = frame;
        }
        Ok(())
    }

    /// Drain pending menu events without blocking.
    pub fn poll(&self) -> Option<TrayCommand> {
        while let Ok(event) = self.menu_events.try_recv() {
            if event.id == self.stop_id {
                return Some(TrayCommand::Stop);
            }
        }
        None
    }
}

const fn tooltip(state: ShellState) -> &'static str {
    match state {
        ShellState::Recording => "scrybe — recording",
        ShellState::Saving => "scrybe — saving",
    }
}
fn menu_state_text(view: ShellView) -> String {
    let state = match view.state {
        ShellState::Recording => "Recording",
        ShellState::Saving => "Saving…",
    };
    format!("{state}  {}", view.elapsed_label())
}

fn frame_index(view: ShellView, reduce_motion: bool, show_waveform: bool) -> usize {
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

fn build_status_frames(
    show_waveform: bool,
    show_brand: bool,
    adaptive_foreground: [u8; 3],
) -> Result<Vec<Icon>> {
    ensure!(
        show_waveform || show_brand,
        "status-bar surface needs a waveform or brand mark"
    );
    let brand = show_brand.then(decode_brand_rgba).transpose()?;
    let width = status_width(show_waveform, show_brand);
    let frame_count = if show_waveform {
        WAVEFORM_HEIGHTS.len()
    } else {
        1
    };

    (0..frame_count)
        .map(|index| {
            let mut rgba = vec![0; usize::try_from(width * WAVEFORM_SIZE * 4).unwrap_or_default()];
            if show_waveform {
                draw_waveform(
                    &mut rgba,
                    width,
                    WAVEFORM_HEIGHTS[index],
                    waveform_color(adaptive_foreground, RED_MIX[index]),
                );
            }
            if let Some(brand) = &brand {
                let offset = if show_waveform {
                    WAVEFORM_SIZE + BRAND_GAP
                } else {
                    0
                };
                blit_brand(&mut rgba, width, offset, brand);
            }
            Icon::from_rgba(rgba, width, WAVEFORM_SIZE)
                .context("constructing status-bar recording frame")
        })
        .collect()
}

const fn status_width(show_waveform: bool, show_brand: bool) -> u32 {
    match (show_waveform, show_brand) {
        (true, true) => WAVEFORM_SIZE + BRAND_GAP + BRAND_SIZE,
        (true, false) => WAVEFORM_SIZE,
        (false, true) => BRAND_SIZE,
        (false, false) => 0,
    }
}

fn decode_brand_rgba() -> Result<Vec<u8>> {
    let decoder = png::Decoder::new(Cursor::new(BRAND_PNG));
    let mut reader = decoder
        .read_info()
        .context("reading embedded Scrybe status brand")?;
    let output_size = reader
        .output_buffer_size()
        .ok_or_else(|| anyhow::anyhow!("embedded Scrybe status brand exceeds decoder limits"))?;
    let mut rgba = vec![0; output_size];
    let info = reader
        .next_frame(&mut rgba)
        .context("decoding embedded Scrybe status brand")?;
    ensure!(
        info.width == BRAND_SIZE
            && info.height == BRAND_SIZE
            && info.color_type == png::ColorType::Rgba
            && info.bit_depth == png::BitDepth::Eight,
        "embedded Scrybe status brand must be 18x18 RGBA8"
    );
    rgba.truncate(info.buffer_size());
    Ok(rgba)
}

fn blit_brand(canvas: &mut [u8], canvas_width: u32, offset_x: u32, brand: &[u8]) {
    let canvas_width = usize::try_from(canvas_width).unwrap_or_default();
    let offset_x = usize::try_from(offset_x).unwrap_or_default();
    let brand_size = usize::try_from(BRAND_SIZE).unwrap_or_default();
    for row in 0..brand_size {
        let source = row * brand_size * 4;
        let destination = (row * canvas_width + offset_x) * 4;
        canvas[destination..destination + brand_size * 4]
            .copy_from_slice(&brand[source..source + brand_size * 4]);
    }
}

fn waveform_color(adaptive_foreground: [u8; 3], red_mix: u8) -> [u8; 4] {
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
fn waveform_rgba(heights: [u8; 5], color: [u8; 4]) -> Vec<u8> {
    let mut rgba = vec![0; usize::try_from(WAVEFORM_SIZE * WAVEFORM_SIZE * 4).unwrap_or_default()];
    draw_waveform(&mut rgba, WAVEFORM_SIZE, heights, color);
    rgba
}

fn draw_waveform(canvas: &mut [u8], canvas_width: u32, heights: [u8; 5], color: [u8; 4]) {
    let size = usize::try_from(WAVEFORM_SIZE).unwrap_or(18);
    let canvas_width = usize::try_from(canvas_width).unwrap_or(size);
    for (bar, height) in heights.iter().copied().enumerate() {
        let height = usize::from(height);
        let first_row = (size - height) / 2;
        let first_column = 1 + bar * 3;
        for row in first_row..first_row + height {
            for column in first_column..first_column + 2 {
                let pixel = (row * canvas_width + column) * 4;
                canvas[pixel..pixel + 4].copy_from_slice(&color);
            }
        }
    }
}

#[cfg(target_os = "macos")]
fn adaptive_status_foreground() -> [u8; 3] {
    use objc2::MainThreadMarker;
    use objc2_app_kit::NSApplication;

    let Some(mtm) = MainThreadMarker::new() else {
        return [0, 0, 0];
    };
    let appearance = NSApplication::sharedApplication(mtm)
        .effectiveAppearance()
        .name()
        .to_string();
    if appearance.contains("Dark") {
        [255, 255, 255]
    } else {
        [0, 0, 0]
    }
}

#[cfg(not(target_os = "macos"))]
const fn adaptive_status_foreground() -> [u8; 3] {
    [0, 0, 0]
}

#[cfg(test)]
#[allow(clippy::panic)]
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
    fn waveform_uses_a_fixed_eight_frame_four_hertz_sequence() {
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
        assert_eq!(
            frame_index(view(ShellState::Recording, 2_000), false, true),
            0
        );
        let adaptive = [255, 255, 255, 255];
        assert_ne!(
            waveform_rgba(WAVEFORM_HEIGHTS[0], adaptive),
            waveform_rgba(WAVEFORM_HEIGHTS[1], adaptive)
        );
        let opaque_pixels = waveform_rgba(WAVEFORM_HEIGHTS[0], adaptive)
            .as_chunks::<4>()
            .0
            .iter()
            .filter(|pixel| pixel[3] == 255)
            .count();
        assert_eq!(opaque_pixels, 76);
    }

    #[test]
    fn reduce_motion_and_saving_hold_the_static_middle_frame() {
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
    fn brand_replaces_status_text_while_elapsed_stays_in_the_menu() {
        assert_eq!(status_width(true, true), 39);
        assert_eq!(status_width(false, true), BRAND_SIZE);
        let Ok(brand) = decode_brand_rgba() else {
            panic!("embedded brand asset must decode");
        };
        assert_eq!(
            brand.len(),
            usize::try_from(BRAND_SIZE * BRAND_SIZE * 4).unwrap_or_default()
        );
        assert_eq!(
            menu_state_text(view(ShellState::Recording, 65_000)),
            "Recording  01:05"
        );
    }

    #[test]
    fn waveform_color_cycles_from_adaptive_foreground_to_recording_red() {
        assert_eq!(waveform_color([255, 255, 255], 0), [255, 255, 255, 255]);
        assert_eq!(
            waveform_color([255, 255, 255], 255),
            [RECORDING_RED[0], RECORDING_RED[1], RECORDING_RED[2], 255]
        );
    }
}
