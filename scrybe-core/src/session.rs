// Copyright 2026 Mathews Tom
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//     https://www.apache.org/licenses/LICENSE-2.0

//! Session orchestrator.
//!
//! Wires the pipeline stages from `docs/system-design.md` §5: consent →
//! capture → channel split → VAD/chunker → resample → STT → diarize →
//! transcript append + audio encode → on stop: LLM prompt → notes write →
//! meta.toml write → hook dispatch.
//!
//! The orchestrator is generic over the capture, STT, LLM, and diarizer
//! seams so library consumers (CLI, Android shell, future GUI) wire in
//! their own implementations. Tests inject deterministic fakes through
//! the same generic surface — no `dyn` indirection in the hot path.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, Utc};
use futures::stream::{Stream, StreamExt};
use serde::{Deserialize, Serialize};
use tracing::{debug, warn};

use crate::consent::ConsentPrompter;
use crate::context::MeetingContext;
use crate::diarize::Diarizer;
use crate::error::{CoreError, PipelineError};
use crate::hooks::{dispatch_hooks, Hook, LifecycleEvent};
use crate::notes;
use crate::pipeline::chunker::{ChunkBoundary, Chunker, ChunkerConfig, EmittedChunk};
use crate::pipeline::encoder::{Encoder, NullEncoder};
use crate::pipeline::resample::resample_linear;
use crate::pipeline::vad::Vad;
use crate::providers::{LlmProvider, SttProvider};
use crate::storage::{
    acquire_session_lock, append_durable, atomic_replace, release_session_lock,
    session_folder_name, write_stignore_template,
};
use crate::types::{
    AttributedChunk, AudioChunk, AudioFrame, ConsentAttestation, ConsentMode, FrameSource,
    SessionId, SpeakerLabel,
};

/// Target rate for STT input. Whisper's native rate is 16 kHz.
pub const STT_SAMPLE_RATE: u32 = 16_000;

/// Inputs the orchestrator needs from the caller. The caller owns
/// every value here so the orchestrator never touches global state.
pub struct SessionInputs<'a, V, S, L, D, P>
where
    V: Vad,
    S: SttProvider,
    L: LlmProvider,
    D: Diarizer,
    P: ConsentPrompter,
{
    pub id: SessionId,
    pub started_at: DateTime<Utc>,
    pub root: PathBuf,
    pub title: Option<String>,
    pub user: String,
    pub consent_mode: ConsentMode,
    pub context: MeetingContext,
    pub mic_vad: V,
    pub system_vad: Option<V>,
    pub stt: &'a S,
    pub llm: &'a L,
    pub diarizer: &'a D,
    pub prompter: &'a P,
    pub hooks: &'a [Box<dyn Hook>],
    pub chunker_config: ChunkerConfig,
}

/// Outputs the orchestrator surfaces to the caller. Paths are owned so
/// the CLI can render them without reaching back into the session
/// folder.
#[derive(Debug)]
pub struct SessionOutputs {
    pub folder: PathBuf,
    pub transcript_path: PathBuf,
    pub notes_path: PathBuf,
    pub meta_path: PathBuf,
    pub audio_path: PathBuf,
    pub attestation: ConsentAttestation,
    pub chunks: Vec<AttributedChunk>,
}

/// Run a session end-to-end. The capture stream is consumed; the
/// orchestrator returns once the stream closes (caller stops the
/// adapter externally).
///
/// # Errors
///
/// `CoreError::Consent` if the prompter declines, `CoreError::Storage`
/// for filesystem failures, `CoreError::Stt` / `CoreError::Llm` for
/// provider failures that exhaust retries.
pub async fn run<C, V, S, L, D, P>(
    inputs: SessionInputs<'_, V, S, L, D, P>,
    capture_stream: C,
) -> Result<SessionOutputs, CoreError>
where
    C: Stream<Item = Result<AudioFrame, crate::error::CaptureError>> + Send + Unpin,
    V: Vad,
    S: SttProvider,
    L: LlmProvider,
    D: Diarizer,
    P: ConsentPrompter,
{
    let SessionInputs {
        id,
        started_at,
        root,
        title,
        user,
        consent_mode,
        context,
        mic_vad,
        system_vad,
        stt,
        llm,
        diarizer,
        prompter,
        hooks,
        chunker_config,
    } = inputs;

    let attestation = crate::consent::run(consent_mode, user, prompter).await?;

    let folder_name = session_folder_name(started_at, title.as_deref().unwrap_or(""), id);
    let folder = root.join(folder_name);
    std::fs::create_dir_all(&folder).map_err(|e| CoreError::Storage(e.into()))?;

    let lock_path = acquire_session_lock(&folder, std::process::id())?;
    write_stignore_template(&folder)?;

    let outcome = drive_session(
        DriveInputs {
            id,
            started_at,
            folder: folder.clone(),
            title: title.clone(),
            context: context.clone(),
            attestation: attestation.clone(),
            mic_vad,
            system_vad,
            stt,
            llm,
            diarizer,
            hooks,
            chunker_config,
        },
        capture_stream,
    )
    .await;

    // Surface lock-release failures via tracing so a stale lockfile
    // has a paper trail; the session result still takes precedence.
    if let Err(e) = release_session_lock(&lock_path) {
        warn!(?e, "failed to release per-session pid lock");
    }

    outcome
}

struct DriveInputs<'a, V, S, L, D>
where
    V: Vad,
    S: SttProvider,
    L: LlmProvider,
    D: Diarizer,
{
    id: SessionId,
    started_at: DateTime<Utc>,
    folder: PathBuf,
    title: Option<String>,
    context: MeetingContext,
    attestation: ConsentAttestation,
    mic_vad: V,
    system_vad: Option<V>,
    stt: &'a S,
    llm: &'a L,
    diarizer: &'a D,
    hooks: &'a [Box<dyn Hook>],
    chunker_config: ChunkerConfig,
}

#[allow(clippy::too_many_lines)]
async fn drive_session<C, V, S, L, D>(
    inputs: DriveInputs<'_, V, S, L, D>,
    mut capture_stream: C,
) -> Result<SessionOutputs, CoreError>
where
    C: Stream<Item = Result<AudioFrame, crate::error::CaptureError>> + Send + Unpin,
    V: Vad,
    S: SttProvider,
    L: LlmProvider,
    D: Diarizer,
{
    let DriveInputs {
        id,
        started_at,
        folder,
        title,
        context,
        attestation,
        mic_vad,
        system_vad,
        stt,
        llm,
        diarizer,
        hooks,
        chunker_config,
    } = inputs;

    let context_arc = Arc::new(context.clone());

    let transcript_path = folder.join("transcript.md");
    let notes_path = folder.join("notes.md");
    let meta_path = folder.join("meta.toml");
    let audio_path = folder.join("audio.opus");

    let header = notes::render_transcript_header(title.as_deref(), started_at, None);
    append_durable(&transcript_path, header.as_bytes())?;

    dispatch_hooks(
        hooks,
        &LifecycleEvent::SessionStart {
            id,
            ctx: Arc::clone(&context_arc),
        },
    )
    .await;
    dispatch_hooks(
        hooks,
        &LifecycleEvent::ConsentRecorded {
            id,
            attestation: attestation.clone(),
        },
    )
    .await;

    let mut mic_chunker = Chunker::new(chunker_config, mic_vad, FrameSource::Mic);
    let mut system_chunker =
        system_vad.map(|v| Chunker::new(chunker_config, v, FrameSource::System));
    let mut audio_encoder = NullEncoder::new(crate::pipeline::encoder::EncoderConfig::default());

    let mut mic_text_chunks: Vec<crate::types::TranscriptChunk> = Vec::new();
    let mut sys_text_chunks: Vec<crate::types::TranscriptChunk> = Vec::new();

    while let Some(frame_result) = capture_stream.next().await {
        let frame = frame_result?;
        let mut chunks_for_stt: Vec<EmittedChunk> = Vec::new();
        let mut sink = |c: EmittedChunk| chunks_for_stt.push(c);
        match frame.source {
            FrameSource::System => {
                if let Some(c) = system_chunker.as_mut() {
                    c.push(frame.clone(), &mut sink);
                }
            }
            FrameSource::Mic | FrameSource::Mixed => {
                mic_chunker.push(frame.clone(), &mut sink);
            }
        }
        let pcm_for_audio = frame.samples.as_ref().to_vec();
        let page = audio_encoder
            .push_pcm(&pcm_for_audio)
            .map_err(CoreError::Pipeline)?;
        if !page.is_empty() {
            // Per docs/system-design.md §8.3: audio is the source of
            // truth, so each completed page is durably appended with
            // fdatasync on Unix / FlushFileBuffers on Windows. A crash
            // mid-session loses at most the most recent partial page.
            append_durable(&audio_path, &page)?;
        }

        for chunk in chunks_for_stt {
            if let Some(result) =
                process_chunk(chunk, stt, &transcript_path, id, hooks, diarizer).await?
            {
                match result.target {
                    StoreTarget::Mic => mic_text_chunks.push(result.text),
                    StoreTarget::System => sys_text_chunks.push(result.text),
                }
            }
        }
    }

    let mut tail_for_stt: Vec<EmittedChunk> = Vec::new();
    {
        let mut sink = |c: EmittedChunk| tail_for_stt.push(c);
        mic_chunker.finish(&mut sink);
        if let Some(c) = system_chunker.as_mut() {
            c.finish(&mut sink);
        }
    }
    for chunk in tail_for_stt {
        if let Some(result) =
            process_chunk(chunk, stt, &transcript_path, id, hooks, diarizer).await?
        {
            match result.target {
                StoreTarget::Mic => mic_text_chunks.push(result.text),
                StoreTarget::System => sys_text_chunks.push(result.text),
            }
        }
    }

    let tail_page = audio_encoder.finish().map_err(CoreError::Pipeline)?;
    if !tail_page.is_empty() {
        append_durable(&audio_path, &tail_page)?;
    }

    let attributed = diarizer
        .diarize(&mic_text_chunks, &sys_text_chunks, &context)
        .await?;

    let transcript_body =
        std::fs::read_to_string(&transcript_path).map_err(|e| CoreError::Storage(e.into()))?;
    let prompt = notes::render_notes_prompt(&transcript_body, &context);
    let llm_output = llm.complete(&prompt).await?;
    let notes_body = notes::render_notes_body(title.as_deref(), started_at, &llm_output);
    atomic_replace(&notes_path, notes_body.as_bytes())?;

    let ended_at = Utc::now();
    let meta = build_meta_toml(&MetaArgs {
        id,
        title: title.as_deref(),
        started_at,
        ended_at,
        attestation: &attestation,
        stt_name: stt.name(),
        llm_name: llm.name(),
        diarizer_name: diarizer.name(),
    })?;
    atomic_replace(&meta_path, meta.as_bytes())?;

    dispatch_hooks(
        hooks,
        &LifecycleEvent::SessionEnd {
            id,
            transcript_path: transcript_path.clone(),
        },
    )
    .await;
    dispatch_hooks(
        hooks,
        &LifecycleEvent::NotesGenerated {
            id,
            notes_path: notes_path.clone(),
        },
    )
    .await;

    Ok(SessionOutputs {
        folder,
        transcript_path,
        notes_path,
        meta_path,
        audio_path,
        attestation,
        chunks: attributed,
    })
}

#[derive(Clone, Copy)]
enum StoreTarget {
    Mic,
    System,
}

struct ChunkOutcome {
    text: crate::types::TranscriptChunk,
    target: StoreTarget,
}

async fn process_chunk<S: SttProvider, D: Diarizer>(
    chunk: EmittedChunk,
    stt: &S,
    transcript_path: &std::path::Path,
    session_id: SessionId,
    hooks: &[Box<dyn Hook>],
    _diarizer: &D,
) -> Result<Option<ChunkOutcome>, CoreError> {
    let target = match chunk.source {
        FrameSource::System => StoreTarget::System,
        FrameSource::Mic | FrameSource::Mixed => StoreTarget::Mic,
    };
    let audio_chunk = match build_audio_chunk(&chunk) {
        Ok(audio) => audio,
        Err(CoreError::Pipeline(PipelineError::EmptyChunk)) => {
            warn!(target = ?target_kind(target), "empty chunk dropped before stt");
            return Ok(None);
        }
        Err(other) => return Err(other),
    };
    let transcript = stt.transcribe(audio_chunk).await?;

    let speaker = match chunk.source {
        FrameSource::System => SpeakerLabel::Them,
        FrameSource::Mic | FrameSource::Mixed => SpeakerLabel::Me,
    };
    let attributed = AttributedChunk {
        chunk: transcript.clone(),
        speaker: speaker.clone(),
    };
    let line = notes::render_transcript_line(&attributed);
    append_durable(transcript_path, line.as_bytes())?;

    if matches!(chunk.ended_on, ChunkBoundary::EndOfStream) {
        debug!(target = ?target_kind(target), "final chunk emitted");
    }
    dispatch_hooks(
        hooks,
        &LifecycleEvent::ChunkTranscribed {
            id: session_id,
            chunk: attributed,
        },
    )
    .await;

    Ok(Some(ChunkOutcome {
        text: transcript,
        target,
    }))
}

const fn target_kind(target: StoreTarget) -> &'static str {
    match target {
        StoreTarget::Mic => "mic",
        StoreTarget::System => "system",
    }
}

fn build_audio_chunk(chunk: &EmittedChunk) -> Result<AudioChunk, CoreError> {
    if chunk.frames.is_empty() {
        return Err(CoreError::Pipeline(PipelineError::EmptyChunk));
    }
    let source_rate = chunk.frames[0].sample_rate;
    let channels = chunk.frames[0].channels.max(1);
    let mut interleaved: Vec<f32> =
        Vec::with_capacity(chunk.frames.iter().map(|f| f.samples.len()).sum());
    for frame in &chunk.frames {
        interleaved.extend_from_slice(&frame.samples);
    }
    let mono: Vec<f32> = if channels == 1 {
        interleaved
    } else {
        downmix_to_mono(&interleaved, channels)
    };
    let resampled = resample_linear(&mono, source_rate, STT_SAMPLE_RATE)
        .map_err(|e| CoreError::Pipeline(e.into()))?;
    let samples: Arc<[f32]> = Arc::from(resampled);
    Ok(AudioChunk {
        samples,
        source: chunk.source,
        start: chunk.start,
        duration: chunk.duration,
    })
}

#[allow(clippy::cast_precision_loss)]
fn downmix_to_mono(interleaved: &[f32], channels: u16) -> Vec<f32> {
    let chans = usize::from(channels);
    if chans == 0 {
        return Vec::new();
    }
    let frames = interleaved.len() / chans;
    let mut out = Vec::with_capacity(frames);
    for f in 0..frames {
        let base = f * chans;
        let mut sum = 0.0_f32;
        for c in 0..chans {
            sum += interleaved[base + c];
        }
        out.push(sum / chans as f32);
    }
    out
}

#[derive(Serialize, Deserialize)]
struct MetaTomlV1 {
    session_id: String,
    title: Option<String>,
    started_at: DateTime<Utc>,
    ended_at: DateTime<Utc>,
    duration_secs: u64,
    consent: ConsentAttestation,
    providers: Providers,
    scrybe: Versioning,
}

#[derive(Serialize, Deserialize)]
struct Providers {
    stt: String,
    llm: String,
    diarizer: String,
}

#[derive(Serialize, Deserialize)]
struct Versioning {
    version: String,
}

struct MetaArgs<'a> {
    id: SessionId,
    title: Option<&'a str>,
    started_at: DateTime<Utc>,
    ended_at: DateTime<Utc>,
    attestation: &'a ConsentAttestation,
    stt_name: &'a str,
    llm_name: &'a str,
    diarizer_name: &'a str,
}

fn build_meta_toml(args: &MetaArgs<'_>) -> Result<String, CoreError> {
    let MetaArgs {
        id,
        title,
        started_at,
        ended_at,
        attestation,
        stt_name,
        llm_name,
        diarizer_name,
    } = *args;
    let duration = (ended_at - started_at)
        .to_std()
        .unwrap_or(Duration::ZERO)
        .as_secs();
    let meta = MetaTomlV1 {
        session_id: id.to_string(),
        title: title.map(str::to_string),
        started_at,
        ended_at,
        duration_secs: duration,
        consent: attestation.clone(),
        providers: Providers {
            stt: stt_name.to_string(),
            llm: llm_name.to_string(),
            diarizer: diarizer_name.to_string(),
        },
        scrybe: Versioning {
            version: env!("CARGO_PKG_VERSION").to_string(),
        },
    };
    toml::to_string(&meta)
        .map_err(|e| CoreError::Pipeline(PipelineError::MetaSerialize(Box::new(e))))
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
    use crate::consent::AcceptingPrompter;
    use crate::diarize::Diarizer;
    use crate::error::{ConsentError, LlmError, SttError};
    use crate::hooks::Hook;
    use crate::pipeline::vad::EnergyVad;
    use crate::types::{AudioFrame, FrameSource, TranscriptChunk};
    use async_trait::async_trait;
    use chrono::TimeZone;
    use futures::stream;
    use pretty_assertions::assert_eq;

    struct EchoStt;
    #[async_trait]
    impl SttProvider for EchoStt {
        async fn transcribe(&self, chunk: AudioChunk) -> Result<TranscriptChunk, SttError> {
            Ok(TranscriptChunk {
                text: format!("samples={}", chunk.samples.len()),
                source: chunk.source,
                start_ms: u64::try_from(chunk.start.as_millis()).unwrap_or(0),
                duration_ms: u64::try_from(chunk.duration.as_millis()).unwrap_or(0),
                language: None,
            })
        }
        fn name(&self) -> &'static str {
            "echo-stt"
        }
    }

    struct CannedLlm;
    #[async_trait]
    impl LlmProvider for CannedLlm {
        async fn complete(&self, _prompt: &str) -> Result<String, LlmError> {
            Ok("## TL;DR\n- talked\n## Action items\n- ship\n".to_string())
        }
        fn name(&self) -> &'static str {
            "canned-llm"
        }
    }

    struct PassThroughDiarizer;
    #[async_trait]
    impl Diarizer for PassThroughDiarizer {
        async fn diarize(
            &self,
            mic: &[TranscriptChunk],
            sys: &[TranscriptChunk],
            _ctx: &MeetingContext,
        ) -> Result<Vec<AttributedChunk>, CoreError> {
            let mut out = Vec::new();
            for c in mic {
                out.push(AttributedChunk {
                    chunk: c.clone(),
                    speaker: SpeakerLabel::Me,
                });
            }
            for c in sys {
                out.push(AttributedChunk {
                    chunk: c.clone(),
                    speaker: SpeakerLabel::Them,
                });
            }
            Ok(out)
        }
        fn name(&self) -> &'static str {
            "binary-channel"
        }
    }

    fn speech_frame(timestamp_ns: u64, frame_size: usize) -> AudioFrame {
        let samples: Vec<f32> = (0..frame_size).map(|n| (n as f32 * 0.5).sin()).collect();
        AudioFrame {
            samples: Arc::from(samples),
            channels: 1,
            sample_rate: 16_000,
            timestamp_ns,
            source: FrameSource::Mic,
        }
    }

    const fn small_chunker_config() -> ChunkerConfig {
        ChunkerConfig {
            max_chunk: Duration::from_millis(300),
            min_speech_before_silence_split: Duration::from_millis(50),
            silence_split_after: Duration::from_millis(50),
        }
    }

    fn dt() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 4, 29, 14, 30, 0).unwrap()
    }

    #[tokio::test]
    async fn test_run_writes_transcript_notes_and_meta_files() {
        let tmp = tempfile::tempdir().unwrap();
        let stt = EchoStt;
        let llm = CannedLlm;
        let diarizer = PassThroughDiarizer;
        let prompter = AcceptingPrompter;
        let hooks: Vec<Box<dyn Hook>> = Vec::new();

        let frames = stream::iter((0..6).map(|i| Ok(speech_frame(i * 10_000_000, 1_600))));

        let inputs = SessionInputs {
            id: SessionId::new(),
            started_at: dt(),
            root: tmp.path().to_path_buf(),
            title: Some("standup".into()),
            user: "tom".into(),
            consent_mode: ConsentMode::Quick,
            context: MeetingContext::default(),
            mic_vad: EnergyVad::default(),
            system_vad: None,
            stt: &stt,
            llm: &llm,
            diarizer: &diarizer,
            prompter: &prompter,
            hooks: &hooks,
            chunker_config: small_chunker_config(),
        };

        let outputs = run(inputs, frames).await.unwrap();

        assert!(outputs.transcript_path.exists());
        assert!(outputs.notes_path.exists());
        assert!(outputs.meta_path.exists());
        let transcript = std::fs::read_to_string(&outputs.transcript_path).unwrap();
        assert!(transcript.contains("# standup"));
        assert!(transcript.contains("**Me**"));
        let notes = std::fs::read_to_string(&outputs.notes_path).unwrap();
        assert!(notes.contains("## TL;DR"));
        let meta = std::fs::read_to_string(&outputs.meta_path).unwrap();
        assert!(meta.contains("session_id"));
        assert!(meta.contains("stt = \"echo-stt\""));
        assert!(meta.contains("llm = \"canned-llm\""));
        assert!(meta.contains("diarizer = \"binary-channel\""));
    }

    #[tokio::test]
    async fn test_run_records_consent_attestation_in_meta_toml() {
        let tmp = tempfile::tempdir().unwrap();
        let stt = EchoStt;
        let llm = CannedLlm;
        let diarizer = PassThroughDiarizer;
        let prompter = AcceptingPrompter;
        let hooks: Vec<Box<dyn Hook>> = Vec::new();

        let frames = stream::iter((0..2).map(|i| Ok(speech_frame(i * 10_000_000, 800))));

        let inputs = SessionInputs {
            id: SessionId::new(),
            started_at: dt(),
            root: tmp.path().to_path_buf(),
            title: None,
            user: "tom".into(),
            consent_mode: ConsentMode::Quick,
            context: MeetingContext::default(),
            mic_vad: EnergyVad::default(),
            system_vad: None,
            stt: &stt,
            llm: &llm,
            diarizer: &diarizer,
            prompter: &prompter,
            hooks: &hooks,
            chunker_config: small_chunker_config(),
        };

        let outputs = run(inputs, frames).await.unwrap();

        assert_eq!(outputs.attestation.mode, ConsentMode::Quick);
        assert_eq!(outputs.attestation.by_user, "tom");
        let meta = std::fs::read_to_string(&outputs.meta_path).unwrap();
        assert!(meta.contains("[consent]"));
        assert!(meta.contains("by_user = \"tom\""));
    }

    #[tokio::test]
    async fn test_run_returns_consent_error_when_user_aborts() {
        let tmp = tempfile::tempdir().unwrap();
        let stt = EchoStt;
        let llm = CannedLlm;
        let diarizer = PassThroughDiarizer;
        let prompter = crate::consent::AbortingPrompter;
        let hooks: Vec<Box<dyn Hook>> = Vec::new();

        let frames = stream::iter((0..2).map(|i| Ok(speech_frame(i * 10_000_000, 800))));

        let inputs = SessionInputs {
            id: SessionId::new(),
            started_at: dt(),
            root: tmp.path().to_path_buf(),
            title: Some("aborted".into()),
            user: "tom".into(),
            consent_mode: ConsentMode::Quick,
            context: MeetingContext::default(),
            mic_vad: EnergyVad::default(),
            system_vad: None,
            stt: &stt,
            llm: &llm,
            diarizer: &diarizer,
            prompter: &prompter,
            hooks: &hooks,
            chunker_config: small_chunker_config(),
        };

        let err = run(inputs, frames).await.unwrap_err();

        assert!(matches!(err, CoreError::Consent(ConsentError::UserAborted)));
    }

    #[tokio::test]
    async fn test_run_acquires_session_lock_for_duration_of_run() {
        let tmp = tempfile::tempdir().unwrap();
        let stt = EchoStt;
        let llm = CannedLlm;
        let diarizer = PassThroughDiarizer;
        let prompter = AcceptingPrompter;
        let hooks: Vec<Box<dyn Hook>> = Vec::new();

        let frames = stream::iter((0..2).map(|i| Ok(speech_frame(i * 10_000_000, 800))));

        let inputs = SessionInputs {
            id: SessionId::new(),
            started_at: dt(),
            root: tmp.path().to_path_buf(),
            title: Some("locked".into()),
            user: "tom".into(),
            consent_mode: ConsentMode::Quick,
            context: MeetingContext::default(),
            mic_vad: EnergyVad::default(),
            system_vad: None,
            stt: &stt,
            llm: &llm,
            diarizer: &diarizer,
            prompter: &prompter,
            hooks: &hooks,
            chunker_config: small_chunker_config(),
        };

        let outputs = run(inputs, frames).await.unwrap();

        let lock = outputs.folder.join(crate::storage::PID_LOCK_NAME);
        assert!(
            !lock.exists(),
            "pid.lock must be released on clean shutdown"
        );
    }
}
