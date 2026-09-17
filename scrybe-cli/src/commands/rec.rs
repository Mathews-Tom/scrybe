// Copyright 2026 Mathews Tom
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//     https://www.apache.org/licenses/LICENSE-2.0

//! `scrybe rec` — start a session (explicit-flags entry point).
//!
//! v1.0.5+ split: this module is the explicit-flags entry point used
//! by CI, scripts, and advanced users. The new `scrybe record <title>`
//! ergonomic command (see `commands::record`) wraps this with config-
//! default resolution and macOS Launch-Services auto-launch so end
//! users typically never invoke `rec` directly. The module name is
//! `rec` (renamed from `record` at v1.0.5) to free up the `record`
//! word for the user-facing entry point.
//!
//! v1.0.1+ closes the v0.1 mic-only path (`.docs/development-plan.md`
//! §7.2). Three opt-in flags surface real audio capture and real
//! Whisper transcription:
//!
//! - `--source mic` consumes frames from the default input device via
//!   `scrybe-capture-mic` (cpal). Requires the binary to be built
//!   with `--features mic-capture`; absent that feature the call
//!   returns `CaptureError::PermissionDenied`.
//! - `--source mic+system` (v1.0.3+) layers `scrybe-capture-mac`
//!   Core Audio Taps on top of the mic adapter so the meeting
//!   counterparty's audio also flows through the pipeline. Frames
//!   from each source carry their own `FrameSource` tag, so the
//!   `BinaryChannelDiarizer` can attribute them to `Me:` (mic) and
//!   `Them:` (system) in `transcript.md`. Requires the binary to be
//!   built with `--features mic-capture,system-capture-mac` and
//!   macOS 14.4+ with the Audio Capture TCC permission granted.
//! - `--whisper-model <PATH>` swaps the stub STT provider for
//!   `WhisperLocalProvider` against the supplied `.bin` / `.gguf`
//!   weights. Requires the binary to be built with
//!   `--features whisper-local`; absent that feature the flag errors
//!   at start time rather than silently falling back to the stub.
//!
//! Without any flag the recorder runs the deterministic synthetic
//! pipeline (440 Hz sine + canned transcripts) so CI smoke tests stay
//! hermetic.

use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};

use crate::capture_control::CaptureRegistry;
use async_trait::async_trait;
use chrono::Utc;
use clap::{Args as ClapArgs, ValueEnum};
use futures::stream::{self, Stream, StreamExt};
use scrybe_application::recording::{
    CaptureCapability, CaptureSource, CaptureSupport, NotesBackend, RecordingController,
    RecordingOverrides, RecordingPlan, RecordingSnapshot, RecordingState, StopAcceptance,
    StopSource, SystemBackend, TranscriptionModel,
};
#[cfg(all(feature = "mic-capture", feature = "system-capture-mac"))]
use scrybe_capture_mac::{input_devices, InputDevice, MacCapture, NativeMicCapture, SckCapture};
#[cfg(all(feature = "mic-capture", not(feature = "system-capture-mac")))]
use scrybe_capture_mic::MicCapture;
// AudioCapture is the registry's common bound whenever microphone capture is
// compiled into the binary.
#[cfg(feature = "mic-capture")]
use scrybe_core::capture::AudioCapture;

use scrybe_core::context::MeetingContext;
use scrybe_core::diarize::Diarizer;
use scrybe_core::error::{CaptureError, CoreError, LlmError, SttError};
use scrybe_core::hooks::{Hook, LifecycleEvent};
use scrybe_core::notes_map_reduce::NotesRuntime;
use scrybe_core::pipeline::chunker::ChunkerConfig;
use scrybe_core::pipeline::vad::EnergyVad;
#[cfg(feature = "llm-openai-compat")]
use scrybe_core::providers::openai_compat_llm::OpenAiCompatLlmProvider;
#[cfg(feature = "stt-sherpa")]
use scrybe_core::providers::sherpa_streaming::{SherpaStreamingConfig, SherpaStreamingProvider};
use scrybe_core::providers::streaming::StreamingSttProvider;
#[cfg(feature = "whisper-local")]
use scrybe_core::providers::whisper_local::{WhisperLocalConfig, WhisperLocalProvider};
use scrybe_core::providers::{LlmProvider, SttProvider};
use scrybe_core::session::{
    run_with_notes as run_session_with_notes, SessionInputs, SessionProgress,
};
#[cfg(any(test, all(feature = "mic-capture", feature = "system-capture-mac")))]
use scrybe_core::storage::session_folder_name;
use scrybe_core::types::{
    AttributedChunk, AudioChunk, AudioFrame, ConsentMode, FrameSource, SessionId, SpeakerLabel,
    TranscriptChunk,
};
use tokio::sync::watch;

use crate::prompter::TtyPrompter;
use crate::runtime::{application, config_service};

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

    /// Consent mode. Omitted means use `[consent].default_mode`.
    #[arg(long, value_enum)]
    pub consent: Option<ConsentModeArg>,

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
    /// `--features mic-capture`. `mic+system` additionally captures
    /// system audio (the meeting counterparty) on macOS via Core
    /// Audio Taps — requires both `mic-capture` and
    /// `system-capture-mac` features and the Audio Capture TCC
    /// permission grant. Absent the relevant feature, the call
    /// returns `CaptureError::PermissionDenied` at start time.
    #[arg(long, value_enum)]
    pub source: Option<CaptureSourceArg>,

    /// Exact macOS Core Audio input-device UID from `scrybe devices`.
    /// Display names are deliberately not accepted as selectors.
    #[arg(long)]
    pub input_device: Option<String>,

    /// System-audio adapter for `--source mic+system`. `sck` is the
    /// macOS 13+ default; `tap` selects the macOS 14.4+ Core Audio
    /// Tap path.
    #[arg(long, value_enum)]
    pub system_backend: Option<SystemBackendArg>,

    /// Path to a whisper.cpp model (`.bin` or `.gguf`). When set and
    /// `whisper-local` is compiled, transcription uses `WhisperLocalProvider`.
    /// An explicit path without that feature errors at start time rather than
    /// silently falling back to the stub.
    #[arg(long, conflicts_with = "sherpa_model")]
    pub whisper_model: Option<PathBuf>,

    /// Directory containing the pinned streaming Zipformer Sherpa-ONNX model.
    /// Requires `stt-sherpa`; without it, an explicit path errors at start
    /// time rather than silently falling back to the stub.
    #[arg(long, conflicts_with = "whisper_model")]
    pub sherpa_model: Option<PathBuf>,

    /// Language-model backend for the `notes.md` summary step. `stub`
    /// (default) returns a fixed templated body so CI smoke tests stay
    /// hermetic. `openai-compat` constructs `OpenAiCompatLlmProvider`
    /// from the `[llm]` config block (defaults to Ollama at
    /// `http://localhost:11434/v1`); requires the binary to be built
    /// with `--features llm-openai-compat`. Without that feature, an
    /// explicit `--llm openai-compat` errors at start time rather
    /// than silently falling back to the stub.
    #[arg(long, value_enum)]
    pub llm: Option<LlmBackendArg>,

    /// Show native recording indicators and accept tray, floating-window,
    /// and global-hotkey stop requests. The shell is opt-in; without this
    /// flag the recorder remains headless and stops on SIGINT or when the
    /// synthetic stream completes. Requires the `cli-shell` build feature.
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
    /// Mic + macOS system audio. Surfaced to clap as the literal
    /// `mic+system` token so the CLI matches the user-facing
    /// documentation (`docs/system-overview.md` §3 channel-split path).
    #[value(name = "mic+system")]
    MicSystem,
}

#[derive(Copy, Clone, Debug, Default, Eq, PartialEq, ValueEnum)]
pub enum SystemBackendArg {
    #[default]
    Sck,
    Tap,
}

#[derive(Copy, Clone, Debug, Default, Eq, PartialEq, ValueEnum)]
pub enum LlmBackendArg {
    #[default]
    Stub,
    /// `OpenAiCompatLlmProvider` against any `/chat/completions`
    /// endpoint configured under `[llm]` — Ollama, vLLM, `OpenAI`,
    /// Groq, Together. Surfaced to clap as the literal `openai-compat`
    /// token to match `docs/system-design.md` §4.3.
    #[value(name = "openai-compat")]
    OpenAiCompat,
}

#[cfg(all(feature = "mic-capture", feature = "system-capture-mac"))]
enum SystemCapture {
    Sck(SckCapture),
    Tap(MacCapture),
}

#[cfg(all(feature = "mic-capture", feature = "system-capture-mac"))]
impl SystemCapture {
    fn new(backend: SystemBackend) -> Self {
        match backend {
            SystemBackend::Sck => Self::Sck(SckCapture::new()),
            SystemBackend::Tap => Self::Tap(MacCapture::new()),
        }
    }

    fn start(&mut self) -> Result<()> {
        match self {
            Self::Sck(capture) => capture.start().map_err(Into::into),
            Self::Tap(capture) => capture.start().map_err(Into::into),
        }
    }

    fn frames(&self) -> Pin<Box<dyn Stream<Item = Result<AudioFrame, CaptureError>> + Send>> {
        match self {
            Self::Sck(capture) => Box::pin(capture.frames()),
            Self::Tap(capture) => Box::pin(capture.frames()),
        }
    }

    fn stop(&mut self) -> Result<()> {
        match self {
            Self::Sck(capture) => capture.stop().map_err(Into::into),
            Self::Tap(capture) => capture.stop().map_err(Into::into),
        }
    }
}

#[cfg(any(test, all(feature = "mic-capture", feature = "system-capture-mac")))]
const TAP_STARTUP_ACTIVITY_WINDOW: Duration = Duration::from_millis(1_500);

#[cfg(any(test, feature = "mic-capture"))]
type CaptureFrameStream = Pin<Box<dyn Stream<Item = Result<AudioFrame, CaptureError>> + Send>>;

#[cfg(all(feature = "mic-capture", feature = "system-capture-mac"))]
async fn start_system_capture(
    selected: SystemBackend,
) -> Result<(SystemCapture, CaptureFrameStream, Option<&'static str>)> {
    let mut capture = SystemCapture::new(selected);
    if let Err(error) = capture.start() {
        let Some(backend) = fallback_backend(selected) else {
            return Err(error);
        };
        let mut fallback = SystemCapture::new(backend);
        fallback.start().context(
            "Core Audio Tap failed to start and ScreenCaptureKit could not start either",
        )?;
        let frames = fallback.frames();
        return Ok((
            fallback,
            frames,
            Some("system capture switched from tap to sck after tap start failure"),
        ));
    }
    let frames = capture.frames();
    if fallback_backend(selected).is_some() {
        let (active, frames) = tap_produces_nonzero_frames(frames).await;
        if !active {
            capture
                .stop()
                .context("stopping silent Core Audio Tap before fallback")?;
            let mut fallback = SystemCapture::new(SystemBackend::Sck);
            fallback.start().context(
                "Core Audio Tap had no startup activity and ScreenCaptureKit could not start",
            )?;
            let frames = fallback.frames();
            return Ok((
                fallback,
                frames,
                Some("system capture switched from tap to sck after no tap startup activity"),
            ));
        }
        return Ok((capture, frames, None));
    }
    Ok((capture, frames, None))
}

#[cfg(any(test, all(feature = "mic-capture", feature = "system-capture-mac")))]
async fn tap_produces_nonzero_frames(mut frames: CaptureFrameStream) -> (bool, CaptureFrameStream) {
    let deadline = tokio::time::Instant::now() + TAP_STARTUP_ACTIVITY_WINDOW;
    let mut buffered = Vec::new();
    let mut active = false;
    loop {
        match tokio::time::timeout_at(deadline, frames.next()).await {
            Ok(Some(Ok(frame))) => {
                active |= frame.samples.iter().any(|sample| *sample != 0.0);
                buffered.push(Ok(frame));
                if active {
                    break;
                }
            }
            Ok(Some(Err(error))) => {
                buffered.push(Err(error));
                break;
            }
            Ok(None) | Err(_) => break,
        }
    }
    (
        active,
        Box::pin(futures::stream::iter(buffered).chain(frames)),
    )
}

#[cfg(any(test, all(feature = "mic-capture", feature = "system-capture-mac")))]
const fn fallback_backend(selected: SystemBackend) -> Option<SystemBackend> {
    match selected {
        SystemBackend::Tap => Some(SystemBackend::Sck),
        SystemBackend::Sck => None,
    }
}

impl From<CaptureSourceArg> for CaptureSource {
    fn from(value: CaptureSourceArg) -> Self {
        match value {
            CaptureSourceArg::Synthetic => Self::Synthetic,
            CaptureSourceArg::Mic => Self::Mic,
            CaptureSourceArg::MicSystem => Self::MicSystem,
        }
    }
}

impl From<SystemBackendArg> for SystemBackend {
    fn from(value: SystemBackendArg) -> Self {
        match value {
            SystemBackendArg::Sck => Self::Sck,
            SystemBackendArg::Tap => Self::Tap,
        }
    }
}

impl From<LlmBackendArg> for NotesBackend {
    fn from(value: LlmBackendArg) -> Self {
        match value {
            LlmBackendArg::Stub => Self::Stub,
            LlmBackendArg::OpenAiCompat => Self::OpenAiCompat,
        }
    }
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

/// Record a session, stoppable by `Ctrl-C` or `SIGTERM`.
///
/// Drives the same process-wide recording controller the native shell
/// uses, so a signal stop and a tray stop are the same accepted
/// transition rather than two implementations of one idea.
///
/// # Errors
///
/// Propagates configuration, capture, provider, and storage failures
/// from the recording session.
/// What this invocation asks for, over the configuration file.
///
/// The CLI's flags are overrides and nothing else: resolution itself
/// belongs to `scrybe-application`, so a terminal and a desktop host
/// read the same file to the same answers.
#[must_use]
pub fn overrides_from(args: &Args) -> RecordingOverrides {
    RecordingOverrides {
        title: args.title.clone(),
        root: args.root.clone(),
        source: args.source.map(Into::into),
        system_backend: args.system_backend.map(Into::into),
        input_device: args.input_device.clone(),
        whisper_model: args.whisper_model.clone(),
        sherpa_model: args.sherpa_model.clone(),
        notes: args.llm.map(Into::into),
        consent: args.consent.map(Into::into),
    }
}

/// What this binary was compiled to be able to open.
///
/// Derived from the same `cfg!` conditions the capture construction in
/// `run_with_stop` is written under, so preflight cannot report a
/// capability the code below then refuses to provide.
#[must_use]
pub const fn build_support() -> CaptureSupport {
    CaptureSupport {
        capture: if cfg!(all(feature = "mic-capture", feature = "system-capture-mac")) {
            CaptureCapability::MicrophoneAndSystemAudio
        } else if cfg!(feature = "mic-capture") {
            CaptureCapability::Microphone
        } else {
            CaptureCapability::SyntheticOnly
        },
        transcription_model: cfg!(any(feature = "whisper-local", feature = "stt-sherpa")),
        notes_provider: cfg!(feature = "llm-openai-compat"),
    }
}

/// The input-device UIDs this platform offers, or `None` when this
/// build cannot enumerate them.
///
/// `None` is not an empty list: a build with no Core Audio binding
/// knows nothing about the machine's devices, and preflight reports
/// that as unverified rather than as a device that is missing.
#[must_use]
#[allow(
    clippy::missing_const_for_fn,
    reason = "const under one feature selection only; the enumerating build allocates"
)]
pub fn available_devices() -> Option<Vec<String>> {
    #[cfg(all(feature = "mic-capture", feature = "system-capture-mac"))]
    {
        input_devices().ok().map(|devices| {
            devices
                .into_iter()
                .map(|device| device.uid)
                .collect::<Vec<_>>()
        })
    }
    #[cfg(not(all(feature = "mic-capture", feature = "system-capture-mac")))]
    {
        None
    }
}

/// Resolves and checks this invocation, leaving the controller
/// preparing.
///
/// The one preflight, shared with every other surface. Six of its seven
/// checks used to be written inline further down this file, where no
/// other frontend could reach them and where a failure was labelled as
/// a capture failure rather than a preflight one.
///
/// # Errors
///
/// The preflight refusal, with every blocking check named.
pub fn begin_recording(controller: &RecordingController, args: &Args) -> Result<RecordingPlan> {
    let devices = available_devices();
    scrybe_application::recording::begin(
        controller,
        &config_service()?,
        home_directory().as_deref(),
        build_support(),
        devices.as_deref(),
        &overrides_from(args),
    )
    .map_err(|refusal| {
        let hint = rebuild_hint(&refusal);
        let error = anyhow::Error::from(refusal.error);
        match hint {
            Some(hint) => error.context(hint),
            None => error,
        }
    })
}

/// How to rebuild this binary so a capture it refused would work.
///
/// The preflight refuses a source this build cannot open, but it cannot
/// say what to do about it: the Cargo features that decide it belong to
/// this package, and `scrybe-application` is shared with a desktop host
/// whose answer is a different build entirely. So the generic refusal
/// travels, and the package-specific remedy is attached here.
fn rebuild_hint(refusal: &scrybe_application::recording::Refusal) -> Option<String> {
    let blocked_on_capture = refusal
        .report
        .blocking()
        .iter()
        .any(|finding| finding.check == scrybe_application::recording::PreflightCheck::Capture);
    if !blocked_on_capture {
        return None;
    }
    match refusal.plan.as_ref()?.source {
        CaptureSource::Synthetic => None,
        CaptureSource::Mic => Some(
            "--source mic requires the binary to be built with --features mic-capture; \
             this binary was built without it"
                .to_string(),
        ),
        CaptureSource::MicSystem => Some(
            "--source mic+system requires the binary to be built with both \
             --features mic-capture and --features system-capture-mac; \
             this binary was built without one or both"
                .to_string(),
        ),
    }
}

fn home_directory() -> Option<PathBuf> {
    directories::BaseDirs::new().map(|dirs| dirs.home_dir().to_path_buf())
}

pub async fn run(args: Args) -> Result<()> {
    let (stop_tx, stop_rx) = watch::channel(false);
    // The controller comes from the composition root, never from a
    // fresh instance: the tray, the floating pill, the global hotkey,
    // `Ctrl-C`, and `SIGTERM` must all converge on one state model, and
    // a second instance would let two consumers give two different
    // answers to "is a recording running".
    let controller = Arc::clone(application(args.root.as_deref())?.recording());
    // Resolution and the seven checks, then `Preparing` — all of it in
    // `scrybe-application`, so this terminal and the desktop host reach
    // the same answers from the same file. A refusal here has written
    // nothing, which is why `RecordingFailureKind::Preflight` means
    // what it says: no journal exists to recover.
    //
    // The controller stays `Preparing` from here until the pipeline
    // publishes `SessionProgress::Recording`, which is the real capture
    // boundary. Marking recording earlier started the single monotonic
    // origin before capture existed, counting permission prompts and a
    // model load as recorded time.
    let plan = begin_recording(&controller, &args)?;

    let signal_controller = Arc::clone(&controller);
    let signal_handle = tokio::spawn(monitor_signals(move || {
        if signal_controller.request_stop(StopSource::Signal) == StopAcceptance::Accepted {
            let _ = stop_tx.send(true);
        }
    }));
    let result = run_with_stop(args, plan, stop_rx, Some(Arc::clone(&controller))).await;
    signal_handle.abort();
    settle(&controller, result.as_ref().err());
    result
}

/// The only summary a recording failure ever carries into a serialized
/// event. Fixed by construction, so no path, provider, device, or model
/// identity can reach a surface through it.
pub const RECORDING_FAILURE_SUMMARY: &str = "recording session failed";

/// Walks the controller to a terminal state and back to idle.
///
/// The controller labels a failure from the state it happened in, so
/// this never decides whether a failure was capture-side or
/// finalization-side.
fn settle(controller: &RecordingController, failure: Option<&anyhow::Error>) {
    if let Some(error) = failure {
        settle_failure(controller, error);
    } else {
        if controller.snapshot().state == RecordingState::Recording {
            if let Err(conflict) = controller.begin_saving() {
                tracing::debug!(%conflict, "recording controller could not enter saving");
            }
        }
        if let Err(conflict) = controller.complete() {
            tracing::debug!(%conflict, "recording controller could not complete");
        }
    }
    if let Err(conflict) = controller.acknowledge() {
        tracing::debug!(%conflict, "recording controller could not settle to idle");
    }
}

/// Records `error` against the controller and returns the snapshot a
/// surface would be handed, or `None` if the state could not fail.
///
/// The summary passed to `fail` is a fixed literal, never
/// `error.to_string()`. That string becomes `RecordingFailure::summary`,
/// a `Serialize` field of both `RecordingEvent` and
/// `RecordingSnapshot` — the event boundary the contract says carries
/// no path or provider content — and the outermost `anyhow` context on
/// this path frequently is not fixed: creating the storage root
/// interpolates an absolute path, the config load propagates its own
/// config-path error, the notes runtime can surface a model path and
/// provider name, and input-device resolution surfaces device identity.
/// The detailed error still reaches the caller through the returned
/// `Result` and the trace here, neither of which is an event surface.
fn settle_failure(
    controller: &RecordingController,
    error: &anyhow::Error,
) -> Option<RecordingSnapshot> {
    tracing::error!(%error, "recording session failed");
    match controller.fail(RECORDING_FAILURE_SUMMARY) {
        Ok(snapshot) => Some(snapshot),
        Err(conflict) => {
            tracing::debug!(%conflict, "recording failure arrived in a state that cannot fail");
            None
        }
    }
}

#[cfg(feature = "mic-capture")]
fn start_registered_capture<T>(registry: &CaptureRegistry, capture: T) -> Result<CaptureFrameStream>
where
    T: AudioCapture,
{
    let capture = registry.register(capture);
    let mut capture = capture
        .lock()
        .map_err(|_| anyhow::anyhow!("capture registry adapter mutex poisoned"))?;
    capture.start()?;
    Ok(Box::pin(capture.frames()))
}

/// Drive a session under an externally-supplied stop signal. The shell driver
/// in `scrybe-cli::shell` calls this directly, feeding stop into `stop_rx` from
/// tray and hotkey events; `run` above wraps it with signal handling.
#[allow(clippy::too_many_lines)]
pub async fn run_with_stop(
    args: Args,
    plan: RecordingPlan,
    stop_rx: watch::Receiver<bool>,
    controller: Option<Arc<RecordingController>>,
) -> Result<()> {
    let cfg = config_service()?.load()?;
    // Every value this function used to resolve for itself now arrives
    // in `plan`, resolved once by the shared orchestration. What is
    // still read from `cfg` here are the blocks the plan does not
    // decide: the language a model is loaded with, the notes runtime's
    // own settings, and the endpoint the notes provider talks to.
    let root = plan.root.clone();
    tokio::fs::create_dir_all(&root)
        .await
        .with_context(|| format!("creating storage root {}", root.display()))?;

    let auto_accept = args.yes || std::env::var("SCRYBE_CONSENT_AUTO_ACCEPT").as_deref() == Ok("1");
    let prompter = TtyPrompter::new(auto_accept);
    let source = plan.source;
    let consent_mode = plan.consent;

    let llm = build_llm_provider(plan.notes, &cfg.llm)?;
    let notes_runtime = match plan.notes {
        NotesBackend::Stub => None,
        NotesBackend::OpenAiCompat => Some(NotesRuntime::load(&cfg.notes)?),
    };
    let system_backend = plan.system_backend;
    #[cfg(not(all(feature = "mic-capture", feature = "system-capture-mac")))]
    let _ = system_backend;
    let diarizer = BinaryChannelDiarizer;
    let hooks: Vec<Box<dyn Hook>> = Vec::new();

    let id = SessionId::new();
    let user = std::env::var("USER").unwrap_or_else(|_| "scrybe-user".into());
    let started_at = Utc::now();

    let capture_registry = CaptureRegistry::default();

    // System frames need their own VAD/chunker for the binary-channel
    // diarizer to attribute them as `Them:`. Set to `Some(...)` only
    // when the source carries system frames.
    let system_vad: Option<EnergyVad> = match source {
        CaptureSource::MicSystem => Some(EnergyVad::default()),
        CaptureSource::Synthetic | CaptureSource::Mic => None,
    };
    #[cfg(all(feature = "mic-capture", feature = "system-capture-mac"))]
    let selected_input = match source {
        CaptureSource::Synthetic => None,
        CaptureSource::Mic | CaptureSource::MicSystem => {
            let device = resolve_macos_input_device(plan.input_device.as_deref())?;
            eprintln!("scrybe: input: {} ({})", device.name, device.uid);
            Some(device)
        }
    };

    let registry_for_stop = capture_registry.clone();
    let stop_future = Box::pin(async move {
        wait_for_stop(stop_rx).await;
        if let Err(error) = registry_for_stop.stop_all() {
            tracing::error!(error = %error, "stopping registered capture failed");
        }
    });
    let stream: Pin<Box<dyn Stream<Item = Result<AudioFrame, CaptureError>> + Send>> = match source
    {
        CaptureSource::Synthetic => {
            Box::pin(synthetic_capture_stream(args.synthetic_secs).take_until(stop_future))
        }
        CaptureSource::Mic => {
            #[cfg(all(feature = "mic-capture", feature = "system-capture-mac"))]
            {
                let device = selected_input
                    .as_ref()
                    .context("resolved microphone missing for mic capture")?;
                let stream = start_registered_capture(
                    &capture_registry,
                    NativeMicCapture::new(device.uid.clone(), plan.aec),
                )
                .with_context(|| {
                    format!(
                        "opening selected Core Audio input {} ({})",
                        device.name, device.uid
                    )
                })?;
                Box::pin(stream.take_until(stop_future))
            }
            #[cfg(all(feature = "mic-capture", not(feature = "system-capture-mac")))]
            {
                if plan.input_device.is_some() {
                    anyhow::bail!(
                        "--input-device requires a macOS build with --features \
                         mic-capture,system-capture-mac"
                    );
                }
                let stream = start_registered_capture(&capture_registry, MicCapture::new())
                    .context(
                        "opening default input device (grant Microphone permission \
                         in System Settings → Privacy & Security if prompted)",
                    )?;
                Box::pin(stream.take_until(stop_future))
            }
            #[cfg(not(feature = "mic-capture"))]
            {
                anyhow::bail!(
                    "--source mic requires the binary to be built with --features mic-capture; \
                     this binary was built without it"
                );
            }
        }
        CaptureSource::MicSystem => {
            #[cfg(all(feature = "mic-capture", feature = "system-capture-mac"))]
            {
                use futures::stream;

                let (mut system_capture, system_frames, fallback_note) =
                    start_system_capture(system_backend).await?;
                if let Some(note) = fallback_note {
                    tracing::warn!(system_backend = "sck", "{note}");
                    write_capture_diagnostic(&root, started_at, id, args.title.as_deref(), note)?;
                }
                capture_registry.register_stopper(move || {
                    system_capture.stop().map_err(|error| {
                        CaptureError::Platform(Box::new(std::io::Error::other(error.to_string())))
                    })
                });
                let device = selected_input
                    .as_ref()
                    .context("resolved microphone missing for mic+system capture")?;
                let mic_frames = start_registered_capture(
                    &capture_registry,
                    NativeMicCapture::new(device.uid.clone(), plan.aec),
                )
                .with_context(|| {
                    format!(
                        "opening selected Core Audio input {} ({})",
                        device.name, device.uid
                    )
                })?;
                Box::pin(stream::select(mic_frames, system_frames).take_until(stop_future))
            }
            #[cfg(not(all(feature = "mic-capture", feature = "system-capture-mac")))]
            {
                anyhow::bail!(
                    "--source mic+system requires the binary to be built with both \
                     --features mic-capture and --features system-capture-mac; \
                     this binary was built without one or both"
                );
            }
        }
    };
    let stream = capture_liveness_watchdog(stream, capture_registry.clone());

    // Start hardware capture before loading the selected STT model. The capture
    // adapters buffer their frames while the model initializes, so the Tap
    // liveness probe runs during startup instead of delaying recording.
    let stt = match build_stt_provider(plan.transcription.clone(), &cfg.stt.language) {
        Ok(stt) => stt,
        Err(error) => {
            if let Err(stop_error) = capture_registry.stop_all() {
                tracing::error!(error = %stop_error, "stopping capture after STT initialization failure failed");
            }
            return Err(error);
        }
    };
    let streaming_stt = stt.streaming();

    // The pipeline already publishes both real boundaries, so the
    // controller is driven from them rather than from the call site.
    // `SessionProgress::Recording` is the moment capture exists, which
    // is where `Preparing` ends; the first finalization event is where
    // capture ends and `Saving` begins. Without the second, `Saving`
    // never spanned the finalization window, so a merge, encode,
    // transcribe, or `meta.toml` failure after a natural capture end
    // was labelled `Capture` even though audio exists and the session
    // is repairable.
    let progress = move |event: SessionProgress| {
        if let Some(controller) = controller.as_deref() {
            match event {
                SessionProgress::Recording => {
                    if let Err(conflict) = controller.mark_recording() {
                        tracing::debug!(%conflict, "capture began in a state that cannot record");
                    }
                }
                SessionProgress::FinalizingTranscript { .. }
                    if controller.snapshot().state == RecordingState::Recording =>
                {
                    if let Err(conflict) = controller.begin_saving() {
                        tracing::debug!(%conflict, "finalization began in a state that cannot save");
                    }
                }
                _ => {}
            }
        }
        print_session_progress(event);
    };
    let outputs = run_session_with_notes(
        SessionInputs {
            id,
            started_at,
            root: root.clone(),
            title: plan.title.clone(),
            user,
            consent_mode,
            context: MeetingContext {
                title: plan.title,
                ..MeetingContext::default()
            },
            mic_vad: EnergyVad::default(),
            system_vad,
            streaming_stt,
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
            // The offline merge's duration assertion is a genuine
            // safety net for a real capture device, whose frames
            // arrive at real wall-clock pace. The synthetic source
            // generates frames in-process with no real-time pacing
            // (`synthetic_capture_stream`'s doc comment); comparing
            // its encoded duration against actual elapsed CPU time

            // would fail by construction on every invocation.
            verify_duration: !matches!(source, CaptureSource::Synthetic),
            progress: Some(&progress),
        },
        stream,
        notes_runtime,
    )
    .await
    .context("running session");
    if let Err(error) = capture_registry.stop_all() {
        tracing::error!(error = %error, "stopping capture after session completion failed");
    }
    let outputs = outputs?;

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
    let playback_path = outputs.folder.join("playback.opus");
    if playback_path.exists() {
        println!("  playback:   {}", playback_path.display());
    }
    Ok(())
}

fn print_session_progress(event: SessionProgress) {
    match event {
        SessionProgress::Recording => {
            eprintln!("scrybe: recording; press Ctrl-C to stop");
        }
        SessionProgress::TranscriptAccepted(attributed) => {
            let elapsed_secs = attributed.chunk.start_ms / 1_000;
            let minutes = elapsed_secs / 60;
            let seconds = elapsed_secs % 60;
            let speaker = match &attributed.speaker {
                SpeakerLabel::Me => "Me",
                SpeakerLabel::Them => "Them",
                SpeakerLabel::Named(name) => name,
                SpeakerLabel::Unknown => "Unknown",
            };
            let text = attributed.chunk.text.trim();
            if !text.is_empty() {
                println!("[{minutes:02}:{seconds:02}] {speaker}: {text}");
            }
        }
        SessionProgress::FinalizingTranscript { pending_chunks } => {
            eprintln!(
                "scrybe: finalizing transcript ({pending_chunks} pending chunk{})",
                if pending_chunks == 1 { "" } else { "s" }
            );
        }
        SessionProgress::EncodingAudio => {
            eprintln!("scrybe: encoding audio artifacts");
        }
        SessionProgress::GeneratingNotes { groups } => {
            eprintln!(
                "scrybe: generating notes ({groups} request group{})",
                if groups == 1 { "" } else { "s" }
            );
        }
        SessionProgress::WritingMetadata => {
            eprintln!("scrybe: writing session metadata");
        }
    }
}

const CAPTURE_LIVENESS_TIMEOUT: Duration = Duration::from_secs(30);

fn capture_liveness_watchdog(
    stream: Pin<Box<dyn Stream<Item = Result<AudioFrame, CaptureError>> + Send>>,
    capture_registry: CaptureRegistry,
) -> Pin<Box<dyn Stream<Item = Result<AudioFrame, CaptureError>> + Send>> {
    capture_liveness_watchdog_with_timeout(stream, capture_registry, CAPTURE_LIVENESS_TIMEOUT)
}

fn capture_liveness_watchdog_with_timeout(
    stream: Pin<Box<dyn Stream<Item = Result<AudioFrame, CaptureError>> + Send>>,
    capture_registry: CaptureRegistry,
    timeout: Duration,
) -> Pin<Box<dyn Stream<Item = Result<AudioFrame, CaptureError>> + Send>> {
    Box::pin(stream::unfold(
        (stream, capture_registry, false),
        move |(mut stream, capture_registry, stopped)| async move {
            if stopped {
                return None;
            }
            match tokio::time::timeout(timeout, stream.next()).await {
                Ok(Some(frame)) => Some((frame, (stream, capture_registry, false))),
                Ok(None) => None,
                Err(_) => {
                    if let Err(error) = capture_registry.stop_all() {
                        tracing::error!(error = %error, "stopping stalled capture failed");
                    }
                    let error = CaptureError::Platform(Box::new(std::io::Error::new(
                        std::io::ErrorKind::TimedOut,
                        "capture liveness watchdog expired after 30 seconds without a frame",
                    )));
                    Some((Err(error), (stream, capture_registry, true)))
                }
            }
        },
    ))
}

#[cfg(any(test, all(feature = "mic-capture", feature = "system-capture-mac")))]
fn write_capture_diagnostic(
    root: &std::path::Path,
    started_at: chrono::DateTime<Utc>,
    id: SessionId,
    title: Option<&str>,
    note: &str,
) -> Result<()> {
    let folder = root.join(session_folder_name(
        started_at,
        title.unwrap_or("untitled"),
        id,
    ));
    std::fs::create_dir_all(&folder)
        .with_context(|| format!("creating capture diagnostic folder {}", folder.display()))?;
    std::fs::write(folder.join("capture.log"), format!("{note}\n"))
        .context("writing system-capture fallback diagnostic")
}

#[cfg(all(feature = "mic-capture", feature = "system-capture-mac"))]
fn resolve_macos_input_device(requested_uid: Option<&str>) -> Result<InputDevice> {
    let devices = input_devices()
        .map_err(anyhow::Error::from)
        .context("enumerating macOS Core Audio input devices")?;
    if let Some(uid) = requested_uid {
        return devices
            .into_iter()
            .find(|device| device.uid == uid)
            .with_context(|| format!("configured Core Audio input device `{uid}` was not found"));
    }
    devices
        .into_iter()
        .find(|device| device.is_default)
        .context("macOS has no default Core Audio input device")
}

/// Future that completes the first time `stop_rx` flips to `true`,
/// or when every `Sender` has been dropped. Used as the `take_until`
/// argument so capture tears down deterministically when a shell
/// control requests Stop & save.
async fn wait_for_stop(mut stop_rx: watch::Receiver<bool>) {
    let _ = stop_rx.wait_for(|stopped| *stopped).await;
}

/// First `SIGINT` or `SIGTERM` requests ordered shutdown through
/// `request_graceful_stop`. A second signal terminates immediately, leaving the
/// independently-written journal for `scrybe repair`.
pub async fn monitor_signals<F>(mut request_graceful_stop: F)
where
    F: FnMut() + Send + 'static,
{
    #[cfg(unix)]
    let Ok(mut sigterm) = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) else {
        tracing::error!("installing SIGTERM listener failed");
        return;
    };
    let mut graceful_requested = false;
    loop {
        #[cfg(unix)]
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {}
            _ = sigterm.recv() => {}
        }
        #[cfg(not(unix))]
        if tokio::signal::ctrl_c().await.is_err() {
            return;
        }
        if graceful_requested {
            std::process::exit(130);
        }
        graceful_requested = true;
        request_graceful_stop();
    }
}

/// Synthetic in-process capture source.
///
/// Generates 16-kHz mono frames of a 440 Hz sine wave for `seconds`
/// seconds and emits silence after to drive the silence-after-speech
/// chunker boundary at session end.
///
/// Frames emit as fast as the pipeline can consume them (no real-time
/// pacing) unless `SCRYBE_TEST_SYNTHETIC_FRAME_DELAY_MS` is set, in
/// which case each frame is preceded by a sleep of that many
/// milliseconds. The env var exists solely so
/// `scrybe-cli/tests/repair_sigkill.rs` can spawn a real `scrybe rec`
/// subprocess that stays "recording" long enough to `SIGKILL` it
/// mid-stream and exercise `scrybe repair` deterministically; ordinary
/// invocations (including every other test in this module) leave the
/// variable unset and see the original sub-second, instant-emission
/// behavior.
#[allow(clippy::cast_precision_loss)]
fn synthetic_capture_stream(
    seconds: u64,
) -> impl Stream<Item = Result<AudioFrame, scrybe_core::error::CaptureError>> + Send + Unpin {
    const SAMPLE_RATE: u32 = 16_000;
    const FRAME_SAMPLES: usize = 1_600;
    let total_speech = seconds * (u64::from(SAMPLE_RATE) / FRAME_SAMPLES as u64);
    let total_silence = (u64::from(SAMPLE_RATE) / FRAME_SAMPLES as u64) * 6;
    let total = total_speech + total_silence;
    let frame_delay = synthetic_frame_delay();

    Box::pin(stream::iter(0..total).then(move |i| async move {
        if !frame_delay.is_zero() {
            tokio::time::sleep(frame_delay).await;
        }
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

fn synthetic_frame_delay() -> Duration {
    std::env::var("SCRYBE_TEST_SYNTHETIC_FRAME_DELAY_MS")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .map_or(Duration::ZERO, Duration::from_millis)
}

/// CLI-local STT dispatch over the two providers `scrybe record` can
/// pick at runtime. Enum variants stay `Sized` so the existing
/// `SessionInputs<S: SttProvider>` generic does not need a `?Sized`
/// relaxation in `scrybe-core` for this v1.0.x patch.
enum CliStt {
    Stub(StubLocalStt),
    #[cfg(feature = "stt-sherpa")]
    Sherpa(SherpaStreamingProvider),
    #[cfg(feature = "whisper-local")]
    Whisper(WhisperLocalProvider),
}

impl CliStt {
    /// Streaming capability of the selected provider, when it has one.
    ///
    /// Only the Sherpa provider decodes incrementally; the stub and
    /// whisper.cpp remain batch-only and take the unchanged
    /// `SttProvider` path.
    fn streaming(&self) -> Option<&dyn StreamingSttProvider> {
        match self {
            #[cfg(feature = "stt-sherpa")]
            Self::Sherpa(provider) => Some(provider),
            #[cfg(feature = "whisper-local")]
            Self::Whisper(_) => None,
            Self::Stub(_) => None,
        }
    }
}

#[async_trait]
impl SttProvider for CliStt {
    async fn transcribe(&self, chunk: AudioChunk) -> Result<TranscriptChunk, SttError> {
        match self {
            Self::Stub(s) => s.transcribe(chunk).await,
            #[cfg(feature = "stt-sherpa")]
            Self::Sherpa(provider) => provider.transcribe(chunk).await,
            #[cfg(feature = "whisper-local")]
            Self::Whisper(provider) => provider.transcribe(chunk).await,
        }
    }

    fn name(&self) -> &str {
        match self {
            Self::Stub(s) => s.name(),
            #[cfg(feature = "stt-sherpa")]
            Self::Sherpa(provider) => SttProvider::name(provider),
            #[cfg(feature = "whisper-local")]
            Self::Whisper(provider) => provider.name(),
        }
    }
}

/// Construct the STT provider selected by the model flags.
///
/// An explicit model always requires its matching feature. The stub remains
/// the default only when no model has been requested or configured.
#[allow(unused_variables)]
fn build_stt_provider(model: TranscriptionModel, language: &str) -> Result<CliStt> {
    match model {
        TranscriptionModel::Stub => Ok(CliStt::Stub(StubLocalStt::new())),
        TranscriptionModel::Whisper(path) => {
            #[cfg(feature = "whisper-local")]
            {
                let mut config = WhisperLocalConfig::new(path.clone());
                config.language = language.to_string();
                let provider = WhisperLocalProvider::new(config)
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
        TranscriptionModel::Sherpa(path) => {
            #[cfg(feature = "stt-sherpa")]
            {
                let provider =
                    SherpaStreamingProvider::new(SherpaStreamingConfig::new(path.clone()))
                        .with_context(|| {
                            format!("loading streaming Sherpa-ONNX model at {}", path.display())
                        })?;
                Ok(CliStt::Sherpa(provider))
            }
            #[cfg(not(feature = "stt-sherpa"))]
            {
                anyhow::bail!(
                    "--sherpa-model {} provided but binary built without --features stt-sherpa; \
                     rebuild with `cargo install --features stt-sherpa,...` or remove the flag",
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
            tokens: Vec::new(),
        })
    }

    fn name(&self) -> &'static str {
        "stub-local-stt"
    }
}

/// CLI-local LLM dispatch over the two providers `scrybe record` can
/// pick at runtime. Same enum-dispatch shape as `CliStt`: variants stay
/// `Sized` so the existing `SessionInputs<L: LlmProvider>` generic
/// does not need a `?Sized` relaxation in `scrybe-core`.
enum CliLlm {
    Stub(StubLocalLlm),
    #[cfg(feature = "llm-openai-compat")]
    OpenAiCompat(OpenAiCompatLlmProvider),
}

#[async_trait]
impl LlmProvider for CliLlm {
    async fn complete(&self, prompt: &str) -> Result<String, LlmError> {
        match self {
            Self::Stub(p) => p.complete(prompt).await,
            #[cfg(feature = "llm-openai-compat")]
            Self::OpenAiCompat(p) => p.complete(prompt).await,
        }
    }

    fn name(&self) -> &str {
        match self {
            Self::Stub(p) => p.name(),
            #[cfg(feature = "llm-openai-compat")]
            Self::OpenAiCompat(p) => p.name(),
        }
    }
}

/// Construct the LLM provider from the `--llm` flag and the `[llm]`
/// config block.
///
/// - `LlmBackendArg::Stub` → `StubLocalLlm` (CI smoke + air-gapped
///   default).
/// - `LlmBackendArg::OpenAiCompat` + `--features llm-openai-compat`
///   → `OpenAiCompatLlmProvider::from_config(&cfg)` against the
///   user's `[llm]` block (defaults to Ollama at
///   `http://localhost:11434/v1`).
/// - `LlmBackendArg::OpenAiCompat` + no feature → hard error so the
///   user does not silently get the stub when they asked for real
///   summaries (mirrors `build_stt_provider`'s behavior at v1.0.1).
#[allow(unused_variables)]
fn build_llm_provider(
    backend: NotesBackend,
    cfg: &scrybe_core::config::LlmConfig,
) -> Result<CliLlm> {
    match backend {
        NotesBackend::Stub => Ok(CliLlm::Stub(StubLocalLlm::new())),
        NotesBackend::OpenAiCompat => {
            #[cfg(feature = "llm-openai-compat")]
            {
                let provider = OpenAiCompatLlmProvider::from_config(cfg)
                    .context("constructing OpenAI-compat LLM provider from [llm] config")?;
                Ok(CliLlm::OpenAiCompat(provider))
            }
            #[cfg(not(feature = "llm-openai-compat"))]
            {
                anyhow::bail!(
                    "--llm openai-compat requires the binary to be built with \
                     --features llm-openai-compat; rebuild with \
                     `cargo install --features llm-openai-compat,...` or pass \
                     --llm stub"
                );
            }
        }
    }
}

/// CLI-local stub LLM provider. Returns a fixed structured-notes body
/// so `notes.md` is well-formed. Real LLM access is via the
/// `OpenAiCompatLlmProvider` path wired in v1.0.4 (`--llm openai-compat`
/// + `--features llm-openai-compat`); see `build_llm_provider`.
struct StubLocalLlm;

impl StubLocalLlm {
    const fn new() -> Self {
        Self
    }
}

#[async_trait]
impl LlmProvider for StubLocalLlm {
    async fn complete(&self, prompt: &str) -> Result<String, LlmError> {
        if prompt.starts_with("Create a short, factual title") {
            return Ok("Synthetic Stub Session".to_string());
        }
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
    use scrybe_core::config::{Config, RecordConfig, RECORD_SOURCE_MIC, RECORD_SYSTEM_BACKEND_TAP};
    #[tokio::test]
    async fn test_capture_liveness_watchdog_stops_adapters_and_reports_timeout() {
        let registry = CaptureRegistry::default();
        let stops = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let observed_stops = Arc::clone(&stops);
        registry.register_stopper(move || {
            observed_stops.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Ok(())
        });

        let stalled = Box::pin(stream::pending::<Result<AudioFrame, CaptureError>>());
        let mut watchdog =
            capture_liveness_watchdog_with_timeout(stalled, registry, Duration::from_millis(10));

        let error = watchdog
            .next()
            .await
            .expect("watchdog error")
            .expect_err("timeout error");
        assert_eq!(error.to_string(), "platform API error: capture liveness watchdog expired after 30 seconds without a frame");
        assert_eq!(stops.load(std::sync::atomic::Ordering::SeqCst), 1);
        assert!(watchdog.next().await.is_none());
    }

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
            tokens: Vec::new(),
        }];
        let sys = vec![TranscriptChunk {
            text: "hello".into(),
            source: FrameSource::System,
            start_ms: 0,
            duration_ms: 1_000,
            language: None,
            tokens: Vec::new(),
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
            consent: Some(ConsentModeArg::Quick),
            synthetic_secs: 1,
            shell: false,
            source: Some(CaptureSourceArg::Synthetic),
            system_backend: None,
            llm: Some(LlmBackendArg::Stub),
            input_device: None,
            whisper_model: None,
            sherpa_model: None,
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
            consent: Some(ConsentModeArg::Quick),
            synthetic_secs: 1,
            shell: false,
            source: Some(CaptureSourceArg::Synthetic),
            system_backend: None,
            llm: Some(LlmBackendArg::Stub),
            input_device: None,
            whisper_model: None,
            sherpa_model: None,
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
    /// the `scrybe` package's `[package.metadata.dist]` block.
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
            consent: Some(ConsentModeArg::Quick),
            synthetic_secs: 1,
            shell: false,
            source: Some(CaptureSourceArg::Synthetic),
            system_backend: None,
            llm: Some(LlmBackendArg::Stub),
            input_device: None,
            whisper_model: None,
            sherpa_model: None,
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
    fn test_capture_source_arg_parses_mic_plus_system_token() {
        // Clap's `ValueEnum::from_str` is the surface users hit when
        // they type `--source mic+system`. The literal `+` is preserved
        // through the `#[value(name = "mic+system")]` attribute on the
        // enum variant. Asserts the parser accepts the documented
        // token and produces the expected variant.
        use clap::ValueEnum;
        let arg = CaptureSourceArg::from_str("mic+system", false)
            .expect("`mic+system` must parse to MicSystem");
        assert_eq!(arg, CaptureSourceArg::MicSystem);
    }

    #[test]
    fn test_capture_source_arg_rejects_typo_variants() {
        use clap::ValueEnum;
        // Common typos that should NOT silently parse to a variant.
        for bad in ["mic-system", "mic_system", "system", "system+mic"] {
            let r = CaptureSourceArg::from_str(bad, false);
            assert!(r.is_err(), "{bad} must not parse to any variant; got {r:?}");
        }
    }

    /// Resolution moved to `scrybe-application`, but the CLI's own
    /// claim — that `--system-backend` beats the file — is about this
    /// binary's flags, so it stays asserted here, through the path the
    /// binary now takes.
    #[test]
    fn test_system_backend_flag_overrides_record_config() {
        let config = Config {
            record: RecordConfig {
                system_backend: RECORD_SYSTEM_BACKEND_TAP.to_string(),
                ..RecordConfig::default()
            },
            ..Config::default()
        };
        let args = Args {
            system_backend: Some(SystemBackendArg::Sck),
            ..bare_args()
        };

        let plan = RecordingPlan::resolve(&config, None, &overrides_from(&args)).unwrap();

        assert_eq!(plan.system_backend, SystemBackend::Sck);
    }

    #[test]
    fn test_system_backend_uses_valid_record_config_then_default() {
        let tap = Config {
            record: RecordConfig {
                system_backend: RECORD_SYSTEM_BACKEND_TAP.to_string(),
                ..RecordConfig::default()
            },
            ..Config::default()
        };
        let overrides = overrides_from(&bare_args());

        assert_eq!(
            RecordingPlan::resolve(&tap, None, &overrides)
                .unwrap()
                .system_backend,
            SystemBackend::Tap
        );
        assert_eq!(
            RecordingPlan::resolve(&Config::default(), None, &overrides)
                .unwrap()
                .system_backend,
            SystemBackend::Sck
        );
    }

    #[test]
    fn test_tap_fallback_is_single_hop_to_sck() {
        assert_eq!(
            fallback_backend(SystemBackend::Tap),
            Some(SystemBackend::Sck)
        );
        assert_eq!(fallback_backend(SystemBackend::Sck), None);
    }

    fn system_frame(samples: &[f32], timestamp_ns: u64) -> AudioFrame {
        AudioFrame::from_slice(samples, 1, 16_000, timestamp_ns, FrameSource::System)
    }

    #[tokio::test]
    async fn test_silent_tap_startup_falls_back_without_dropping_frames() {
        let input = vec![
            Ok(system_frame(&[0.0, 0.0], 0)),
            Ok(system_frame(&[0.0, 0.0], 125_000)),
        ];

        let (active, frames) =
            tap_produces_nonzero_frames(Box::pin(futures::stream::iter(input))).await;
        let observed: Vec<_> = frames
            .map(|frame| {
                let frame = frame.unwrap();
                (frame.timestamp_ns, frame.samples.to_vec())
            })
            .collect()
            .await;

        assert!(!active);
        assert_eq!(
            observed,
            vec![(0, vec![0.0, 0.0]), (125_000, vec![0.0, 0.0])]
        );
    }

    #[tokio::test]
    async fn test_active_tap_startup_preserves_buffered_and_remaining_frames() {
        let input = vec![
            Ok(system_frame(&[0.0, 0.0], 0)),
            Ok(system_frame(&[0.25, 0.0], 125_000)),
            Ok(system_frame(&[0.5, 0.0], 250_000)),
        ];

        let (active, frames) =
            tap_produces_nonzero_frames(Box::pin(futures::stream::iter(input))).await;
        let observed: Vec<_> = frames
            .map(|frame| {
                let frame = frame.unwrap();
                (frame.timestamp_ns, frame.samples.to_vec())
            })
            .collect()
            .await;

        assert!(active);
        assert_eq!(
            observed,
            vec![
                (0, vec![0.0, 0.0]),
                (125_000, vec![0.25, 0.0]),
                (250_000, vec![0.5, 0.0]),
            ]
        );
    }

    #[test]
    fn test_fallback_diagnostic_uses_initial_session_folder() {
        let root = tempfile::tempdir().unwrap();
        let started_at = Utc::now();
        let id = SessionId::new();

        write_capture_diagnostic(
            root.path(),
            started_at,
            id,
            Some("Initial title"),
            "system capture switched from tap to sck after no tap startup activity",
        )
        .unwrap();

        let folder = root
            .path()
            .join(session_folder_name(started_at, "Initial title", id));
        assert_eq!(
            std::fs::read_to_string(folder.join("capture.log")).unwrap(),
            "system capture switched from tap to sck after no tap startup activity\n"
        );
    }

    #[cfg(not(all(feature = "mic-capture", feature = "system-capture-mac")))]
    #[tokio::test]
    async fn test_run_with_mic_system_source_errors_without_both_features() {
        // The MicSystem arm of the source match must hard-error when
        // either of the two underlying features is missing. Surfaces
        // as an `anyhow::Error` rooted in the bail! string so the user
        // sees the named features they need to rebuild with.
        std::env::set_var("SCRYBE_CONSENT_AUTO_ACCEPT", "1");
        let dir = tempfile::tempdir().unwrap();
        let cfg_dir = tempfile::tempdir().unwrap();
        std::env::set_var("SCRYBE_CONFIG", cfg_dir.path().join("absent.toml"));
        let result = run(Args {
            title: Some("ms-feature-gate".into()),
            root: Some(dir.path().to_path_buf()),
            yes: true,
            consent: Some(ConsentModeArg::Quick),
            synthetic_secs: 1,
            shell: false,
            source: Some(CaptureSourceArg::MicSystem),
            system_backend: None,
            llm: Some(LlmBackendArg::Stub),
            whisper_model: None,
            sherpa_model: None,
            input_device: None,
        })
        .await;
        let Err(err) = result else {
            panic!("MicSystem without both features must error");
        };
        let msg = format!("{err:?}");
        assert!(
            msg.contains("--source mic+system")
                && msg.contains("mic-capture")
                && msg.contains("system-capture-mac"),
            "error must name the source flag and both required features; got: {msg}"
        );
    }

    #[test]
    fn test_build_stt_provider_returns_stub_when_no_model_path_supplied() {
        let stt =
            build_stt_provider(TranscriptionModel::Stub, "en").expect("stub branch must succeed");
        assert_eq!(stt.name(), "stub-local-stt");
    }

    #[test]
    fn test_stub_provider_exposes_no_streaming_capability() {
        // The batch path must stay selected for providers that cannot
        // decode incrementally; only Sherpa answers this call.
        let stt =
            build_stt_provider(TranscriptionModel::Stub, "en").expect("stub branch must succeed");
        assert!(stt.streaming().is_none());
    }

    #[cfg(not(feature = "whisper-local"))]
    #[test]
    fn test_build_stt_provider_errors_when_whisper_model_supplied_without_feature() {
        let result = build_stt_provider(
            TranscriptionModel::Whisper(PathBuf::from("/tmp/no-such-model.bin")),
            "en",
        );
        let Err(err) = result else {
            panic!("flag without feature must error rather than silently stub");
        };
        let message = format!("{err:?}");
        assert!(
            message.contains("--whisper-model") && message.contains("--features whisper-local"),
            "error must name both the flag and the missing feature; got: {message}"
        );
    }

    #[cfg(not(feature = "stt-sherpa"))]
    #[test]
    fn test_build_stt_provider_errors_when_sherpa_model_supplied_without_feature() {
        let result = build_stt_provider(
            TranscriptionModel::Sherpa(PathBuf::from("/tmp/no-such-model")),
            "en",
        );
        let Err(err) = result else {
            panic!("flag without feature must error rather than silently stub");
        };
        let message = format!("{err:?}");
        assert!(
            message.contains("--sherpa-model") && message.contains("--features stt-sherpa"),
            "error must name both the flag and the missing feature; got: {message}"
        );
    }

    /// `--sherpa-model` beating a configured whisper path is a claim
    /// about this binary's flags, so it stays asserted here even though
    /// the ordering rule itself now lives in `scrybe-application`.
    #[test]
    fn test_explicit_sherpa_model_overrides_configured_whisper_model() {
        let config = Config {
            record: RecordConfig {
                source: RECORD_SOURCE_MIC.to_string(),
                whisper_model: Some(PathBuf::from("/models/whisper.bin")),
                ..RecordConfig::default()
            },
            ..Config::default()
        };
        let args = Args {
            sherpa_model: Some(PathBuf::from("/models/sherpa")),
            ..bare_args()
        };

        let plan = RecordingPlan::resolve(&config, None, &overrides_from(&args)).unwrap();

        assert_eq!(
            plan.transcription,
            TranscriptionModel::Sherpa(PathBuf::from("/models/sherpa"))
        );
    }

    /// The flags a test builds a plan from, with every override absent.
    fn bare_args() -> Args {
        Args {
            title: None,
            root: None,
            yes: false,
            consent: None,
            synthetic_secs: 5,
            source: None,
            input_device: None,
            system_backend: None,
            whisper_model: None,
            sherpa_model: None,
            llm: None,
            shell: false,
        }
    }

    #[cfg(feature = "whisper-local")]
    #[test]
    fn test_build_stt_provider_rejects_partial_whisper_model_path() {
        let dir = tempfile::tempdir().unwrap();
        let partial = dir.path().join("ggml-tiny.bin.partial");
        std::fs::write(&partial, b"unfinished download").unwrap();
        let result = build_stt_provider(TranscriptionModel::Whisper(partial), "en");
        let Err(err) = result else {
            panic!("partial paths must be rejected at construction");
        };
        let message = format!("{err:?}");
        assert!(
            message.contains("loading whisper.cpp model"),
            "context chain must mention the loading step; got: {message}"
        );
    }

    #[test]
    fn test_build_llm_provider_returns_stub_when_backend_is_stub() {
        let cfg = scrybe_core::config::LlmConfig::default();

        let llm = build_llm_provider(NotesBackend::Stub, &cfg)
            .expect("stub branch must succeed regardless of features");

        assert_eq!(llm.name(), "stub-local-llm");
    }

    #[cfg(not(feature = "llm-openai-compat"))]
    #[test]
    fn test_build_llm_provider_errors_when_openai_compat_requested_without_feature() {
        let cfg = scrybe_core::config::LlmConfig::default();

        let result = build_llm_provider(NotesBackend::OpenAiCompat, &cfg);

        let Err(err) = result else {
            panic!("openai-compat without feature must error rather than silently stub");
        };
        let msg = format!("{err:?}");
        assert!(
            msg.contains("--llm openai-compat") && msg.contains("--features llm-openai-compat"),
            "error must name both the flag and the missing feature; got: {msg}"
        );
    }

    #[cfg(feature = "llm-openai-compat")]
    #[test]
    fn test_build_llm_provider_constructs_openai_compat_when_feature_enabled() {
        // Construction does not exercise the network (the inner reqwest
        // Client is built but no request is dispatched). We assert the
        // returned variant reports a `<provider>:<model>` name so the
        // [llm] config block flowed through `from_config` correctly.
        let cfg = scrybe_core::config::LlmConfig {
            provider: "ollama".into(),
            model: "llama3.1:8b".into(),
            ..scrybe_core::config::LlmConfig::default()
        };

        let llm = build_llm_provider(NotesBackend::OpenAiCompat, &cfg)
            .expect("openai-compat branch must succeed when feature is on");

        assert_eq!(llm.name(), "ollama:llama3.1:8b");
    }

    #[test]
    fn test_llm_backend_arg_default_is_stub() {
        assert_eq!(LlmBackendArg::default(), LlmBackendArg::Stub);
    }
    /// The process-wide controller, taken from the composition root
    /// exactly as `run` does. There is no other way to obtain one.
    fn app_controller(root: &std::path::Path) -> Arc<RecordingController> {
        Arc::clone(application(Some(root)).unwrap().recording())
    }

    /// `Args` for a synthetic one-second session under `root`.
    fn synthetic_args(root: PathBuf) -> Args {
        Args {
            title: Some("labelling".into()),
            root: Some(root),
            yes: true,
            consent: Some(ConsentModeArg::Quick),
            synthetic_secs: 1,
            shell: false,
            source: Some(CaptureSourceArg::Synthetic),
            system_backend: None,
            llm: Some(LlmBackendArg::Stub),
            input_device: None,
            whisper_model: None,
            sherpa_model: None,
        }
    }

    /// The preflight runs before anything is written, so a refusal is
    /// labelled `Preflight` — the kind whose documented meaning is that
    /// no journal exists to recover — and leaves the filesystem as it
    /// found it. Asserted on the filesystem, not on the returned error.
    #[test]
    fn test_a_preflight_failure_is_labelled_preflight_and_leaves_nothing_on_disk() {
        let cfg_dir = tempfile::tempdir().unwrap();
        std::env::set_var("SCRYBE_CONFIG", cfg_dir.path().join("no-such-config.toml"));
        let dir = tempfile::tempdir().unwrap();
        // A regular file where the storage root's parent would have to
        // be a directory.
        let blocker = dir.path().join("not-a-directory");
        std::fs::write(&blocker, b"").unwrap();
        let root = blocker.join("sessions");

        let controller = app_controller(dir.path());
        let kinds = Arc::new(std::sync::Mutex::new(Vec::new()));
        let recorded = Arc::clone(&kinds);
        controller.subscribe(Arc::new(move |event| {
            if let Some(failure) = &event.failure {
                recorded.lock().unwrap().push(failure.kind);
            }
        }));

        let result = begin_recording(&controller, &synthetic_args(root.clone()));

        assert!(result.is_err());
        assert_eq!(
            *kinds.lock().unwrap(),
            vec![scrybe_application::recording::RecordingFailureKind::Preflight]
        );
        // Settled, so the next recording can start.
        assert_eq!(controller.snapshot().state, RecordingState::Idle);
        assert!(!root.exists(), "a failed preflight must write nothing");
        assert!(
            !blocker.is_dir(),
            "a failed preflight must not have replaced the blocker"
        );
    }

    #[tokio::test]
    async fn test_a_finalization_failure_after_a_natural_capture_end_is_not_labelled_capture() {
        let cfg_dir = tempfile::tempdir().unwrap();
        std::env::set_var("SCRYBE_CONFIG", cfg_dir.path().join("no-such-config.toml"));
        let dir = tempfile::tempdir().unwrap();

        let controller = app_controller(dir.path());
        let args = synthetic_args(dir.path().to_path_buf());
        let plan = begin_recording(&controller, &args).unwrap();
        let (_stop_tx, stop_rx) = watch::channel(false);
        // `--synthetic-secs` ends capture on its own, so nothing ever
        // requests a stop. The controller must still have entered
        // `Saving` by the time finalization runs; otherwise a failure
        // while merging, encoding, transcribing, or writing
        // `meta.toml` would be labelled `Capture` even though audio
        // exists and the session is repairable.
        run_with_stop(args, plan, stop_rx, Some(Arc::clone(&controller)))
            .await
            .unwrap();

        // Capture ended on its own — nothing requested a stop — yet
        // finalization must still have run in `Saving`. Otherwise a
        // failure while merging, encoding, transcribing, or writing
        // `meta.toml` would be labelled `Capture` even though audio
        // exists and the session is repairable.
        assert_eq!(controller.snapshot().state, RecordingState::Saving);
        assert_eq!(
            controller
                .fail(RECORDING_FAILURE_SUMMARY)
                .unwrap()
                .failure
                .map(|failure| failure.kind),
            Some(scrybe_application::recording::RecordingFailureKind::Finalization)
        );
    }

    #[tokio::test]
    async fn test_a_capture_side_failure_is_not_labelled_finalization() {
        let dir = tempfile::tempdir().unwrap();

        let controller = app_controller(dir.path());
        controller.begin_preparing().unwrap();
        controller.mark_recording().unwrap();

        // A capture-side error that surfaces before any finalization
        // event must not be labelled `Finalization`.
        let failed = controller.fail(RECORDING_FAILURE_SUMMARY).unwrap();

        assert_eq!(
            failed.failure.map(|failure| failure.kind),
            Some(scrybe_application::recording::RecordingFailureKind::Capture)
        );
    }

    #[test]
    fn test_no_serialized_failure_carries_a_path_or_provider_name() {
        let dir = tempfile::tempdir().unwrap();
        let controller = app_controller(dir.path());
        controller.begin_preparing().unwrap();

        // The shapes the outermost `anyhow` context really takes on
        // this path: an absolute storage root, a notes model path and
        // provider name, and an input-device identity.
        let error = anyhow::anyhow!("connection refused")
            .context("loading notes model /Users/someone/Library/scrybe/qwen3-8b.gguf")
            .context("resolving input device MacBook Pro Microphone (openai-compat)")
            .context("creating storage root /Users/someone/Meetings/scrybe");
        let snapshot = settle_failure(&controller, &error).expect("preparing can fail");
        let encoded = serde_json::to_string(&snapshot).unwrap();

        // Asserting on the key set alone could never catch this; the
        // leak was always in the values.
        assert!(!encoded.contains('/'), "a path reached an event: {encoded}");
        for secret in ["Users", "qwen3", "openai-compat", "MacBook", "gguf"] {
            assert!(
                !encoded.contains(secret),
                "{secret} reached an event: {encoded}"
            );
        }
        assert!(encoded.contains(RECORDING_FAILURE_SUMMARY));
    }
}
