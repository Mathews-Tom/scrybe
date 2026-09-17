// Copyright 2026 Mathews Tom
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//     https://www.apache.org/licenses/LICENSE-2.0

//! Running one recording, from a resolved plan to a durable session.
//!
//! The providers a plan names and the call into `scrybe_core::session`
//! live here, so a desktop host and a terminal produce the same session
//! from the same configuration rather than each assembling its own.
//! Before this they were binary-private in the command-line recorder.
//!
//! What stays with the caller is the capture stream and the consent
//! prompter. The stream is per-platform — the adapters are in crates
//! this one does not depend on — and consent is per-surface: a terminal
//! asks on a TTY and a window cannot.

use std::sync::Arc;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use futures::stream::{self, Stream, StreamExt};
use scrybe_core::config::Config;
use scrybe_core::consent::ConsentPrompter;
use scrybe_core::context::MeetingContext;
use scrybe_core::diarize::BinaryChannelDiarizer;
use scrybe_core::error::{CaptureError, CoreError, LlmError, SttError};
use scrybe_core::hooks::Hook;
use scrybe_core::notes_map_reduce::NotesRuntime;
use scrybe_core::pipeline::chunker::ChunkerConfig;
use scrybe_core::pipeline::vad::EnergyVad;
use scrybe_core::providers::streaming::StreamingSttProvider;
use scrybe_core::providers::{LlmProvider, SttProvider};
use scrybe_core::session::{SessionInputs, SessionOutputs, SessionProgress};
use scrybe_core::types::{AudioChunk, AudioFrame, FrameSource, SessionId, TranscriptChunk};

use crate::error::{ApplicationError, ErrorCode};
use crate::recording::controller::RecordingController;
use crate::recording::plan::{CaptureSource, NotesBackend, RecordingPlan, TranscriptionModel};
use crate::recording::progress::{observe, RecordingProgressObserver};
use crate::Result;

/// How the chunker cuts a live stream into transcribable segments.
///
/// One policy, not a per-frontend choice: two surfaces recording the
/// same meeting with different boundaries would produce transcripts
/// that cannot be compared.
const CHUNKER: ChunkerConfig = ChunkerConfig {
    max_chunk: std::time::Duration::from_secs(30),
    min_speech_before_silence_split: std::time::Duration::from_secs(5),
    silence_split_after: std::time::Duration::from_secs(5),
};

/// Everything one session run needs that the plan does not already say.
pub struct RecordingRun<'a, P: ConsentPrompter> {
    pub plan: &'a RecordingPlan,
    pub config: &'a Config,
    /// Minted by the caller, because a capture diagnostic written
    /// before the session starts has to name the folder the session
    /// will create.
    pub id: SessionId,
    pub started_at: DateTime<Utc>,
    /// Attributed to the session in `meta.toml`.
    pub user: String,
    /// How this surface asks for consent.
    pub prompter: &'a P,
    /// Driven from the pipeline's own boundaries. `None` for a caller
    /// with no state model to drive.
    pub controller: Option<Arc<RecordingController>>,
    /// Told each time saving moves a step. This is what a recording
    /// indicator renders from: it carries a step, an index, and a
    /// total, and nothing that was said.
    pub on_progress: Option<Arc<RecordingProgressObserver>>,
    /// Handed the pipeline's own progress stream, unfiltered.
    ///
    /// **It carries transcript text.** `SessionProgress` has a variant
    /// holding a transcribed line, and this is the only route by which
    /// it leaves the pipeline. It exists for the command-line
    /// recorder's live transcript, which prints to a terminal the
    /// reader is already watching. A tray, a pill, a window event, or a
    /// log must use `on_progress` instead, which cannot carry one.
    pub on_session_event: Option<Arc<dyn Fn(SessionProgress) + Send + Sync>>,
}

/// Runs one recording to a durable session.
///
/// Creates the storage root if it is absent — the first thing on this
/// path that writes anything, and deliberately after the preflight that
/// refused would have refused.
///
/// # Errors
///
/// [`ErrorCode::StorageUnavailable`] when the storage root cannot be
/// created, [`ErrorCode::ProviderUnavailable`] when a provider the plan
/// names cannot be built, and [`ErrorCode::RecordingFailed`] for any
/// capture, transcription, notes, consent, or storage failure the
/// session itself reports.
pub async fn run<C, P>(inputs: RecordingRun<'_, P>, capture: C) -> Result<SessionOutputs>
where
    C: Stream<Item = std::result::Result<AudioFrame, CaptureError>> + Send + Unpin,
    P: ConsentPrompter,
{
    let RecordingRun {
        plan,
        config,
        id,
        started_at,
        user,
        prompter,
        controller,
        on_progress,
        on_session_event,
    } = inputs;

    std::fs::create_dir_all(&plan.root).map_err(|source| {
        ApplicationError::new(
            ErrorCode::StorageUnavailable,
            format!(
                "the storage root {} could not be created",
                plan.root.display()
            ),
        )
        .with_source(source)
    })?;

    let transcription = transcription(plan, &config.stt.language)?;
    let notes = notes(plan, &config.llm)?;
    let notes_runtime = match plan.notes {
        NotesBackend::Stub => None,
        NotesBackend::OpenAiCompat => {
            Some(NotesRuntime::load(&config.notes).map_err(|source| {
                ApplicationError::new(
                    ErrorCode::ProviderUnavailable,
                    "the notes runtime named under [notes] could not be loaded",
                )
                .with_source(source)
            })?)
        }
    };
    let progress = observe(controller, on_progress, move |event| {
        if let Some(sink) = on_session_event.as_deref() {
            sink(event);
        }
    });
    let diarizer = BinaryChannelDiarizer;
    let hooks: Vec<Box<dyn Hook>> = Vec::new();
    let streaming = transcription.streaming();

    scrybe_core::session::run_with_notes(
        SessionInputs {
            id,
            started_at,
            root: plan.root.clone(),
            title: plan.title.clone(),
            user,
            consent_mode: plan.consent,
            context: MeetingContext {
                title: plan.title.clone(),
                ..MeetingContext::default()
            },
            mic_vad: EnergyVad::default(),
            // A second detector, on the system channel only, is what
            // lets the binary-channel diarizer attribute the far side
            // of a meeting as `Them:`. A source that carries no system
            // frames has nothing for it to look at.
            system_vad: plan.source.uses_system_audio().then(EnergyVad::default),
            streaming_stt: streaming,
            stt: &transcription,
            llm: &notes,
            diarizer: &diarizer,
            prompter,
            hooks: &hooks,
            chunker_config: CHUNKER,
            // The offline merge's duration assertion is a genuine
            // safety net for a real capture device, whose frames arrive
            // at real wall-clock pace. The synthetic source generates
            // frames in-process with no real-time pacing, so comparing
            // its encoded duration against elapsed wall-clock time
            // would fail by construction on every invocation rather
            // than because anything is wrong.
            verify_duration: plan.source != CaptureSource::Synthetic,
            progress: Some(&progress),
        },
        capture,
        notes_runtime,
    )
    .await
    .map_err(session_failure)
}

/// The session's own failure, narrowed.
///
/// One code and one line the service layer wrote. `CoreError` carries
/// paths, provider names, and device identities through its source
/// chain; that chain is kept as the error's source — where a log and a
/// caller can reach it — and out of the message a surface renders.
fn session_failure(source: CoreError) -> ApplicationError {
    ApplicationError::new(ErrorCode::RecordingFailed, "the recording session failed")
        .with_source(source)
}

/// Deterministic in-process frames: `seconds` of a 440 Hz tone followed
/// by six seconds of silence.
///
/// The one capture source that needs no hardware, no permission grant,
/// and no adapter, which is what makes a recording path exercisable on
/// a machine that has none of them.
pub fn synthetic_frames(
    seconds: u64,
) -> impl Stream<Item = std::result::Result<AudioFrame, CaptureError>> + Send + Unpin {
    const SAMPLE_RATE: u32 = 16_000;
    const FRAME_SAMPLES: usize = 1_600;
    const SILENCE_SECONDS: u64 = 6;
    let per_second = u64::from(SAMPLE_RATE) / FRAME_SAMPLES as u64;
    let speech_frames = seconds * per_second;
    let total = speech_frames + SILENCE_SECONDS * per_second;

    Box::pin(stream::iter(0..total).map(move |index| {
        let speech = index < speech_frames;
        let samples: Vec<f32> = (0..FRAME_SAMPLES)
            .map(|offset| {
                if speech {
                    #[allow(
                        clippy::cast_precision_loss,
                        reason = "a tone's phase at 16 kHz stays far inside f32's exact range"
                    )]
                    let t =
                        (index * FRAME_SAMPLES as u64 + offset as u64) as f32 / SAMPLE_RATE as f32;
                    (t * 440.0 * std::f32::consts::TAU).sin()
                } else {
                    0.0
                }
            })
            .collect();
        Ok(AudioFrame {
            samples: Arc::from(samples),
            channels: 1,
            sample_rate: SAMPLE_RATE,
            timestamp_ns: (index * FRAME_SAMPLES as u64 * 1_000_000_000) / u64::from(SAMPLE_RATE),
            source: FrameSource::Mic,
        })
    }))
}

/// The transcription providers a plan can name.
///
/// An enum rather than a boxed trait object: the variants stay `Sized`,
/// so `SessionInputs`' `S: SttProvider` needs no `?Sized` relaxation in
/// `scrybe-core`.
pub enum Transcription {
    Stub(StubTranscription),
    #[cfg(feature = "stt-sherpa")]
    Sherpa(scrybe_core::providers::sherpa_streaming::SherpaStreamingProvider),
    #[cfg(feature = "whisper-local")]
    Whisper(scrybe_core::providers::whisper_local::WhisperLocalProvider),
}

impl Transcription {
    /// The streaming capability of the selected provider, when it has
    /// one.
    ///
    /// Only the Sherpa provider decodes incrementally; the stub and
    /// whisper.cpp are batch-only and take the unchanged path.
    #[must_use]
    pub fn streaming(&self) -> Option<&dyn StreamingSttProvider> {
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
impl SttProvider for Transcription {
    async fn transcribe(
        &self,
        chunk: AudioChunk,
    ) -> std::result::Result<TranscriptChunk, SttError> {
        match self {
            Self::Stub(provider) => provider.transcribe(chunk).await,
            #[cfg(feature = "stt-sherpa")]
            Self::Sherpa(provider) => provider.transcribe(chunk).await,
            #[cfg(feature = "whisper-local")]
            Self::Whisper(provider) => provider.transcribe(chunk).await,
        }
    }

    fn name(&self) -> &str {
        match self {
            Self::Stub(provider) => provider.name(),
            #[cfg(feature = "stt-sherpa")]
            Self::Sherpa(provider) => provider.name(),
            #[cfg(feature = "whisper-local")]
            Self::Whisper(provider) => provider.name(),
        }
    }
}

/// The notes providers a plan can name.
pub enum Notes {
    Stub(StubNotes),
    #[cfg(feature = "notes-generation")]
    OpenAiCompat(scrybe_core::providers::openai_compat_llm::OpenAiCompatLlmProvider),
}

#[async_trait]
impl LlmProvider for Notes {
    async fn complete(&self, prompt: &str) -> std::result::Result<String, LlmError> {
        match self {
            Self::Stub(provider) => provider.complete(prompt).await,
            #[cfg(feature = "notes-generation")]
            Self::OpenAiCompat(provider) => provider.complete(prompt).await,
        }
    }

    fn name(&self) -> &str {
        match self {
            Self::Stub(provider) => provider.name(),
            #[cfg(feature = "notes-generation")]
            Self::OpenAiCompat(provider) => provider.name(),
        }
    }
}

/// Builds the transcription provider `plan` names.
///
/// A model the build carries no runtime for is refused rather than
/// quietly replaced by the stub: a reader who configured a model and
/// got the stub's fixed line would have a transcript that is not of
/// their meeting and no indication of it.
///
/// # Errors
///
/// [`ErrorCode::ProviderUnavailable`] when the build carries no runtime
/// for the named model, or when the model itself fails to load.
pub fn transcription(plan: &RecordingPlan, language: &str) -> Result<Transcription> {
    match &plan.transcription {
        TranscriptionModel::Stub => Ok(Transcription::Stub(StubTranscription)),
        TranscriptionModel::Whisper(path) => {
            #[cfg(feature = "whisper-local")]
            {
                let mut config =
                    scrybe_core::providers::whisper_local::WhisperLocalConfig::new(path.clone());
                config.language = language.to_string();
                scrybe_core::providers::whisper_local::WhisperLocalProvider::new(config)
                    .map(Transcription::Whisper)
                    .map_err(|source| {
                        unavailable(format!(
                            "the whisper.cpp model at {} could not be loaded",
                            path.display()
                        ))
                        .with_source(source)
                    })
            }
            #[cfg(not(feature = "whisper-local"))]
            {
                let _ = language;
                Err(unavailable(format!(
                    "a whisper.cpp model is configured at {}, but this build carries no whisper runtime",
                    path.display()
                )))
            }
        }
        TranscriptionModel::Sherpa(path) => {
            #[cfg(feature = "stt-sherpa")]
            {
                let _ = language;
                scrybe_core::providers::sherpa_streaming::SherpaStreamingProvider::new(
                    scrybe_core::providers::sherpa_streaming::SherpaStreamingConfig::new(
                        path.clone(),
                    ),
                )
                .map(Transcription::Sherpa)
                .map_err(|source| {
                    unavailable(format!(
                        "the streaming Sherpa-ONNX model at {} could not be loaded",
                        path.display()
                    ))
                    .with_source(source)
                })
            }
            #[cfg(not(feature = "stt-sherpa"))]
            {
                let _ = language;
                Err(unavailable(format!(
                    "a Sherpa-ONNX model is configured at {}, but this build carries no Sherpa runtime",
                    path.display()
                )))
            }
        }
    }
}

/// Builds the notes provider `plan` names.
///
/// # Errors
///
/// [`ErrorCode::ProviderUnavailable`] when the plan names a provider
/// this build carries no transport for, or when the `[llm]` block does
/// not describe a usable endpoint.
pub fn notes(plan: &RecordingPlan, config: &scrybe_core::config::LlmConfig) -> Result<Notes> {
    match plan.notes {
        NotesBackend::Stub => Ok(Notes::Stub(StubNotes)),
        NotesBackend::OpenAiCompat => {
            #[cfg(feature = "notes-generation")]
            {
                scrybe_core::providers::openai_compat_llm::OpenAiCompatLlmProvider::from_config(
                    config,
                )
                .map(Notes::OpenAiCompat)
                .map_err(|source| {
                    unavailable("the [llm] block does not describe a usable endpoint")
                        .with_source(source)
                })
            }
            #[cfg(not(feature = "notes-generation"))]
            {
                let _ = config;
                Err(unavailable(
                    "[record].llm names openai-compat, but this build carries no notes provider",
                ))
            }
        }
    }
}

fn unavailable(message: impl Into<String>) -> ApplicationError {
    ApplicationError::new(ErrorCode::ProviderUnavailable, message)
}

/// The transcription used when no model is configured.
///
/// Emits a line that says what it is, so a transcript produced without
/// a model cannot be mistaken for one produced with it.
pub struct StubTranscription;

#[async_trait]
impl SttProvider for StubTranscription {
    async fn transcribe(
        &self,
        chunk: AudioChunk,
    ) -> std::result::Result<TranscriptChunk, SttError> {
        let speech = chunk.samples.iter().any(|sample| sample.abs() > 0.01);
        Ok(TranscriptChunk {
            text: if speech {
                "[synthetic speech chunk; configure a transcription model for real transcription]"
                    .to_string()
            } else {
                "[silence]".to_string()
            },
            source: chunk.source,
            start_ms: u64::try_from(chunk.start.as_millis()).unwrap_or(u64::MAX),
            duration_ms: u64::try_from(chunk.duration.as_millis()).unwrap_or(u64::MAX),
            language: None,
            tokens: Vec::new(),
        })
    }

    fn name(&self) -> &'static str {
        "stub-local-stt"
    }
}

/// The notes used when no provider is configured.
///
/// Returns a well-formed body so `notes.md` has the shape every reader
/// of a session expects, and says in it that no provider wrote it.
pub struct StubNotes;

#[async_trait]
impl LlmProvider for StubNotes {
    async fn complete(&self, prompt: &str) -> std::result::Result<String, LlmError> {
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

/// A consent prompter for a surface that has already decided.
///
/// A window cannot raise a terminal prompt, and the decision it carries
/// was made before the recording was requested. It is not a way to skip
/// consent: a `false` refuses the recording, exactly as a declined
/// terminal prompt does, and the attestation the session writes is the
/// same one either way.
pub struct SettledConsent {
    accepted: bool,
}

impl SettledConsent {
    /// A prompter that answers `accepted`.
    #[must_use]
    pub const fn new(accepted: bool) -> Self {
        Self { accepted }
    }
}

#[async_trait]
impl ConsentPrompter for SettledConsent {
    async fn prompt(
        &self,
        _mode: scrybe_core::types::ConsentMode,
    ) -> std::result::Result<(), scrybe_core::error::ConsentError> {
        if self.accepted {
            Ok(())
        } else {
            Err(scrybe_core::error::ConsentError::UserAborted)
        }
    }
}

/// Which progress events this module forwards, kept honest by the one
/// consumer that can see the difference.
#[cfg(test)]
mod tests {
    use super::*;

    /// The pipeline's progress stream carries one variant holding a
    /// transcribed line. If it could become a saving step, every
    /// surface rendering progress would be rendering what the meeting
    /// said.
    #[test]
    fn test_no_saving_step_is_derived_from_an_event_carrying_transcript_text() {
        let chunk = scrybe_core::types::AttributedChunk {
            chunk: TranscriptChunk {
                text: "the number is four".to_string(),
                source: FrameSource::Mic,
                start_ms: 0,
                duration_ms: 1_000,
                language: None,
                tokens: Vec::new(),
            },
            speaker: scrybe_core::types::SpeakerLabel::Me,
        };

        assert_eq!(
            crate::recording::SavingStep::of(&SessionProgress::TranscriptAccepted(chunk)),
            None
        );
    }
}
