// Copyright 2026 Mathews Tom
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//     https://www.apache.org/licenses/LICENSE-2.0

//! Voice-activity-detection seam.
//!
//! The pipeline depends on a trait, not on a concrete VAD implementation,
//! so the chunker is testable without an ONNX runtime. The reference
//! `EnergyVad` is a deterministic RMS-threshold detector with no
//! dependencies — adequate for synthetic test fixtures and
//! cheap-to-evaluate. The production `SileroVad` (Silero v5 via
//! `voice_activity_detector` 0.2) lives behind the `vad-silero` feature
//! and is wired in by the CLI; see `docs/system-design.md` §5 VAD strategy.

use crate::types::AudioFrame;

/// One frame's voice-activity decision.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum VadDecision {
    Speech,
    Silence,
}

/// Voice-activity detector. Stateful by design: many implementations
/// maintain a sliding window or hangover counter to smooth the
/// per-frame decision.
pub trait Vad: Send {
    /// Classify a single frame as speech or silence.
    ///
    /// Implementations MAY consume any sample rate the pipeline supplies;
    /// callers are responsible for resampling if the implementation
    /// requires a fixed rate (Silero requires 16 kHz; energy-threshold
    /// detectors are rate-agnostic).
    fn decide(&mut self, frame: &AudioFrame) -> VadDecision;
}

/// Deterministic RMS-energy VAD used by tests and as a fallback when the
/// `vad-silero` feature is not enabled. Produces a `Speech` decision when
/// the frame's RMS exceeds `threshold`. No allocation, no I/O.
pub struct EnergyVad {
    threshold: f32,
}

impl EnergyVad {
    /// Construct with a linear-amplitude threshold. Sensible test values
    /// are in the range `0.001..=0.05`; production silence is generally
    /// well below `0.001` and natural speech well above `0.01`.
    #[must_use]
    pub const fn new(threshold: f32) -> Self {
        Self { threshold }
    }
}

impl Default for EnergyVad {
    /// Default threshold tuned for synthetic test fixtures: anything
    /// above `0.01` linear amplitude counts as speech.
    fn default() -> Self {
        Self::new(0.01)
    }
}

impl Vad for EnergyVad {
    #[allow(clippy::cast_precision_loss)]
    fn decide(&mut self, frame: &AudioFrame) -> VadDecision {
        if frame.samples.is_empty() {
            return VadDecision::Silence;
        }
        let sum_sq: f32 = frame.samples.iter().map(|s| s * s).sum();
        let mean_sq = sum_sq / frame.samples.len() as f32;
        let rms = mean_sq.sqrt();
        if rms >= self.threshold {
            VadDecision::Speech
        } else {
            VadDecision::Silence
        }
    }
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::cast_precision_loss
)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::types::FrameSource;
    use pretty_assertions::assert_eq;

    fn frame(samples: Vec<f32>) -> AudioFrame {
        AudioFrame {
            samples: Arc::from(samples),
            channels: 1,
            sample_rate: 16_000,
            timestamp_ns: 0,
            source: FrameSource::Mic,
        }
    }

    #[test]
    fn test_energy_vad_classifies_full_amplitude_sine_as_speech() {
        let mut vad = EnergyVad::default();
        let samples: Vec<f32> = (0..160).map(|n| (n as f32 * 0.1).sin()).collect();

        let decision = vad.decide(&frame(samples));

        assert_eq!(decision, VadDecision::Speech);
    }

    #[test]
    fn test_energy_vad_classifies_zero_buffer_as_silence() {
        let mut vad = EnergyVad::default();
        let samples = vec![0.0_f32; 160];

        let decision = vad.decide(&frame(samples));

        assert_eq!(decision, VadDecision::Silence);
    }

    #[test]
    fn test_energy_vad_classifies_below_threshold_as_silence() {
        let mut vad = EnergyVad::new(0.05);
        let samples = vec![0.001_f32; 160];

        let decision = vad.decide(&frame(samples));

        assert_eq!(decision, VadDecision::Silence);
    }

    #[test]
    fn test_energy_vad_handles_empty_frame_as_silence() {
        let mut vad = EnergyVad::default();

        let decision = vad.decide(&frame(Vec::new()));

        assert_eq!(decision, VadDecision::Silence);
    }

    #[test]
    fn test_energy_vad_threshold_at_exactly_rms_classifies_as_speech() {
        let mut vad = EnergyVad::new(0.5);
        let samples = vec![0.5_f32; 160];

        let decision = vad.decide(&frame(samples));

        assert_eq!(decision, VadDecision::Speech);
    }
}
