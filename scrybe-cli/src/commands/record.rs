// Copyright 2026 Mathews Tom
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//     https://www.apache.org/licenses/LICENSE-2.0

//! `scrybe record` — start a session.
//!
//! v0.1 wires the macOS adapter (`scrybe-capture-mac`) and a
//! synthetic-frame source for environments where Core Audio Taps is
//! not built in. STT and LLM providers default to local Whisper and
//! local Ollama respectively (`docs/system-design.md` §6); when those
//! features are not built into `scrybe-cli`, transcription returns
//! `SttError::ModelNotLoaded` and the user gets an actionable error.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use async_trait::async_trait;
use chrono::Utc;
use clap::{Args as ClapArgs, ValueEnum};
use futures::stream::{self, Stream, StreamExt};
use scrybe_core::context::MeetingContext;
use scrybe_core::diarize::Diarizer;
use scrybe_core::error::{CoreError, LlmError, SttError};
use scrybe_core::hooks::{Hook, LifecycleEvent};
use scrybe_core::pipeline::chunker::ChunkerConfig;
use scrybe_core::pipeline::vad::EnergyVad;
use scrybe_core::providers::{LlmProvider, SttProvider};
use scrybe_core::session::{run as run_session, SessionInputs};
use scrybe_core::types::{
    AttributedChunk, AudioChunk, AudioFrame, ConsentMode, FrameSource, SessionId, SpeakerLabel,
    TranscriptChunk,
};

use crate::prompter::TtyPrompter;
use crate::runtime::{expand_root, load_or_default_config};

#[derive(ClapArgs, Debug)]
pub struct Args {
    /// Session title for the folder name and notes.
    #[arg(long)]
    pub title: Option<String>,

    /// Override the storage root from config.
    #[arg(long)]
    pub root: Option<PathBuf>,

    /// Skip the consent prompt — for headless smoke tests on a
    /// developer workstation. Equivalent to setting the
    /// `SCRYBE_CONSENT_AUTO_ACCEPT=1` environment variable; either
    /// alone is sufficient because both are interactive overrides
    /// the user can audit in the surrounding shell history.
    #[arg(long, default_value_t = false)]
    pub yes: bool,

    /// Consent mode — `quick` is the v0.1 default; `notify`/`announce`
    /// downgrade until v0.2.
    #[arg(long, value_enum, default_value_t = ConsentModeArg::Quick)]
    pub consent: ConsentModeArg,

    /// Synthetic-source duration in seconds. v0.1 records from a
    /// deterministic in-process generator (sine sweep) so the full
    /// pipeline is exercisable without macOS hardware. Real Core Audio
    /// Taps capture lands behind a hardware-validation pass —
    /// `system-design.md` §11 Tier 3.
    #[arg(long, default_value_t = 5)]
    pub synthetic_secs: u64,
}

#[derive(Copy, Clone, Debug, ValueEnum)]
pub enum ConsentModeArg {
    Quick,
    Notify,
    Announce,
}

impl From<ConsentModeArg> for ConsentMode {
    fn from(value: ConsentModeArg) -> Self {
        match value {
            ConsentModeArg::Quick => Self::Quick,
            ConsentModeArg::Notify => Self::Notify,
            ConsentModeArg::Announce => Self::Announce,
        }
    }
}

pub async fn run(args: Args) -> Result<()> {
    let cfg = load_or_default_config()?;
    let root = match &args.root {
        Some(p) => expand_root(p),
        None => expand_root(&cfg.storage.root),
    };
    tokio::fs::create_dir_all(&root)
        .await
        .with_context(|| format!("creating storage root {}", root.display()))?;

    let auto_accept = args.yes || std::env::var("SCRYBE_CONSENT_AUTO_ACCEPT").as_deref() == Ok("1");
    let prompter = TtyPrompter::new(auto_accept);

    let stt = StubLocalStt::new();
    let llm = StubLocalLlm::new();
    let diarizer = BinaryChannelDiarizer;
    let hooks: Vec<Box<dyn Hook>> = Vec::new();

    let id = SessionId::new();
    let user = std::env::var("USER").unwrap_or_else(|_| "scrybe-user".into());
    let started_at = Utc::now();

    let stream = synthetic_capture_stream(args.synthetic_secs);

    let outputs = run_session(
        SessionInputs {
            id,
            started_at,
            root: root.clone(),
            title: args.title.clone(),
            user,
            consent_mode: args.consent.into(),
            context: MeetingContext {
                title: args.title,
                ..MeetingContext::default()
            },
            mic_vad: EnergyVad::default(),
            system_vad: None,
            stt: &stt,
            llm: &llm,
            diarizer: &diarizer,
            prompter: &prompter,
            hooks: &hooks,
            chunker_config: ChunkerConfig {
                max_chunk: Duration::from_secs(30),
                min_speech_before_silence_split: Duration::from_secs(5),
                silence_split_after: Duration::from_secs(5),
            },
        },
        stream,
    )
    .await
    .context("running session")?;

    println!(
        "scrybe record: session {} written to {}",
        id,
        outputs.folder.display()
    );
    println!("  transcript: {}", outputs.transcript_path.display());
    println!("  notes:      {}", outputs.notes_path.display());
    println!("  meta:       {}", outputs.meta_path.display());
    if outputs.audio_path.exists() {
        println!("  audio:      {}", outputs.audio_path.display());
    }
    Ok(())
}

/// Synthetic in-process capture source.
///
/// Generates 16-kHz mono frames of a 440 Hz sine wave for `seconds`
/// seconds and emits silence after to drive the silence-after-speech
/// chunker boundary at session end.
#[allow(clippy::cast_precision_loss)]
fn synthetic_capture_stream(
    seconds: u64,
) -> impl Stream<Item = Result<AudioFrame, scrybe_core::error::CaptureError>> + Send + Unpin {
    const SAMPLE_RATE: u32 = 16_000;
    const FRAME_SAMPLES: usize = 1_600;
    let total_speech = seconds * (u64::from(SAMPLE_RATE) / FRAME_SAMPLES as u64);
    let total_silence = (u64::from(SAMPLE_RATE) / FRAME_SAMPLES as u64) * 6;
    let total = total_speech + total_silence;

    Box::pin(stream::iter(0..total).map(move |i| {
        let speech = i < total_speech;
        let samples: Vec<f32> = (0..FRAME_SAMPLES)
            .map(|n| {
                if speech {
                    let t = (i * FRAME_SAMPLES as u64 + n as u64) as f32 / SAMPLE_RATE as f32;
                    (t * 440.0 * std::f32::consts::TAU).sin()
                } else {
                    0.0
                }
            })
            .collect();
        let timestamp_ns = (i * FRAME_SAMPLES as u64 * 1_000_000_000) / u64::from(SAMPLE_RATE);
        Ok(AudioFrame {
            samples: Arc::from(samples),
            channels: 1,
            sample_rate: SAMPLE_RATE,
            timestamp_ns,
            source: FrameSource::Mic,
        })
    }))
}

/// CLI-local stub STT provider. Emits a deterministic line so the
/// rest of the pipeline (transcript append, LLM prompt rendering,
/// notes write) is exercisable without a real Whisper model. Using a
/// real model is wired via `--features whisper-local` and the
/// `WhisperLocalProvider` in `scrybe-core::providers::whisper_local`.
struct StubLocalStt;

impl StubLocalStt {
    const fn new() -> Self {
        Self
    }
}

#[async_trait]
impl SttProvider for StubLocalStt {
    async fn transcribe(&self, chunk: AudioChunk) -> Result<TranscriptChunk, SttError> {
        let speech = chunk.samples.iter().any(|s| s.abs() > 0.01);
        let text = if speech {
            "[synthetic speech chunk; build with --features whisper-local for real transcription]"
        } else {
            "[silence]"
        };
        Ok(TranscriptChunk {
            text: text.to_string(),
            source: chunk.source,
            start_ms: u64::try_from(chunk.start.as_millis()).unwrap_or(0),
            duration_ms: u64::try_from(chunk.duration.as_millis()).unwrap_or(0),
            language: None,
        })
    }

    fn name(&self) -> &'static str {
        "stub-local-stt"
    }
}

/// CLI-local stub LLM provider. Returns a fixed structured-notes body
/// so `notes.md` is well-formed. Real LLM access is via Ollama or
/// `openai-compat`; both ship in v0.2 (`docs/system-design.md` §4.3).
struct StubLocalLlm;

impl StubLocalLlm {
    const fn new() -> Self {
        Self
    }
}

#[async_trait]
impl LlmProvider for StubLocalLlm {
    async fn complete(&self, _prompt: &str) -> Result<String, LlmError> {
        Ok(
            "## TL;DR\nSynthetic stub session. Build with a configured LLM \
            provider to generate real notes.\n## Action items\n- (none)\n\
            ## Decisions\n- (none)\n## Follow-ups\n- (none)\n"
                .to_string(),
        )
    }

    fn name(&self) -> &'static str {
        "stub-local-llm"
    }
}

/// CLI-local diarizer using the binary-channel heuristic. Mic-only
/// sessions yield `Me:` everywhere.
struct BinaryChannelDiarizer;

#[async_trait]
impl Diarizer for BinaryChannelDiarizer {
    async fn diarize(
        &self,
        mic: &[TranscriptChunk],
        sys: &[TranscriptChunk],
        _ctx: &MeetingContext,
    ) -> Result<Vec<AttributedChunk>, CoreError> {
        let mut out = Vec::with_capacity(mic.len() + sys.len());
        for chunk in mic {
            out.push(AttributedChunk {
                chunk: chunk.clone(),
                speaker: SpeakerLabel::Me,
            });
        }
        for chunk in sys {
            out.push(AttributedChunk {
                chunk: chunk.clone(),
                speaker: SpeakerLabel::Them,
            });
        }
        Ok(out)
    }

    fn name(&self) -> &'static str {
        "binary-channel"
    }
}

#[allow(dead_code)]
const fn _ensure_event_dispatch_compiles(_event: &LifecycleEvent) {}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    #[test]
    fn test_consent_mode_arg_quick_maps_to_consent_mode_quick() {
        let mode: ConsentMode = ConsentModeArg::Quick.into();

        assert_eq!(mode, ConsentMode::Quick);
    }

    #[test]
    fn test_consent_mode_arg_notify_maps_to_consent_mode_notify() {
        let mode: ConsentMode = ConsentModeArg::Notify.into();

        assert_eq!(mode, ConsentMode::Notify);
    }

    #[test]
    fn test_consent_mode_arg_announce_maps_to_consent_mode_announce() {
        let mode: ConsentMode = ConsentModeArg::Announce.into();

        assert_eq!(mode, ConsentMode::Announce);
    }

    #[tokio::test]
    async fn test_synthetic_capture_stream_emits_speech_then_silence_frames() {
        let stream = synthetic_capture_stream(1);
        let frames: Vec<_> = stream.collect().await;

        assert!(!frames.is_empty());
        let speech_count = frames
            .iter()
            .filter(|f| {
                f.as_ref()
                    .map(|frame| frame.samples.iter().any(|s| s.abs() > 0.01))
                    .unwrap_or(false)
            })
            .count();
        assert!(
            speech_count >= 5,
            "expected speech frames; got {speech_count}"
        );
    }

    #[tokio::test]
    async fn test_stub_local_stt_returns_speech_marker_for_non_silence_chunk() {
        let pcm: Arc<[f32]> = Arc::from(vec![0.5_f32; 16_000]);
        let chunk = AudioChunk {
            samples: pcm,
            source: FrameSource::Mic,
            start: Duration::ZERO,
            duration: Duration::from_secs(1),
        };

        let result = StubLocalStt::new().transcribe(chunk).await.unwrap();

        assert!(result.text.contains("synthetic speech"));
    }

    #[tokio::test]
    async fn test_stub_local_stt_returns_silence_marker_for_zero_buffer() {
        let pcm: Arc<[f32]> = Arc::from(vec![0.0_f32; 16_000]);
        let chunk = AudioChunk {
            samples: pcm,
            source: FrameSource::Mic,
            start: Duration::ZERO,
            duration: Duration::from_secs(1),
        };

        let result = StubLocalStt::new().transcribe(chunk).await.unwrap();

        assert_eq!(result.text, "[silence]");
    }
}
