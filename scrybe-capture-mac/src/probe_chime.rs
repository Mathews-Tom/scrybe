// Copyright 2026 Mathews Tom
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//     https://www.apache.org/licenses/LICENSE-2.0

//! Calibration chime for `scrybe doctor --check-tap`.
//!
//! A Core Audio Tap reads the digital pre-mix stream, not the
//! acoustic output — so a quiet signal still lands at the tap as a
//! real, nonzero sample, while a TCC-denied tap reads exact digital
//! zeros no matter what is playing. Generating and playing a short
//! calibration chime in-process (via `cpal`'s output stream, already
//! a workspace dependency for `scrybe-capture-mic`'s input side)
//! replaces the previous `afplay /System/Library/Sounds/Submarine.aiff`
//! fixture, which was loud on every doctor run and depended on a
//! specific system sound file existing on disk.
//!
//! The original design played a continuous single-frequency tone at
//! -62 dBFS (amplitude 0.0008), aiming for genuinely inaudible. On
//! real hardware that amplitude reads as exact digital zero at the
//! tap: this Mac's output path applies a hard gate somewhere between
//! -62 dBFS (0.0008, gated to zero) and -60 dBFS (0.001, passes
//! through undiminished) — likely a hiss-suppression noise gate
//! rather than a smooth attenuation curve, since every measured level
//! above the boundary arrived at the tap at its exact configured
//! amplitude with no attenuation. A single sustained tone loud enough
//! to clear that gate is also an unpleasant, un-musical drone on
//! every `doctor --check-tap` run. This module instead plays a short
//! three-note ascending chime (E5–G5–C6, doorbell-like) at a safe
//! margin above the empirical gate, looping for the probe window so
//! timing jitter cannot cause a miss.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};

use crate::error::MacCaptureError;
use scrybe_core::error::CaptureError;

/// Peak amplitude of the chime.
///
/// Empirically confirmed (this Mac, built-in output) to arrive at
/// the tap at its exact configured level with no attenuation —
/// comfortably above the hard gate found between 0.0008 (silently
/// zeroed) and 0.001 (passes through).
pub const PROBE_CHIME_AMPLITUDE: f32 = 0.005;

/// Peak threshold separating a granted tap from a TCC-denied one.
///
/// Below `PROBE_CHIME_AMPLITUDE` with margin above the empirical gate
/// boundary observed near 0.0008–0.001, and comfortably above the
/// noise floor of a genuinely silent (all-zero) tap.
pub const PROBE_CHIME_PASS_THRESHOLD: f32 = 0.002;

/// One note in the chime sequence: `(frequency_hz, duration_ms)`.
/// `frequency_hz == 0.0` is a silent gap.
const CHIME_NOTES: &[(f32, f32)] = &[
    (659.25, 110.0), // E5
    (0.0, 20.0),
    (783.99, 110.0), // G5
    (0.0, 20.0),
    (1046.50, 160.0), // C6, held slightly longer
    (0.0, 400.0),     // gap before the pattern loops
];

/// Fade-in/fade-out applied at each note's edges so the chime does
/// not click at note boundaries.
const NOTE_FADE_MS: f32 = 8.0;

/// Build one period of the chime as a mono `f32` buffer at
/// `sample_rate`. Looping this buffer (see [`fill_from_chime`])
/// produces a repeating chime that plays cleanly regardless of when
/// within the period a capture window starts.
#[allow(
    clippy::cast_precision_loss,
    clippy::cast_sign_loss,
    clippy::cast_possible_truncation
)]
fn build_chime_buffer(sample_rate: u32) -> Vec<f32> {
    let sample_rate = sample_rate.max(1);
    let mut buffer = Vec::new();
    for &(freq_hz, duration_ms) in CHIME_NOTES {
        let note_frames = ((duration_ms / 1000.0) * sample_rate as f32).round() as usize;
        let fade_frames =
            (((NOTE_FADE_MS / 1000.0) * sample_rate as f32).round() as usize).min(note_frames / 2);
        if freq_hz <= 0.0 {
            buffer.resize(buffer.len() + note_frames, 0.0);
            continue;
        }
        let cycle_step = freq_hz / sample_rate as f32;
        let mut cycle = 0.0_f32;
        for i in 0..note_frames {
            let envelope = if fade_frames == 0 {
                1.0
            } else if i < fade_frames {
                i as f32 / fade_frames as f32
            } else if i >= note_frames - fade_frames {
                (note_frames - i) as f32 / fade_frames as f32
            } else {
                1.0
            };
            let sample = PROBE_CHIME_AMPLITUDE * envelope * (cycle * std::f32::consts::TAU).sin();
            buffer.push(sample);
            cycle += cycle_step;
            if cycle >= 1.0 {
                cycle -= 1.0;
            }
        }
    }
    buffer
}

/// Copy from the precomputed `chime` buffer into `buffer`, looping
/// back to the start and duplicating the mono sample across every
/// channel. `read_pos` carries the loop position across successive
/// stream callbacks.
fn fill_from_chime(buffer: &mut [f32], channels: u16, chime: &[f32], read_pos: &mut usize) {
    if chime.is_empty() {
        buffer.fill(0.0);
        return;
    }
    let channels = usize::from(channels.max(1));
    for frame in buffer.chunks_mut(channels) {
        let sample = chime[*read_pos];
        for s in frame {
            *s = sample;
        }
        *read_pos += 1;
        if *read_pos >= chime.len() {
            *read_pos = 0;
        }
    }
}

/// Convert a filled `f32` scratch buffer into the target integer
/// sample format in place, using the natural full-scale mapping for
/// that format.
fn convert_from_f32_scratch<T>(scratch: &[f32], dst: &mut [T])
where
    T: cpal::Sample + cpal::FromSample<f32>,
{
    for (d, s) in dst.iter_mut().zip(scratch.iter()) {
        *d = <T as cpal::Sample>::from_sample(*s);
    }
}

fn probe_chime_error(message: impl Into<String>) -> CaptureError {
    MacCaptureError::ProbeChimePlayback(Box::new(std::io::Error::other(message.into()))).into()
}

fn probe_chime_error_source(err: impl std::error::Error + Send + Sync + 'static) -> CaptureError {
    MacCaptureError::ProbeChimePlayback(Box::new(err)).into()
}

/// Play the calibration chime for `duration`, looping it as needed.
///
/// Blocks the calling thread until playback completes on the default
/// output device. Intended to run on a dedicated blocking thread
/// (e.g. via `tokio::task::spawn_blocking`) concurrently with a
/// tap-frame capture loop of the same duration.
///
/// # Errors
///
/// Returns `CaptureError::Platform` (wrapping
/// `MacCaptureError::ProbeChimePlayback`) if no default output device
/// exists, the device rejects the stream configuration, or the
/// output stream reports an error during playback.
pub fn play_probe_chime(duration: Duration) -> Result<(), CaptureError> {
    let host = cpal::default_host();
    let device = host
        .default_output_device()
        .ok_or_else(|| probe_chime_error("no default output device"))?;
    let supported_config = device
        .default_output_config()
        .map_err(probe_chime_error_source)?;
    let sample_format = supported_config.sample_format();
    let stream_config: cpal::StreamConfig = supported_config.into();
    let channels = stream_config.channels;
    let chime = build_chime_buffer(stream_config.sample_rate.0);

    let stream_failed = Arc::new(AtomicBool::new(false));
    let error_flag = Arc::clone(&stream_failed);
    let error_callback = move |err: cpal::StreamError| {
        tracing::warn!("probe chime output stream error: {err}");
        error_flag.store(true, Ordering::Relaxed);
    };

    let mut read_pos = 0_usize;
    let stream = match sample_format {
        cpal::SampleFormat::F32 => device.build_output_stream(
            &stream_config,
            move |data: &mut [f32], _| fill_from_chime(data, channels, &chime, &mut read_pos),
            error_callback,
            None,
        ),
        cpal::SampleFormat::I16 => build_integer_output_stream::<i16>(
            &device,
            &stream_config,
            channels,
            chime,
            error_callback,
        ),
        cpal::SampleFormat::U16 => build_integer_output_stream::<u16>(
            &device,
            &stream_config,
            channels,
            chime,
            error_callback,
        ),
        other => {
            return Err(probe_chime_error(format!(
                "unsupported output sample format: {other:?}"
            )))
        }
    }
    .map_err(probe_chime_error_source)?;

    stream.play().map_err(probe_chime_error_source)?;
    std::thread::sleep(duration);
    drop(stream);

    if stream_failed.load(Ordering::Relaxed) {
        return Err(probe_chime_error(
            "output stream reported an error during playback",
        ));
    }
    Ok(())
}

/// Build an output stream for an integer `cpal` sample format by
/// filling an `f32` scratch buffer with the chime and converting into
/// the target type per callback.
fn build_integer_output_stream<T>(
    device: &cpal::Device,
    stream_config: &cpal::StreamConfig,
    channels: u16,
    chime: Vec<f32>,
    error_callback: impl FnMut(cpal::StreamError) + Send + 'static,
) -> Result<cpal::Stream, cpal::BuildStreamError>
where
    T: cpal::SizedSample + cpal::FromSample<f32>,
{
    let mut scratch: Vec<f32> = Vec::new();
    let mut read_pos = 0_usize;
    device.build_output_stream(
        stream_config,
        move |data: &mut [T], _| {
            scratch.clear();
            scratch.resize(data.len(), 0.0);
            fill_from_chime(&mut scratch, channels, &chime, &mut read_pos);
            convert_from_f32_scratch(&scratch, data);
        },
        error_callback,
        None,
    )
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::float_cmp,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss
)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    fn peak(samples: &[f32]) -> f32 {
        samples.iter().fold(0.0_f32, |acc, s| acc.max(s.abs()))
    }

    #[test]
    fn test_build_chime_buffer_peak_matches_configured_amplitude() {
        let chime = build_chime_buffer(48_000);

        let observed_peak = peak(&chime);
        assert!(
            (observed_peak - PROBE_CHIME_AMPLITUDE).abs() < 0.0001,
            "peak {observed_peak} does not match configured amplitude {PROBE_CHIME_AMPLITUDE}"
        );
    }

    #[test]
    fn test_build_chime_buffer_peak_exceeds_pass_threshold() {
        let chime = build_chime_buffer(48_000);

        assert!(peak(&chime) > PROBE_CHIME_PASS_THRESHOLD);
    }

    #[test]
    fn test_build_chime_buffer_contains_silent_gaps_between_notes() {
        let chime = build_chime_buffer(48_000);

        // The 20ms gap after the first note (E5, 110ms in) must be
        // silent — proves notes and gaps land in the right place.
        let gap_start_frame = ((0.110 + 0.004) * 48_000.0) as usize; // past the fade-out
        assert_eq!(chime[gap_start_frame], 0.0);
    }

    #[test]
    fn test_build_chime_buffer_note_onset_is_not_a_click() {
        let chime = build_chime_buffer(48_000);

        // The very first sample must start at (or near) zero thanks
        // to the fade-in envelope, not jump straight to full
        // amplitude — a discontinuity would be an audible click.
        assert!(chime[0].abs() < 0.0001);
    }

    #[test]
    fn test_build_chime_buffer_zero_sample_rate_does_not_panic() {
        // sample_rate=0 clamps to 1; at 1 sample/sec every note in
        // `CHIME_NOTES` rounds to 0 frames, so an empty buffer is the
        // correct (not buggy) result. The invariant under test is
        // that computing it does not divide by zero or panic.
        let chime = build_chime_buffer(0);

        assert!(chime.is_empty());
    }

    #[test]
    fn test_fill_from_chime_loops_back_to_start() {
        let chime = vec![0.1_f32, 0.2, 0.3];
        let mut read_pos = 2; // one sample from the end.
        let mut buffer = vec![0.0_f32; 3];

        fill_from_chime(&mut buffer, 1, &chime, &mut read_pos);

        assert_eq!(buffer, vec![0.3, 0.1, 0.2]);
        assert_eq!(read_pos, 2);
    }

    #[test]
    fn test_fill_from_chime_duplicates_mono_sample_across_channels() {
        let chime = vec![0.1_f32, 0.2, 0.3, 0.4];
        let mut read_pos = 0;
        let mut buffer = vec![0.0_f32; 8]; // 4 stereo frames.

        fill_from_chime(&mut buffer, 2, &chime, &mut read_pos);

        for frame in buffer.chunks(2) {
            assert_eq!(frame[0], frame[1]);
        }
    }

    #[test]
    fn test_fill_from_chime_empty_chime_fills_silence() {
        let chime: Vec<f32> = Vec::new();
        let mut read_pos = 0;
        let mut buffer = vec![1.0_f32; 4];

        fill_from_chime(&mut buffer, 1, &chime, &mut read_pos);

        assert_eq!(buffer, vec![0.0, 0.0, 0.0, 0.0]);
    }
}
