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
//! - **Downsampling** (`step > 1`): a Hann-windowed sinc FIR centered
//!   at each output sample, followed by decimation.
//!
//! Every real Core Audio Tap recording takes the 3:1 48 kHz → 16 kHz
//! downsample path. Naive linear interpolation folds 8–24 kHz energy
//! into the 0–8 kHz output band; the prior three-sample box average
//! only attenuated a 12 kHz tone by roughly 9.5 dB. The zero-dependency
//! FIR keeps the cutoff below the target Nyquist frequency and rejects
//! that aliasing energy without pulling a general-purpose DSP crate.

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
/// Half-width of the Hann-windowed sinc kernel, in input samples.
/// Shared by the stateless [`resample_linear`] and the stateful
/// [`StreamingResampler`] so the two cannot drift apart.
const FILTER_RADIUS: isize = 5;
/// Kernel cutoff as a fraction of the target Nyquist frequency.
const CUTOFF_SCALE: f64 = 0.9;

/// One sample of a buffer whose element 0 has absolute index `base`.
///
/// `None` for an index outside the retained window: before the first
/// input sample, or past the newest one. Both callers treat a missing
/// tap as a zero-weight tap, which is what makes the streaming path's
/// partial windows agree with the stateless path's edge clamping.
fn sample_at(base: u64, buffer: &[f32], index: isize) -> Option<f32> {
    if index < 0 {
        return None;
    }
    #[allow(clippy::cast_sign_loss)]
    let index = index as u64;
    if index < base {
        return None;
    }
    #[allow(clippy::cast_possible_truncation)]
    buffer.get((index - base) as usize).copied()
}

/// Hann-windowed sinc low-pass output at fractional input position
/// `center`, normalized by the realized tap gain.
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_precision_loss,
    clippy::suboptimal_flops
)]
fn fir_sample(center: f64, cutoff: f64, base: u64, buffer: &[f32], fallback: f32) -> f32 {
    let center_index = center.floor() as isize;
    let mut weighted_sum = 0.0_f64;
    let mut gain = 0.0_f64;

    for index in (center_index - FILTER_RADIUS)..=(center_index + FILTER_RADIUS) {
        let Some(sample) = sample_at(base, buffer, index) else {
            continue;
        };
        let distance = center - (index as f64);
        let window =
            0.5 + 0.5 * (std::f64::consts::PI * distance / (FILTER_RADIUS + 1) as f64).cos();
        let scaled_distance = 2.0 * cutoff * distance;
        let sinc = if scaled_distance.abs() < f64::EPSILON {
            1.0
        } else {
            (std::f64::consts::PI * scaled_distance).sin()
                / (std::f64::consts::PI * scaled_distance)
        };
        let coefficient = 2.0 * cutoff * sinc * window;
        weighted_sum += f64::from(sample) * coefficient;
        gain += coefficient;
    }

    if gain.abs() < f64::EPSILON {
        fallback
    } else {
        (weighted_sum / gain) as f32
    }
}

/// Linear interpolation at fractional input position `pos`, used for
/// upsampling and near-equal rates where there is no alias to reject.
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_precision_loss,
    clippy::suboptimal_flops
)]
fn linear_sample(pos: f64, base: u64, buffer: &[f32], fallback: f32) -> f32 {
    let lo = pos.floor() as isize;
    let frac = pos - (lo as f64);
    match (sample_at(base, buffer, lo), sample_at(base, buffer, lo + 1)) {
        (Some(a), Some(b)) => (f64::from(a) * (1.0 - frac) + f64::from(b) * frac) as f32,
        (Some(a), None) => a,
        _ => fallback,
    }
}

/// Resample a mono buffer from `source_rate` to `target_rate`.
///
/// - `target_rate >= source_rate` (upsample / equal-rate): linear
///   interpolation between adjacent input samples.
/// - `target_rate < source_rate` (downsample): a Hann-windowed sinc
///   low-pass FIR before decimation, which rejects out-of-band energy
///   before it can fold into the target's Nyquist band.
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
        let cutoff = 0.5 / step * CUTOFF_SCALE;
        let fallback = samples[src_len - 1];
        for i in 0..out_len {
            out.push(fir_sample((i as f64) * step, cutoff, 0, samples, fallback));
        }
    } else {
        // Upsampling or near-equal: linear interpolation between
        // adjacent input samples is correct (no aliasing direction to
        // guard against).
        let fallback = samples[src_len - 1];
        for i in 0..out_len {
            out.push(linear_sample((i as f64) * step, 0, samples, fallback));
        }
    }
    Ok(out)
}

/// Stateful mono resampler for a continuously arriving frame stream.
///
/// Calling [`resample_linear`] once per capture frame is wrong for the
/// live path: the FIR kernel needs five input samples of context on
/// both sides of every output sample. Independent per-frame calls clamp
/// that window at each frame edge — a periodic
/// distortion at the frame rate. This type keeps the kernel context
/// across pushes, so feeding a stream in arbitrary frame sizes and
/// then calling [`Self::finish`] yields exactly what
/// [`resample_linear`] would have produced over the concatenated
/// input.
///
/// It deliberately withholds output samples whose kernel window is not
/// yet fully covered by the audio received so far; those samples are
/// emitted by a later push (or by `finish`, which clamps the tail the
/// same way the stateless path does).
pub struct StreamingResampler {
    /// Input samples consumed per output sample.
    step: f64,
    /// Output samples produced per input sample.
    ratio: f64,
    /// Kernel cutoff; unused on the linear path.
    cutoff: f64,
    /// Retained input window. `buffer[0]` has absolute index `base`.
    buffer: Vec<f32>,
    base: u64,
    /// Total input samples pushed so far.
    consumed: u64,
    /// Next output-sample index to emit.
    next_out: u64,
    /// Equal rates need no conversion at all.
    passthrough: bool,
}

impl StreamingResampler {
    /// Create a resampler from `source_rate` to `target_rate`.
    ///
    /// # Errors
    ///
    /// `ResampleError::Unsupported` when either rate is zero.
    pub fn new(source_rate: u32, target_rate: u32) -> Result<Self, ResampleError> {
        if source_rate == 0 {
            return Err(ResampleError::Unsupported(source_rate));
        }
        if target_rate == 0 {
            return Err(ResampleError::Unsupported(target_rate));
        }
        let step = f64::from(source_rate) / f64::from(target_rate);
        Ok(Self {
            step,
            ratio: f64::from(target_rate) / f64::from(source_rate),
            cutoff: 0.5 / step * CUTOFF_SCALE,
            buffer: Vec::new(),
            base: 0,
            consumed: 0,
            next_out: 0,
            passthrough: source_rate == target_rate,
        })
    }

    /// Feed the next contiguous block of mono input and return every
    /// output sample whose kernel window is now fully covered.
    pub fn push(&mut self, samples: &[f32]) -> Vec<f32> {
        if self.passthrough {
            self.consumed = self.consumed.saturating_add(samples.len() as u64);
            return samples.to_vec();
        }
        self.buffer.extend_from_slice(samples);
        self.consumed = self.consumed.saturating_add(samples.len() as u64);
        let ready = self.emit(self.consumed);
        self.trim();
        ready
    }

    /// Drain the tail: emit the remaining output samples of the stream,
    /// clamping the kernel at the final input sample exactly as the
    /// stateless path does.
    pub fn finish(&mut self) -> Vec<f32> {
        if self.passthrough {
            return Vec::new();
        }
        let tail = self.emit(u64::MAX);
        self.buffer.clear();
        self.base = self.consumed;
        tail
    }

    /// Emit output samples while every kernel tap index is `< available`
    /// and the stream's total output length has not been reached.
    #[allow(
        clippy::cast_possible_truncation,
        clippy::cast_precision_loss,
        clippy::cast_sign_loss,
        clippy::cast_possible_wrap
    )]
    fn emit(&mut self, available: u64) -> Vec<f32> {
        let total_out = ((self.consumed as f64) * self.ratio).round() as u64;
        let fallback = self.buffer.last().copied().unwrap_or(0.0);
        let mut out = Vec::new();
        while self.next_out < total_out {
            let position = (self.next_out as f64) * self.step;
            let highest_tap = if self.step > 1.0 {
                position.floor() as i64 + FILTER_RADIUS as i64
            } else {
                position.floor() as i64 + 1
            };
            if highest_tap >= 0 && (highest_tap as u64) >= available {
                break;
            }
            let sample = if self.step > 1.0 {
                fir_sample(position, self.cutoff, self.base, &self.buffer, fallback)
            } else {
                linear_sample(position, self.base, &self.buffer, fallback)
            };
            out.push(sample);
            self.next_out += 1;
        }
        out
    }

    /// Drop input samples no future output sample can reference.
    #[allow(
        clippy::cast_possible_truncation,
        clippy::cast_precision_loss,
        clippy::cast_sign_loss
    )]
    fn trim(&mut self) {
        let position = (self.next_out as f64) * self.step;
        let lowest_tap = if self.step > 1.0 {
            position.floor() as i64 - FILTER_RADIUS as i64
        } else {
            position.floor() as i64
        };
        let keep_from = u64::try_from(lowest_tap).unwrap_or(0).max(self.base);
        let drop_count = usize::try_from(keep_from - self.base).unwrap_or(0);
        if drop_count == 0 {
            return;
        }
        let drop_count = drop_count.min(self.buffer.len());
        self.buffer.drain(..drop_count);
        self.base += drop_count as u64;
    }
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
        // The superseded 3-tap box average only attenuated this to
        // roughly -9.5 dB. A Hann-windowed sinc decimator must reduce
        // the fold-back energy below -20 dB.
        let input = sine(48_000, 12_000.0, 0.5);

        let out = resample_linear(&input, 48_000, 16_000).unwrap();

        let in_rms = rms(&input);
        let out_rms = rms(&out);
        let ratio_db = 20.0 * (out_rms / in_rms).log10();
        assert!(
            ratio_db < -20.0,
            "12 kHz tone above Nyquist must be attenuated below -20 dB; \
             observed {ratio_db:.3} dB"
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
