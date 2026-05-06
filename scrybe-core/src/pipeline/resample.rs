// Copyright 2026 Mathews Tom
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//     https://www.apache.org/licenses/LICENSE-2.0

//! Sample-rate conversion to 16 kHz mono. Whisper's native rate is
//! 16 kHz; we resample once at the chunker boundary so downstream stages
//! are rate-agnostic.
//!
//! Two-mode resampler with no external dependency:
//!
//! - **Upsampling / equal-rate** (`step ≤ 1`): linear interpolation.
//!   Fine for the harmonic content STT cares about.
//! - **Downsampling** (`step > 1`): box-averaging over the source-time
//!   window each output sample covers. Acts as a 1st-order low-pass
//!   that decimates anti-aliased.
//!
//! Box averaging matters because every real Core Audio Tap recording
//! goes through a 3:1 downsample (48 kHz → 16 kHz). Naive linear
//! interpolation in that direction folds 8–24 kHz energy into the
//! 0–8 kHz audible band as aliasing artifacts — Whisper interpreted
//! that as `*Gibberish*`, `[fast forward]`, and similar non-speech
//! events on the Them channel during v1.1 system-audio testing. A
//! cleaner solution is a polyphase FIR via `rubato` / `samplerate`,
//! but those crates pull non-trivial dependency trees. Box averaging
//! is correct enough for STT, ~30 lines, and zero-dep. Adapter crates
//! that need broadcast-quality resampling may bring their own.

use crate::error::PipelineError;

#[derive(Debug, Eq, PartialEq)]
pub enum ResampleError {
    Unsupported(u32),
}

impl From<ResampleError> for PipelineError {
    fn from(value: ResampleError) -> Self {
        match value {
            ResampleError::Unsupported(rate) => Self::Resample { source_rate: rate },
        }
    }
}

/// Resample a mono buffer from `source_rate` to `target_rate`.
///
/// - `target_rate >= source_rate` (upsample / equal-rate): linear
///   interpolation between adjacent input samples.
/// - `target_rate < source_rate` (downsample): box-averaging over the
///   source-time window each output sample covers, which acts as an
///   anti-aliasing low-pass filter that prevents 8–24 kHz energy from
///   folding into the 0–8 kHz band on a 48 kHz → 16 kHz decimation.
///
/// The input is borrowed and not mutated.
///
/// # Errors
///
/// Returns `ResampleError::Unsupported` when `source_rate` or
/// `target_rate` is zero. Equal rates short-circuit to a clone.
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_precision_loss,
    clippy::cast_sign_loss,
    clippy::suboptimal_flops
)]
pub fn resample_linear(
    samples: &[f32],
    source_rate: u32,
    target_rate: u32,
) -> Result<Vec<f32>, ResampleError> {
    if source_rate == 0 {
        return Err(ResampleError::Unsupported(source_rate));
    }
    if target_rate == 0 {
        return Err(ResampleError::Unsupported(target_rate));
    }
    if source_rate == target_rate || samples.is_empty() {
        return Ok(samples.to_vec());
    }

    let src_len = samples.len();
    let ratio = f64::from(target_rate) / f64::from(source_rate);
    let out_len_f = (src_len as f64) * ratio;
    let out_len = out_len_f.round() as usize;
    if out_len == 0 {
        return Ok(Vec::new());
    }

    let step = f64::from(source_rate) / f64::from(target_rate);
    let mut out = Vec::with_capacity(out_len);

    if step > 1.0 {
        // Downsampling: average all input samples whose timestamps
        // fall inside the source-time window covered by output sample
        // `i`. The window spans [i*step, (i+1)*step) and is treated as
        // closed on the lower bound, open on the upper. This is the
        // anti-aliasing path; without it, 48 kHz → 16 kHz decimation
        // produces gibberish output for STT.
        for i in 0..out_len {
            let win_start_f = (i as f64) * step;
            let win_end_f = ((i + 1) as f64) * step;
            let win_start = (win_start_f.floor() as usize).min(src_len);
            // `ceil` so partially-covered samples at the upper bound
            // contribute to the average. `min(src_len)` clamps the
            // tail at the buffer end.
            let win_end = (win_end_f.ceil() as usize).min(src_len);
            if win_start >= win_end {
                // Empty window — fall back to nearest-neighbour from
                // the buffer end so the output length still matches
                // `out_len_f.round()`.
                out.push(samples.last().copied().unwrap_or(0.0));
                continue;
            }
            let mut sum = 0.0_f64;
            for s in &samples[win_start..win_end] {
                sum += f64::from(*s);
            }
            let avg = sum / ((win_end - win_start) as f64);
            out.push(avg as f32);
        }
    } else {
        // Upsampling or near-equal: linear interpolation between
        // adjacent input samples is correct (no aliasing direction to
        // guard against).
        for i in 0..out_len {
            let src_pos = (i as f64) * step;
            let lo = src_pos.floor() as usize;
            let hi = lo + 1;
            let frac = src_pos - (lo as f64);
            if hi >= src_len {
                out.push(samples[src_len - 1]);
            } else {
                let a = f64::from(samples[lo]);
                let b = f64::from(samples[hi]);
                let mixed = (a * (1.0 - frac) + b * frac) as f32;
                out.push(mixed);
            }
        }
    }
    Ok(out)
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_possible_wrap,
    clippy::cast_sign_loss,
    clippy::float_cmp,
    clippy::suboptimal_flops
)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    fn rms(samples: &[f32]) -> f32 {
        if samples.is_empty() {
            return 0.0;
        }
        let s: f32 = samples.iter().map(|x| x * x).sum();
        (s / samples.len() as f32).sqrt()
    }

    fn sine(rate: u32, freq: f32, secs: f32) -> Vec<f32> {
        let n = (rate as f32 * secs) as usize;
        (0..n)
            .map(|i| {
                let t = i as f32 / rate as f32;
                (t * freq * std::f32::consts::TAU).sin()
            })
            .collect()
    }

    #[test]
    fn test_resample_linear_equal_rates_returns_input_unchanged() {
        let input = vec![0.0_f32, 0.5, -0.5, 0.25];

        let out = resample_linear(&input, 16_000, 16_000).unwrap();

        assert_eq!(out, input);
    }

    #[test]
    fn test_resample_linear_48k_to_16k_preserves_1khz_sine_amplitude() {
        let input = sine(48_000, 1_000.0, 0.5);

        let out = resample_linear(&input, 48_000, 16_000).unwrap();

        let in_rms = rms(&input);
        let out_rms = rms(&out);
        let ratio_db = 20.0 * (out_rms / in_rms).log10();
        assert!(
            ratio_db.abs() < 0.5,
            "1 kHz sine RMS deviation {ratio_db:.3} dB exceeds ±0.5 dB"
        );
        assert!((out.len() as i64 - 8_000).abs() <= 1);
    }

    #[test]
    fn test_resample_linear_44_1k_to_16k_preserves_500hz_sine_amplitude() {
        let input = sine(44_100, 500.0, 0.5);

        let out = resample_linear(&input, 44_100, 16_000).unwrap();

        let in_rms = rms(&input);
        let out_rms = rms(&out);
        let ratio_db = 20.0 * (out_rms / in_rms).log10();
        assert!(
            ratio_db.abs() < 0.5,
            "500 Hz sine RMS deviation {ratio_db:.3} dB exceeds ±0.5 dB"
        );
    }

    #[test]
    fn test_resample_linear_empty_input_returns_empty_output() {
        let out = resample_linear(&[], 48_000, 16_000).unwrap();

        assert!(out.is_empty());
    }

    #[test]
    fn test_resample_linear_zero_source_rate_is_unsupported() {
        let err = resample_linear(&[0.0], 0, 16_000).unwrap_err();

        assert_eq!(err, ResampleError::Unsupported(0));
    }

    #[test]
    fn test_resample_linear_zero_target_rate_is_unsupported() {
        let err = resample_linear(&[0.0], 48_000, 0).unwrap_err();

        assert_eq!(err, ResampleError::Unsupported(0));
    }

    #[test]
    fn test_resample_linear_upsample_then_downsample_round_trips_within_tolerance() {
        let original = sine(16_000, 440.0, 0.25);
        let up = resample_linear(&original, 16_000, 48_000).unwrap();

        let down = resample_linear(&up, 48_000, 16_000).unwrap();

        let ratio = rms(&down) / rms(&original);
        assert!(
            (ratio - 1.0).abs() < 0.05,
            "round-trip RMS ratio {ratio:.4} differs from unity by more than 5%"
        );
    }

    #[test]
    fn test_resample_linear_48k_to_16k_attenuates_above_nyquist_signal() {
        // Regression for the v1.1 STT-gibberish bug: a 12 kHz tone in
        // 48 kHz input is above the 8 kHz Nyquist of 16 kHz output.
        // Naive linear interpolation aliases this to a 4 kHz fold-back
        // tone with near-original RMS (~0 dB), which Whisper
        // transcribes as gibberish. The 3-tap box-average path used
        // for downsampling here attenuates it by `sinc(πf/fs)` —
        // approximately -9.5 dB at 12 kHz (the theoretical max for
        // a 3-tap box at this frequency). The threshold is set to
        // -8 dB so the test still discriminates cleanly from the
        // ~0 dB linear-interp regression while staying above the
        // box-average's worst case across the 8–24 kHz aliasing band.
        let input = sine(48_000, 12_000.0, 0.5);

        let out = resample_linear(&input, 48_000, 16_000).unwrap();

        let in_rms = rms(&input);
        let out_rms = rms(&out);
        let ratio_db = 20.0 * (out_rms / in_rms).log10();
        assert!(
            ratio_db < -8.0,
            "12 kHz tone above Nyquist must be attenuated below -8 dB; \
             observed {ratio_db:.3} dB (linear interp would be near 0 dB \
             due to aliasing)"
        );
    }

    #[test]
    fn test_resample_error_promotes_to_pipeline_error_with_source_rate() {
        let err = ResampleError::Unsupported(96_000);

        let pipeline: PipelineError = err.into();

        match pipeline {
            PipelineError::Resample { source_rate } => {
                assert_eq!(source_rate, 96_000);
            }
            other => panic!("expected PipelineError::Resample, got {other:?}"),
        }
    }
}
