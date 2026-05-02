// Copyright 2026 Mathews Tom
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//     https://www.apache.org/licenses/LICENSE-2.0

//! `scrybe record` — start a session.
//!
//! v1.0.1 closes the v0.1 mic-only path (`.docs/development-plan.md`
//! §7.2). Two opt-in flags surface real audio capture and real
//! Whisper transcription:
//!
//! - `--source mic` consumes frames from the default input device via
//!   `scrybe-capture-mic` (cpal). Requires the binary to be built
//!   with `--features mic-capture`; absent that feature the call
//!   returns `CaptureError::PermissionDenied`.
//! - `--whisper-model <PATH>` swaps the stub STT provider for
//!   `WhisperLocalProvider` against the supplied `.bin` / `.gguf`
//!   weights. Requires the binary to be built with
//!   `--features whisper-local`; absent that feature the flag errors
//!   at start time rather than silently falling back to the stub.
//!
//! Without either flag the recorder runs the deterministic synthetic
//! pipeline (440 Hz sine + canned transcripts) so CI smoke tests stay
//! hermetic.

use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use async_trait::async_trait;
use chrono::Utc;
use clap::{Args as ClapArgs, ValueEnum};
use futures::stream::{self, Stream, StreamExt};
#[cfg(feature = "mic-capture")]
use scrybe_capture_mic::MicCapture;
#[cfg(feature = "mic-capture")]
use scrybe_core::capture::AudioCapture;
use scrybe_core::context::MeetingContext;
use scrybe_core::diarize::Diarizer;
use scrybe_core::error::{CaptureError, CoreError, LlmError, SttError};
use scrybe_core::hooks::{Hook, LifecycleEvent};
use scrybe_core::pipeline::chunker::ChunkerConfig;
use scrybe_core::pipeline::vad::EnergyVad;
#[cfg(feature = "whisper-local")]
use scrybe_core::providers::whisper_local::{WhisperLocalConfig, WhisperLocalProvider};
use scrybe_core::providers::{LlmProvider, SttProvider};
use scrybe_core::session::{run as run_session, SessionInputs};
use scrybe_core::types::{
    AttributedChunk, AudioChunk, AudioFrame, ConsentMode, FrameSource, SessionId, SpeakerLabel,
    TranscriptChunk,
};
use tokio::sync::watch;
use tokio::task::JoinHandle;

use crate::prompter::TtyPrompter;
use crate::runtime::{expand_root, load_or_default_config};

#[derive(ClapArgs, Clone, Debug)]
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

    /// Synthetic-source duration in seconds. The default `--source
    /// synthetic` records from a deterministic in-process generator
    /// (440 Hz sine sweep) so the full pipeline is exercisable
    /// without microphone hardware. Ignored when `--source mic`.
    #[arg(long, default_value_t = 5)]
    pub synthetic_secs: u64,

    /// Capture source. `synthetic` (default) plays a deterministic
    /// 440 Hz sine through the pipeline so CI smoke tests stay
    /// hermetic. `mic` opens the host's default input device via
    /// cpal — requires the binary to be built with
    /// `--features mic-capture`; absent the feature the call returns
    /// `CaptureError::PermissionDenied` at start time.
    #[arg(long, value_enum, default_value_t = CaptureSourceArg::Synthetic)]
    pub source: CaptureSourceArg,

    /// Path to a whisper.cpp model (`.bin` or `.gguf`). When set AND
    /// the binary is built with `--features whisper-local`, the STT
    /// provider becomes `WhisperLocalProvider` against these weights.
    /// Without the feature, an explicit path errors at start time
    /// rather than silently falling back to the stub. `*.partial`
    /// paths are rejected per the existing
    /// `WhisperLocalProvider::new` contract.
    #[arg(long)]
    pub whisper_model: Option<PathBuf>,

    /// Attach the desktop status-bar indicator (tray icon with a Quit
    /// menu) and register the global hotkey from `[capture] hotkey`
    /// in `config.toml`. The integrated main-thread shell driver that
    /// surfaces tray and hotkey events into this loop lands in a
    /// follow-up; this flag currently logs an advisory and otherwise
    /// runs the headless path. Without `--shell` the recorder stops on
    /// SIGINT or when the synthetic stream completes.
    #[arg(long, default_value_t = false)]
    pub shell: bool,
}

#[derive(Copy, Clone, Debug, Eq, PartialEq, ValueEnum)]
pub enum ConsentModeArg {
    Quick,
    Notify,
    Announce,
}

#[derive(Copy, Clone, Debug, Default, Eq, PartialEq, ValueEnum)]
pub enum CaptureSourceArg {
    #[default]
    Synthetic,
    Mic,
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
    let (stop_tx, stop_rx) = watch::channel(false);
    let sigint_handle = spawn_sigint_listener(stop_tx);
    let result = run_with_stop(args, stop_rx).await;
    sigint_handle.abort();
    result
}

/// Drive a session under an externally-supplied stop signal. The
/// shell driver in `scrybe-cli::shell` calls this directly, feeding
/// stop into `stop_rx` from tray and hotkey events; the public
/// `run` entry point above wraps it with a SIGINT-only stop signal.
pub async fn run_with_stop(args: Args, stop_rx: watch::Receiver<bool>) -> Result<()> {
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

    let stt = build_stt_provider(args.whisper_model.as_ref())?;
    let llm = StubLocalLlm::new();
    let diarizer = BinaryChannelDiarizer;
    let hooks: Vec<Box<dyn Hook>> = Vec::new();

    let id = SessionId::new();
    let user = std::env::var("USER").unwrap_or_else(|_| "scrybe-user".into());
    let started_at = Utc::now();

    // `mic_keepalive` owns the live `MicCapture` so the dedicated cpal
    // capture thread keeps running for the lifetime of `run_session`.
    // Dropping `MicCapture` tears down the cpal stream via the
    // `SharedState::stop_tx` channel; we keep it bound here to defer
    // that drop until after the session writes its outputs.
    #[cfg(feature = "mic-capture")]
    let mut mic_keepalive: Option<MicCapture> = None;

    let stop_future = Box::pin(wait_for_stop(stop_rx));
    let stream: Pin<Box<dyn Stream<Item = Result<AudioFrame, CaptureError>> + Send>> =
        match args.source {
            CaptureSourceArg::Synthetic => {
                Box::pin(synthetic_capture_stream(args.synthetic_secs).take_until(stop_future))
            }
            CaptureSourceArg::Mic => {
                #[cfg(feature = "mic-capture")]
                {
                    let mut mic = MicCapture::new();
                    mic.start().context(
                        "opening default input device (grant Microphone permission \
                     in System Settings → Privacy & Security if prompted)",
                    )?;
                    let s: Pin<Box<dyn Stream<Item = _> + Send>> =
                        Box::pin(mic.frames().take_until(stop_future));
                    mic_keepalive = Some(mic);
                    s
                }
                #[cfg(not(feature = "mic-capture"))]
                {
                    anyhow::bail!(
                    "--source mic requires the binary to be built with --features mic-capture; \
                     this binary was built without it"
                );
                }
            }
        };

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

    #[cfg(feature = "mic-capture")]
    drop(mic_keepalive);

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

/// Future that completes the first time `stop_rx` flips to `true`,
/// or when every `Sender` has been dropped. Used as the `take_until`
/// argument so the synthetic stream tears down deterministically
/// when SIGINT, the global hotkey, or the tray Quit menu fires.
async fn wait_for_stop(mut stop_rx: watch::Receiver<bool>) {
    let _ = stop_rx.wait_for(|stopped| *stopped).await;
}

fn spawn_sigint_listener(stop_tx: watch::Sender<bool>) -> JoinHandle<()> {
    tokio::spawn(async move {
        if tokio::signal::ctrl_c().await.is_ok() {
            let _ = stop_tx.send(true);
        }
    })
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

/// CLI-local STT dispatch over the two providers `scrybe record` can
/// pick at runtime. Enum variants stay `Sized` so the existing
/// `SessionInputs<S: SttProvider>` generic does not need a `?Sized`
/// relaxation in `scrybe-core` for this v1.0.x patch.
enum CliStt {
    Stub(StubLocalStt),
    #[cfg(feature = "whisper-local")]
    Whisper(WhisperLocalProvider),
}

#[async_trait]
impl SttProvider for CliStt {
    async fn transcribe(&self, chunk: AudioChunk) -> Result<TranscriptChunk, SttError> {
        match self {
            Self::Stub(s) => s.transcribe(chunk).await,
            #[cfg(feature = "whisper-local")]
            Self::Whisper(s) => s.transcribe(chunk).await,
        }
    }

    fn name(&self) -> &str {
        match self {
            Self::Stub(s) => s.name(),
            #[cfg(feature = "whisper-local")]
            Self::Whisper(s) => s.name(),
        }
    }
}

/// Construct the STT provider from the `--whisper-model` flag.
///
/// - `Some(path)` + `--features whisper-local` → real
///   `WhisperLocalProvider` against the supplied weights.
/// - `Some(path)` + no `whisper-local` feature → hard error so the
///   user does not silently get the stub when they asked for real
///   transcription.
/// - `None` → `StubLocalStt` so the synthetic-source CI smoke path
///   stays hermetic.
#[allow(unused_variables)]
fn build_stt_provider(whisper_model: Option<&PathBuf>) -> Result<CliStt> {
    match whisper_model {
        None => Ok(CliStt::Stub(StubLocalStt::new())),
        Some(path) => {
            #[cfg(feature = "whisper-local")]
            {
                let cfg = WhisperLocalConfig::new(path.clone());
                let provider = WhisperLocalProvider::new(cfg)
                    .with_context(|| format!("loading whisper.cpp model at {}", path.display()))?;
                Ok(CliStt::Whisper(provider))
            }
            #[cfg(not(feature = "whisper-local"))]
            {
                anyhow::bail!(
                    "--whisper-model {} provided but binary built without --features whisper-local; \
                     rebuild with `cargo install --features whisper-local,...` or remove the flag",
                    path.display()
                );
            }
        }
    }
}

/// CLI-local stub STT provider. Emits a deterministic line so the
/// rest of the pipeline (transcript append, LLM prompt rendering,
/// notes write) is exercisable without a real Whisper model. Using a
/// real model is wired via `--features whisper-local` plus the
/// `--whisper-model <PATH>` flag — see `build_stt_provider` above.
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
                    .is_ok_and(|frame| frame.samples.iter().any(|s| s.abs() > 0.01))
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

    #[tokio::test]
    async fn test_stub_local_llm_returns_template_notes_body() {
        let llm = StubLocalLlm::new();

        let body = llm.complete("any prompt").await.unwrap();

        assert!(body.contains("## TL;DR"));
        assert!(body.contains("## Action items"));
    }

    #[tokio::test]
    async fn test_binary_channel_diarizer_labels_mic_as_me_and_system_as_them() {
        let mic = vec![TranscriptChunk {
            text: "hi".into(),
            source: FrameSource::Mic,
            start_ms: 0,
            duration_ms: 1_000,
            language: None,
        }];
        let sys = vec![TranscriptChunk {
            text: "hello".into(),
            source: FrameSource::System,
            start_ms: 0,
            duration_ms: 1_000,
            language: None,
        }];

        let result = BinaryChannelDiarizer
            .diarize(&mic, &sys, &MeetingContext::default())
            .await
            .unwrap();

        assert_eq!(result.len(), 2);
        assert_eq!(result[0].speaker, SpeakerLabel::Me);
        assert_eq!(result[1].speaker, SpeakerLabel::Them);
    }

    #[test]
    fn test_binary_channel_diarizer_name_returns_binary_channel() {
        assert_eq!(BinaryChannelDiarizer.name(), "binary-channel");
    }

    #[test]
    fn test_stub_local_stt_name_returns_stub_local_stt() {
        assert_eq!(StubLocalStt::new().name(), "stub-local-stt");
    }

    #[test]
    fn test_stub_local_llm_name_returns_stub_local_llm() {
        assert_eq!(StubLocalLlm::new().name(), "stub-local-llm");
    }

    #[tokio::test]
    async fn test_run_writes_session_artifacts_for_synthetic_capture() {
        // Point config discovery at a tempdir so the test does not
        // pick up a malformed real config from the developer's home.
        let cfg_dir = tempfile::tempdir().unwrap();
        std::env::set_var("SCRYBE_CONFIG", cfg_dir.path().join("no-such-config.toml"));
        let dir = tempfile::tempdir().unwrap();

        run(Args {
            title: Some("synthetic".into()),
            root: Some(dir.path().to_path_buf()),
            yes: true,
            consent: ConsentModeArg::Quick,
            synthetic_secs: 1,
            shell: false,
            source: CaptureSourceArg::Synthetic,
            whisper_model: None,
        })
        .await
        .unwrap();

        let mut entries = std::fs::read_dir(dir.path()).unwrap();
        let session = entries
            .next()
            .expect("a session folder must exist")
            .unwrap();
        assert!(session.path().join("transcript.md").exists());
        assert!(session.path().join("notes.md").exists());
        assert!(session.path().join("meta.toml").exists());
    }

    #[tokio::test]
    async fn test_wait_for_stop_resolves_when_sender_flips_to_true() {
        let (tx, rx) = watch::channel(false);
        let fut = wait_for_stop(rx);
        tokio::pin!(fut);

        assert!(
            futures::poll!(&mut fut).is_pending(),
            "wait_for_stop must remain pending while the flag is false"
        );

        tx.send(true).unwrap();
        fut.await;
    }

    #[tokio::test]
    async fn test_wait_for_stop_returns_immediately_when_sender_already_true() {
        let (_tx, rx) = watch::channel(true);

        wait_for_stop(rx).await;
    }

    #[tokio::test]
    async fn test_wait_for_stop_resolves_when_sender_dropped() {
        let (tx, rx) = watch::channel(false);
        drop(tx);

        wait_for_stop(rx).await;
    }

    #[tokio::test]
    async fn test_run_auto_accepts_consent_via_env_var_when_yes_flag_is_false() {
        // Exercises the right-hand side of
        //   `let auto_accept = args.yes
        //       || std::env::var("SCRYBE_CONSENT_AUTO_ACCEPT").as_deref() == Ok("1");`
        // Other tests pass `yes: true`, which short-circuits the OR
        // before the env-var check; this is the only path that
        // covers the env-var arm. Setting the env var here is safe
        // because every other record test already auto-accepts via
        // `yes: true`, so this test cannot flip an unsuspecting
        // sibling into a different code path.
        let cfg_dir = tempfile::tempdir().unwrap();
        std::env::set_var("SCRYBE_CONFIG", cfg_dir.path().join("no-such-config.toml"));
        std::env::set_var("SCRYBE_CONSENT_AUTO_ACCEPT", "1");
        let dir = tempfile::tempdir().unwrap();

        let result = run(Args {
            title: Some("env-consent".into()),
            root: Some(dir.path().to_path_buf()),
            yes: false,
            consent: ConsentModeArg::Quick,
            synthetic_secs: 1,
            shell: false,
            source: CaptureSourceArg::Synthetic,
            whisper_model: None,
        })
        .await;

        std::env::remove_var("SCRYBE_CONSENT_AUTO_ACCEPT");
        result.unwrap();
    }

    #[tokio::test]
    async fn test_synthetic_capture_stream_emits_only_silence_for_zero_seconds() {
        // `synthetic_capture_stream(0)` short-circuits the speech-frame
        // branch in the closure; cover the silence-only iteration path.
        let stream = synthetic_capture_stream(0);
        let frames: Vec<_> = stream.collect().await;

        let speech_count = frames
            .iter()
            .filter(|f| {
                f.as_ref()
                    .is_ok_and(|frame| frame.samples.iter().any(|s| s.abs() > 0.01))
            })
            .count();
        assert_eq!(speech_count, 0);
    }

    /// E-5 from `.docs/development-plan.md` §7.3.3: cold-start latency.
    ///
    /// The §7.3.3 budget is 12 s, anchored to real Whisper warm-up
    /// (loading `large-v3-turbo` weights, JIT-compiling Metal shaders,
    /// running a silence buffer to prime the encoder). With the stub
    /// providers used here, actual elapsed is sub-second; the budget
    /// loosens to 10 s as a "pipeline didn't hang or pick up an
    /// unbounded retry loop" guard. The Whisper-warm-up assertion
    /// returns when `whisper-local` is enabled in CI — currently that
    /// feature isn't on the default build because `whisper-rs` needs a
    /// verified C++ toolchain on the macos-14 hosted runner per
    /// `scrybe-cli/Cargo.toml`'s `[package.metadata.dist]` block.
    ///
    /// 10 s is loose enough to absorb CI noise (Windows shared
    /// runners are the slowest cell in the matrix today; the macos-14
    /// build job's full pipeline takes ~50 s, of which test startup
    /// is a few hundred ms). If this test starts flaking, the right
    /// move is to investigate what's slowing the stub-provider path,
    /// not to bump the budget further.
    #[tokio::test]
    async fn test_run_completes_within_cold_start_budget_with_stub_providers() {
        const COLD_START_BUDGET: std::time::Duration = std::time::Duration::from_secs(10);

        let cfg_dir = tempfile::tempdir().unwrap();
        std::env::set_var("SCRYBE_CONFIG", cfg_dir.path().join("no-such-config.toml"));
        let dir = tempfile::tempdir().unwrap();

        let started = std::time::Instant::now();
        run(Args {
            title: Some("cold-start".into()),
            root: Some(dir.path().to_path_buf()),
            yes: true,
            consent: ConsentModeArg::Quick,
            synthetic_secs: 1,
            shell: false,
            source: CaptureSourceArg::Synthetic,
            whisper_model: None,
        })
        .await
        .unwrap();
        let elapsed = started.elapsed();

        assert!(
            elapsed < COLD_START_BUDGET,
            "cold-start exceeded {COLD_START_BUDGET:?}: actual {elapsed:?} \
             — the stub-provider path should complete sub-second; investigate \
             before bumping this budget"
        );
    }

    #[test]
    fn test_capture_source_arg_default_is_synthetic() {
        assert_eq!(CaptureSourceArg::default(), CaptureSourceArg::Synthetic);
    }

    #[test]
    fn test_build_stt_provider_returns_stub_when_no_model_path_supplied() {
        let stt = build_stt_provider(None).expect("stub branch must succeed");
        assert_eq!(stt.name(), "stub-local-stt");
    }

    #[cfg(not(feature = "whisper-local"))]
    #[test]
    fn test_build_stt_provider_errors_when_model_supplied_without_feature() {
        let path = std::path::PathBuf::from("/tmp/no-such-model.bin");
        let result = build_stt_provider(Some(&path));
        let Err(err) = result else {
            panic!("flag without feature must error rather than silently stub");
        };
        let msg = format!("{err:?}");
        assert!(
            msg.contains("--whisper-model") && msg.contains("--features whisper-local"),
            "error must name both the flag and the missing feature; got: {msg}"
        );
    }

    #[cfg(feature = "whisper-local")]
    #[test]
    fn test_build_stt_provider_rejects_partial_model_path() {
        // `WhisperLocalProvider::new` rejects `*.partial` paths up-front
        // (see scrybe-core::providers::whisper_local). The CLI surfaces
        // this as a `loading whisper.cpp model at <path>` context-chained
        // error so the user sees both the offending path and the
        // underlying contract violation.
        let dir = tempfile::tempdir().unwrap();
        let partial = dir.path().join("ggml-tiny.bin.partial");
        std::fs::write(&partial, b"unfinished download").unwrap();
        let result = build_stt_provider(Some(&partial));
        let Err(err) = result else {
            panic!("partial paths must be rejected at construction");
        };
        let msg = format!("{err:?}");
        assert!(
            msg.contains("loading whisper.cpp model"),
            "context chain must mention the loading step; got: {msg}"
        );
    }
}
