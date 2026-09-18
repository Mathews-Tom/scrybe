// Copyright 2026 Mathews Tom
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//     https://www.apache.org/licenses/LICENSE-2.0

//! Deterministic status-bar rendering for an active recording shell.

use std::io::Cursor;

use anyhow::{ensure, Context, Result};
use crossbeam_channel::Receiver;
use tray_icon::{
    menu::{Menu, MenuEvent, MenuId, MenuItem},
    Icon, TrayIcon, TrayIconBuilder,
};

// The geometry, the animation sequence, and the colour blend live in
// `scrybe-widgets`, where the desktop host's own status indicator draws
// from them too. What stays here is the `tray-icon` object this binary
// builds and the image it rasterises into.
use scrybe_widgets::status::{
    frame_index, status_width, waveform_color, BRAND_GAP, BRAND_SIZE, RED_MIX, WAVEFORM_HEIGHTS,
    WAVEFORM_SIZE,
};
use scrybe_widgets::{ShellState, ShellView};
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
    use std::time::Duration;

    use super::*;

    fn view(state: ShellState, millis: u64) -> ShellView {
        ShellView {
            state,
            elapsed: Duration::from_millis(millis),
            stop_enabled: state == ShellState::Recording,
        }
    }

    /// The frame sequence itself is `scrybe-widgets`' and is asserted
    /// there. What this binary owns is the rasterisation of one frame
    /// into pixels, so that is what stays asserted here: two different
    /// frames must produce different images, and one frame must fill
    /// exactly the bars its heights describe.
    #[test]
    fn each_waveform_frame_rasterises_to_the_bars_its_heights_describe() {
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
        assert_eq!(
            opaque_pixels,
            usize::from(
                WAVEFORM_HEIGHTS[0]
                    .iter()
                    .copied()
                    .map(u16::from)
                    .sum::<u16>()
            ) * 2,
            "each bar is two points wide, so the lit pixels are twice the summed heights"
        );
    }

    #[test]
    fn brand_replaces_status_text_while_elapsed_stays_in_the_menu() {
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
}
