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
use std::thread::JoinHandle;
use std::time::Duration;

use chrono::{DateTime, Utc};
use futures::stream::{Stream, StreamExt};
use serde::{Deserialize, Serialize};
use tracing::{debug, warn};

use crate::consent::ConsentPrompter;
use crate::context::MeetingContext;
use crate::diarize::Diarizer;
use crate::error::{CoreError, PipelineError, StorageError};
use crate::hooks::{dispatch_hooks, Hook, LifecycleEvent};
use crate::notes;
use crate::pipeline::chunker::{ChunkBoundary, Chunker, ChunkerConfig, EmittedChunk};
use crate::pipeline::encoder::EncoderConfig;
use crate::pipeline::journal::{JournalAnchor, JournalManifest, JournalWriter};
use crate::pipeline::merge::merge_journal;
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
    /// Whether the offline merge asserts the encoded audio duration
    /// is within 1% of real wall-clock elapsed time
    /// (`Utc::now() - started_at`). `true` for every capture source
    /// backed by real hardware, where the assertion is a genuine
    /// safety net against journal-level data loss or a resample bug
    /// (`docs/development-plan.md` §19.2 defect D1). `false` for the
    /// deterministic synthetic source, whose frames declare their
    /// own elapsed time via `AudioFrame::timestamp_ns` but are
    /// generated in-process without real-time pacing — comparing
    /// their encoded duration against actual CPU wall-clock time
    /// would fail by construction, not because anything is wrong.
    pub verify_duration: bool,
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
        verify_duration,
    } = inputs;

    let attestation = crate::consent::run(consent_mode, user, prompter).await?;

    let initial_title = title.as_deref().unwrap_or("untitled");
    let folder_name = session_folder_name(started_at, initial_title, id);
    let folder = root.join(folder_name);
    std::fs::create_dir_all(&folder).map_err(|e| CoreError::Storage(e.into()))?;

    let mut lock_path = acquire_session_lock(&folder, std::process::id())?;
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
            verify_duration,
        },
        capture_stream,
    )
    .await;

    if let Ok(outputs) = &outcome {
        lock_path = outputs.folder.join(crate::storage::PID_LOCK_NAME);
    }

    if let Err(error) = &outcome {
        let failure = LifecycleEvent::SessionFailed {
            id,
            error: Arc::new(std::io::Error::other(error.to_string())),
        };
        let _ = dispatch_hooks(hooks, &failure).await;
    }

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
    verify_duration: bool,
}

/// Lazily-spawned per-source journal writers for one session. Each
/// source spawns its writer on its own first frame, using that
/// frame's native `sample_rate`/`channels` — the same stability
/// assumption every other per-source pipeline stage makes. Additive
/// to the live encode path below: every session now also durably
/// journals raw per-source PCM, independent of whatever the encoder
/// does with it. The anchor manifest and the offline merge that
/// replace the live encode path are layered on top in later stacks.
struct SessionJournals {
    dir: PathBuf,
    mic: Option<JournalSlot>,
    system: Option<JournalSlot>,
    partial_manifest_writer: Option<JoinHandle<Result<(), CoreError>>>,
}

/// A spawned writer plus the anchor fields known the moment it was
/// spawned (`sample_rate`/`channels`/`first_frame_epoch_ms` — all
/// final at spawn time; only `frames_written` grows afterward).
struct JournalSlot {
    first_frame_epoch_ms: i64,
    sample_rate: u32,
    channels: u16,
    writer: JournalWriter,
}

impl JournalSlot {
    /// Anchor reflecting this slot's state right now. `frames_written`
    /// is `0` while capture is still in progress — never load-bearing
    /// for the merge (which re-reads segment bytes directly), only
    /// informational for a partial manifest written mid-session.
    const fn partial_anchor(&self) -> JournalAnchor {
        JournalAnchor {
            first_frame_epoch_ms: self.first_frame_epoch_ms,
            sample_rate: self.sample_rate,
            channels: self.channels,
            frames_written: 0,
        }
    }
}

impl SessionJournals {
    const fn new(dir: PathBuf) -> Self {
        Self {
            dir,
            mic: None,
            system: None,
            partial_manifest_writer: None,
        }
    }

    fn push(&mut self, frame: &AudioFrame) -> Result<(), CoreError> {
        let slot = match frame.source {
            FrameSource::System => &mut self.system,
            FrameSource::Mic | FrameSource::Mixed => &mut self.mic,
        };
        let spawned_new = slot.is_none();
        if spawned_new {
            let writer =
                JournalWriter::spawn(&self.dir, frame.source, frame.sample_rate, frame.channels)?;
            *slot = Some(JournalSlot {
                first_frame_epoch_ms: Utc::now().timestamp_millis(),
                sample_rate: frame.sample_rate,
                channels: frame.channels,
                writer,
            });
        }
        if let Some(s) = slot.as_ref() {
            s.writer.push(Arc::clone(&frame.samples));
        }
        if spawned_new {
            // Refresh the on-disk manifest immediately, not just at
            // session end: a process that never reaches its normal
            // `finish()` (crash, SIGKILL) still leaves a usable
            // anchor on disk for `scrybe repair` to read.
            self.write_partial_manifest()?;
        }
        Ok(())
    }

    /// Writes in the background during capture. Source transitions and
    /// [`Self::finish`] join the prior write before starting the next one, so
    /// `manifest.toml` never has competing replacements on Windows.
    fn write_partial_manifest(&mut self) -> Result<(), CoreError> {
        self.finish_partial_manifest_write()?;
        let manifest = JournalManifest {
            mic: self.mic.as_ref().map(JournalSlot::partial_anchor),
            system: self.system.as_ref().map(JournalSlot::partial_anchor),
        };
        let dir = self.dir.clone();
        self.partial_manifest_writer = Some(std::thread::spawn(move || {
            crate::pipeline::journal::write_manifest(&dir, &manifest)
        }));
        Ok(())
    }

    fn finish_partial_manifest_write(&mut self) -> Result<(), CoreError> {
        let Some(writer) = self.partial_manifest_writer.take() else {
            return Ok(());
        };
        writer.join().unwrap_or_else(|_| {
            Err(CoreError::Storage(StorageError::Io(std::io::Error::other(
                "partial journal manifest writer thread panicked",
            ))))
        })
    }

    /// Closes every spawned writer and returns the session's journal
    /// manifest: one `JournalAnchor` per source that received at
    /// least one frame, `None` for a source never used this session.
    fn finish(mut self) -> Result<JournalManifest, CoreError> {
        self.finish_partial_manifest_write()?;
        let mic = finish_anchor(self.mic)?;
        let system = finish_anchor(self.system)?;
        Ok(JournalManifest { mic, system })
    }
}

fn finish_anchor(slot: Option<JournalSlot>) -> Result<Option<JournalAnchor>, CoreError> {
    let Some(slot) = slot else {
        return Ok(None);
    };
    let summary = slot.writer.finish()?;
    Ok(Some(JournalAnchor {
        first_frame_epoch_ms: slot.first_frame_epoch_ms,
        sample_rate: summary.sample_rate,
        channels: summary.channels,
        frames_written: summary.frames_written,
    }))
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
        mut folder,
        title,
        mut context,
        attestation,
        mic_vad,
        system_vad,
        stt,
        llm,
        diarizer,
        hooks,
        chunker_config,
        verify_duration,
    } = inputs;

    let context_arc = Arc::new(context.clone());

    let mut transcript_path = folder.join("transcript.md");
    let mut notes_path = folder.join("notes.md");
    let mut meta_path = folder.join("meta.toml");
    let mut audio_path = folder.join("audio.opus");

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
    // `audio.opus` is produced entirely by the offline merge
    // (`pipeline::merge::merge_journal`) after capture ends, from the
    // per-source journal this loop writes below — never by a live
    // push/drain/encode path. This closes defect D1 (a crash loses at
    // most the current journal segment, not the whole recording) and
    // defect D2 (the merge aligns sources by their wall-clock
    // `first_frame_epoch_ms` anchors, not by comparing
    // `AudioFrame::timestamp_ns` across sources).
    let encoder_config = EncoderConfig::default();
    let mut journals = SessionJournals::new(folder.join("journal"));
    let mut terminal_capture_error = None;

    let mut mic_text_chunks: Vec<crate::types::TranscriptChunk> = Vec::new();
    let mut sys_text_chunks: Vec<crate::types::TranscriptChunk> = Vec::new();

    while let Some(frame_result) = capture_stream.next().await {
        let frame = match frame_result {
            Ok(frame) => frame,
            Err(error) => {
                terminal_capture_error = Some(error);
                break;
            }
        };
        journals.push(&frame)?;
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

    let journal_manifest = journals.finish()?;
    if journal_manifest.mic.is_some() || journal_manifest.system.is_some() {
        // Written before the merge attempt (not after) so a duration-
        // assertion failure below still leaves a manifest on disk for
        // `scrybe repair` to read the anchors from without re-deriving
        // them.
        crate::pipeline::journal::write_manifest(&folder.join("journal"), &journal_manifest)?;
    }
    #[allow(clippy::cast_precision_loss)]
    let wall_clock_secs = if verify_duration {
        (Utc::now() - started_at).num_milliseconds() as f64 / 1000.0
    } else {
        0.0
    };
    let merge_report = merge_journal(
        &folder.join("journal"),
        &audio_path,
        &journal_manifest,
        encoder_config,
        wall_clock_secs,
    )?;
    debug!(
        ?journal_manifest,
        encoded_secs = merge_report.encoded_secs,
        channels = merge_report.channels,
        wall_clock_secs,
        "offline journal merge complete"
    );

    let attributed = diarizer
        .diarize(&mic_text_chunks, &sys_text_chunks, &context)
        .await?;

    let mut transcript_body =
        std::fs::read_to_string(&transcript_path).map_err(|e| CoreError::Storage(e.into()))?;
    let final_title = if let Some(existing) = title {
        existing
    } else {
        let title_prompt = notes::render_title_prompt(&transcript_body);
        let raw_title = llm.complete(&title_prompt).await?;
        notes::clean_generated_title(&raw_title)
            .ok_or(CoreError::Pipeline(PipelineError::InvalidGeneratedTitle))?
    };
    context.title = Some(final_title.clone());
    if !transcript_body.starts_with(&format!("# {final_title}\n")) {
        let body = transcript_body
            .split_once("\n\n")
            .map_or("", |(_, rest)| rest);
        let updated = notes::render_transcript_header(Some(&final_title), started_at, None) + body;
        atomic_replace(&transcript_path, updated.as_bytes())?;
        transcript_body = updated;
    }

    let prompt = notes::render_notes_prompt(&transcript_body, &context);
    let llm_output = llm.complete(&prompt).await?;
    let notes_body = notes::render_notes_body(Some(&final_title), started_at, &llm_output);
    atomic_replace(&notes_path, notes_body.as_bytes())?;

    let ended_at = Utc::now();
    let audio_meta = Some(AudioMeta {
        channels: merge_report.channels,
        layout: audio_layout(merge_report.channels == 2, merge_report.channels).to_string(),
        sample_rate: encoder_config.sample_rate,
        bitrate_bps: encoder_config.bitrate_bps,
        mic_epoch_ms: journal_manifest.mic.map(|a| a.first_frame_epoch_ms),
        system_epoch_ms: journal_manifest.system.map(|a| a.first_frame_epoch_ms),
    });
    let meta = build_meta_toml(MetaArgs {
        id,
        title: Some(&final_title),
        started_at,
        ended_at,
        attestation: &attestation,
        stt_name: stt.name(),
        llm_name: llm.name(),
        diarizer_name: diarizer.name(),
        audio: audio_meta,
    })?;
    atomic_replace(&meta_path, meta.as_bytes())?;

    let final_folder = folder.parent().map_or_else(
        || folder.clone(),
        |parent| parent.join(session_folder_name(started_at, &final_title, id)),
    );
    if final_folder != folder {
        std::fs::rename(&folder, &final_folder).map_err(|e| CoreError::Storage(e.into()))?;
        folder = final_folder;
        transcript_path = folder.join("transcript.md");
        notes_path = folder.join("notes.md");
        meta_path = folder.join("meta.toml");
        audio_path = folder.join("audio.opus");
    }

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

    if let Some(error) = terminal_capture_error {
        return Err(CoreError::Capture(error));
    }

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
pub(crate) struct MetaTomlV1 {
    session_id: String,
    title: Option<String>,
    started_at: DateTime<Utc>,
    ended_at: DateTime<Utc>,
    duration_secs: u64,
    consent: ConsentAttestation,
    providers: Providers,
    scrybe: Versioning,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    audio: Option<AudioMeta>,
}

#[derive(Serialize, Deserialize)]
pub(crate) struct Providers {
    stt: String,
    llm: String,
    diarizer: String,
}

#[derive(Serialize, Deserialize)]
pub(crate) struct Versioning {
    version: String,
}

/// Storage-layer description of how `audio.opus` was encoded.
///
/// Added in v1.1: `layout` is the canonical channel-attribution
/// descriptor so downstream tools (`scrybe rerun`, third-party
/// scripts, future GUI) can split mic and system audio without
/// re-running the diarizer. Older sessions without this block are
/// implicitly `mono:mic` or `mono:synthetic` — readers MUST treat the
/// absence as v1.0 mono and not assume any channel attribution.
#[derive(Serialize, Deserialize)]
pub(crate) struct AudioMeta {
    pub(crate) channels: u16,
    pub(crate) layout: String,
    pub(crate) sample_rate: u32,
    pub(crate) bitrate_bps: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) mic_epoch_ms: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) system_epoch_ms: Option<i64>,
}

pub(crate) struct MetaArgs<'a> {
    pub(crate) id: SessionId,
    pub(crate) title: Option<&'a str>,
    pub(crate) started_at: DateTime<Utc>,
    pub(crate) ended_at: DateTime<Utc>,
    pub(crate) attestation: &'a ConsentAttestation,
    pub(crate) stt_name: &'a str,
    pub(crate) llm_name: &'a str,
    pub(crate) diarizer_name: &'a str,
    pub(crate) audio: Option<AudioMeta>,
}

/// Canonical channel-attribution descriptor written into
/// `meta.audio.layout`. Stable values consumed by `scrybe show`,
/// future `scrybe rerun`, and any third-party tool that needs to
/// know what each channel of `audio.opus` carries.
pub(crate) const fn audio_layout(stereo: bool, channels: u16) -> &'static str {
    match (stereo, channels) {
        (true, 2) => "stereo:mic-l,system-r",
        _ => "mono:mic",
    }
}

pub(crate) fn build_meta_toml(args: MetaArgs<'_>) -> Result<String, CoreError> {
    let MetaArgs {
        id,
        title,
        started_at,
        ended_at,
        attestation,
        stt_name,
        llm_name,
        diarizer_name,
        audio,
    } = args;
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
        audio,
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
    use crate::error::{CaptureError, ConsentError, LlmError, SttError};
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
        async fn complete(&self, prompt: &str) -> Result<String, LlmError> {
            if prompt.starts_with("Create a short, factual title") {
                return Ok("Generated Standup".to_string());
            }
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
            verify_duration: false,
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
    async fn test_run_finalizes_artifacts_before_returning_terminal_capture_error() {
        let tmp = tempfile::tempdir().unwrap();
        let stt = EchoStt;
        let llm = CannedLlm;
        let diarizer = PassThroughDiarizer;
        let prompter = AcceptingPrompter;
        let hooks: Vec<Box<dyn Hook>> = Vec::new();
        let frames = stream::iter([
            Ok(speech_frame(0, 1_600)),
            Err(CaptureError::Platform(Box::new(std::io::Error::other(
                "capture liveness watchdog expired",
            )))),
        ]);
        let inputs = SessionInputs {
            id: SessionId::new(),
            started_at: dt(),
            root: tmp.path().to_path_buf(),
            title: Some("watchdog".into()),
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
            verify_duration: false,
        };

        let error = run(inputs, frames)
            .await
            .expect_err("terminal capture error");
        assert!(matches!(
            error,
            CoreError::Capture(CaptureError::Platform(_))
        ));

        let folder = std::fs::read_dir(tmp.path())
            .unwrap()
            .find_map(Result::ok)
            .map(|entry| entry.path())
            .expect("finalized session folder");
        assert!(folder.join("audio.opus").exists());
        assert!(folder.join("transcript.md").exists());
        assert!(folder.join("notes.md").exists());
        assert!(folder.join("meta.toml").exists());
        assert!(!folder.join("journal").exists());
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
            verify_duration: false,
        };

        let outputs = run(inputs, frames).await.unwrap();

        assert_eq!(outputs.attestation.mode, ConsentMode::Quick);
        assert_eq!(outputs.attestation.by_user, "tom");
        let meta = std::fs::read_to_string(&outputs.meta_path).unwrap();
        assert!(meta.contains("[consent]"));
        assert!(meta.contains("by_user = \"tom\""));
        assert!(meta.contains("title = \"Generated Standup\""));
        assert!(outputs
            .folder
            .file_name()
            .unwrap()
            .to_string_lossy()
            .contains("generated-standup"));
        let transcript = std::fs::read_to_string(&outputs.transcript_path).unwrap();
        assert!(transcript.starts_with("# Generated Standup\n"));
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
            verify_duration: false,
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
            verify_duration: false,
        };

        let outputs = run(inputs, frames).await.unwrap();

        let lock = outputs.folder.join(crate::storage::PID_LOCK_NAME);
        assert!(
            !lock.exists(),
            "pid.lock must be released on clean shutdown"
        );
    }

    fn stereo_speech_frame(
        timestamp_ns: u64,
        frame_size: usize,
        source: FrameSource,
    ) -> AudioFrame {
        let samples: Vec<f32> = (0..frame_size).map(|n| (n as f32 * 0.5).sin()).collect();
        AudioFrame {
            samples: Arc::from(samples),
            channels: 1,
            sample_rate: 48_000,
            timestamp_ns,
            source,
        }
    }

    #[tokio::test]
    async fn test_run_writes_audio_meta_block_with_mono_layout_when_no_system_vad() {
        let tmp = tempfile::tempdir().unwrap();
        let stt = EchoStt;
        let llm = CannedLlm;
        let diarizer = PassThroughDiarizer;
        let prompter = AcceptingPrompter;
        let hooks: Vec<Box<dyn Hook>> = Vec::new();

        let frames = stream::iter((0..4).map(|i| Ok(speech_frame(i * 10_000_000, 1_600))));

        let inputs = SessionInputs {
            id: SessionId::new(),
            started_at: dt(),
            root: tmp.path().to_path_buf(),
            title: Some("mono-meta".into()),
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
            verify_duration: false,
        };

        let outputs = run(inputs, frames).await.unwrap();
        let meta = std::fs::read_to_string(&outputs.meta_path).unwrap();

        assert!(
            meta.contains("[audio]"),
            "meta missing [audio] block: {meta}"
        );
        assert!(meta.contains("channels = 1"));
        assert!(meta.contains("layout = \"mono:mic\""));
        assert!(meta.contains("sample_rate = 48000"));
    }

    #[tokio::test]
    async fn test_run_writes_audio_meta_block_with_stereo_layout_when_system_vad_set() {
        let tmp = tempfile::tempdir().unwrap();
        let stt = EchoStt;
        let llm = CannedLlm;
        let diarizer = PassThroughDiarizer;
        let prompter = AcceptingPrompter;
        let hooks: Vec<Box<dyn Hook>> = Vec::new();

        // Interleave mic and system frames at 48 kHz so the interleaver
        // can pair them. Each frame is 480 samples = 10 ms; pushing 8
        // pairs gives 80 ms of audio — enough to clear the interleaver's
        // MIN_DRAIN_SAMPLES threshold (480 samples).
        let mut interleaved: Vec<Result<AudioFrame, crate::error::CaptureError>> = Vec::new();
        for i in 0..16 {
            let ts = i * 10_000_000;
            let source = if i % 2 == 0 {
                FrameSource::Mic
            } else {
                FrameSource::System
            };
            interleaved.push(Ok(stereo_speech_frame(ts, 480, source)));
        }
        let frames = stream::iter(interleaved);

        let inputs = SessionInputs {
            id: SessionId::new(),
            started_at: dt(),
            root: tmp.path().to_path_buf(),
            title: Some("stereo-meta".into()),
            user: "tom".into(),
            consent_mode: ConsentMode::Quick,
            context: MeetingContext::default(),
            mic_vad: EnergyVad::default(),
            system_vad: Some(EnergyVad::default()),
            stt: &stt,
            llm: &llm,
            diarizer: &diarizer,
            prompter: &prompter,
            hooks: &hooks,
            chunker_config: small_chunker_config(),
            verify_duration: false,
        };

        let outputs = run(inputs, frames).await.unwrap();
        let meta = std::fs::read_to_string(&outputs.meta_path).unwrap();

        assert!(
            meta.contains("[audio]"),
            "meta missing [audio] block: {meta}"
        );
        assert!(meta.contains("channels = 2"));
        assert!(meta.contains("layout = \"stereo:mic-l,system-r\""));
        assert!(meta.contains("sample_rate = 48000"));
    }

    #[tokio::test]
    async fn test_run_writes_meta_epoch_fields_for_stereo_session_and_deletes_journal() {
        let tmp = tempfile::tempdir().unwrap();
        let stt = EchoStt;
        let llm = CannedLlm;
        let diarizer = PassThroughDiarizer;
        let prompter = AcceptingPrompter;
        let hooks: Vec<Box<dyn Hook>> = Vec::new();

        let mut interleaved: Vec<Result<AudioFrame, crate::error::CaptureError>> = Vec::new();
        for i in 0..16 {
            let ts = i * 10_000_000;
            let source = if i % 2 == 0 {
                FrameSource::Mic
            } else {
                FrameSource::System
            };
            interleaved.push(Ok(stereo_speech_frame(ts, 480, source)));
        }
        let frames = stream::iter(interleaved);

        let inputs = SessionInputs {
            id: SessionId::new(),
            started_at: dt(),
            root: tmp.path().to_path_buf(),
            title: Some("journal-anchor".into()),
            user: "tom".into(),
            consent_mode: ConsentMode::Quick,
            context: MeetingContext::default(),
            mic_vad: EnergyVad::default(),
            system_vad: Some(EnergyVad::default()),
            stt: &stt,
            llm: &llm,
            diarizer: &diarizer,
            prompter: &prompter,
            hooks: &hooks,
            chunker_config: small_chunker_config(),
            verify_duration: false,
        };

        let outputs = run(inputs, frames).await.unwrap();

        let journal_dir = outputs.folder.join("journal");
        assert!(
            !journal_dir.exists(),
            "journal must be deleted after a successful offline merge"
        );
        assert!(outputs.audio_path.exists());

        let meta = std::fs::read_to_string(&outputs.meta_path).unwrap();
        assert!(
            meta.contains("channels = 2"),
            "expected stereo merge: {meta}"
        );
        assert!(
            meta.contains("mic_epoch_ms"),
            "meta.toml [audio] missing mic_epoch_ms: {meta}"
        );
        assert!(
            meta.contains("system_epoch_ms"),
            "meta.toml [audio] missing system_epoch_ms: {meta}"
        );

        // Decode the merged audio (NullEncoder writes raw interleaved
        // f32 LE) and confirm the frame count is at least what was
        // pushed: 8 mic + 8 system frames of 480 samples each = 3840
        // stereo frames. Any epoch delta between the two sources'
        // `first_frame_epoch_ms` (real, even for frames generated
        // back-to-back, under scheduler pressure) adds a silence
        // prefix on top, so `>=` rather than `==`.
        let audio_bytes = std::fs::read(&outputs.audio_path).unwrap();
        let total_samples = audio_bytes.len() / 4;
        assert!(
            total_samples / 2 >= 3_840,
            "expected at least 3840 stereo frames, got {}",
            total_samples / 2
        );
    }

    #[tokio::test]
    async fn test_run_mono_session_meta_omits_system_epoch_ms_and_deletes_journal() {
        let tmp = tempfile::tempdir().unwrap();
        let stt = EchoStt;
        let llm = CannedLlm;
        let diarizer = PassThroughDiarizer;
        let prompter = AcceptingPrompter;
        let hooks: Vec<Box<dyn Hook>> = Vec::new();

        let frames = stream::iter((0..4).map(|i| Ok(speech_frame(i * 10_000_000, 1_600))));

        let inputs = SessionInputs {
            id: SessionId::new(),
            started_at: dt(),
            root: tmp.path().to_path_buf(),
            title: Some("mono-journal".into()),
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
            verify_duration: false,
        };

        let outputs = run(inputs, frames).await.unwrap();

        assert!(
            !outputs.folder.join("journal").exists(),
            "journal must be deleted after a successful offline merge"
        );
        let meta = std::fs::read_to_string(&outputs.meta_path).unwrap();
        assert!(meta.contains("channels = 1"), "expected mono merge: {meta}");
        assert!(meta.contains("mic_epoch_ms"));
        assert!(
            !meta.contains("system_epoch_ms"),
            "mono session must not emit system_epoch_ms: {meta}"
        );
    }

    #[tokio::test]
    async fn test_run_with_verify_duration_true_fails_loudly_on_declared_vs_wall_clock_mismatch() {
        // Regression guard for the `verify_duration` field itself:
        // when set (every real capture source), a session whose
        // frames declare far more audio than the real wall-clock time
        // `run()` actually took must fail loudly via the offline
        // merge's duration assertion, rather than silently accepting
        // a corrupt-duration `audio.opus`. `started_at: Utc::now()`
        // makes this a genuine real-time comparison: frames still
        // generate near-instantly (in-memory `stream::iter`), so the
        // encoded duration (7s of declared content) is wildly off
        // from the real elapsed wall-clock time of this test.
        let tmp = tempfile::tempdir().unwrap();
        let stt = EchoStt;
        let llm = CannedLlm;
        let diarizer = PassThroughDiarizer;
        let prompter = AcceptingPrompter;
        let hooks: Vec<Box<dyn Hook>> = Vec::new();

        // 7s of declared audio (frame_size=1600 at 16kHz = 100ms per
        // frame; 70 frames = 7s), generated instantly.
        let frames = stream::iter((0..70).map(|i| Ok(speech_frame(i * 100_000_000, 1_600))));

        let inputs = SessionInputs {
            id: SessionId::new(),
            started_at: Utc::now(),
            root: tmp.path().to_path_buf(),
            title: Some("duration-guard".into()),
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
            verify_duration: true,
        };

        let err = run(inputs, frames).await.unwrap_err();

        assert!(
            matches!(
                err,
                CoreError::Pipeline(PipelineError::DurationMismatch { .. })
            ),
            "expected DurationMismatch, got {err:?}"
        );
    }

    #[tokio::test]
    async fn test_run_stereo_audio_interleaves_mic_left_and_system_right() {
        // Regression for the v1.0.4 bug where mic + system frames pushed
        // serially into a mono encoder produced an `audio.opus` whose
        // duration was the SUM of both source durations rather than the
        // pairwise interleave. The byte count alone cannot distinguish
        // those two cases under `NullEncoder` (it writes raw f32 bytes
        // regardless of the encoder's channel config), so this test
        // inspects the actual content: mic samples must land on the
        // left channel and system samples on the right.
        let tmp = tempfile::tempdir().unwrap();
        let stt = EchoStt;
        let llm = CannedLlm;
        let diarizer = PassThroughDiarizer;
        let prompter = AcceptingPrompter;
        let hooks: Vec<Box<dyn Hook>> = Vec::new();

        // Distinct constant samples per source so the channel split is
        // detectable byte-for-byte. Mic = +0.5, System = -0.5.
        let mut interleaved: Vec<Result<AudioFrame, crate::error::CaptureError>> = Vec::new();
        for i in 0..16 {
            let ts = i * 10_000_000;
            let (source, value) = if i % 2 == 0 {
                (FrameSource::Mic, 0.5_f32)
            } else {
                (FrameSource::System, -0.5_f32)
            };
            let samples: Vec<f32> = vec![value; 480];
            interleaved.push(Ok(AudioFrame {
                samples: Arc::from(samples),
                channels: 1,
                sample_rate: 48_000,
                timestamp_ns: ts,
                source,
            }));
        }
        let frames = stream::iter(interleaved);

        let inputs = SessionInputs {
            id: SessionId::new(),
            started_at: dt(),
            root: tmp.path().to_path_buf(),
            title: Some("stereo-content".into()),
            user: "tom".into(),
            consent_mode: ConsentMode::Quick,
            context: MeetingContext::default(),
            mic_vad: EnergyVad::default(),
            system_vad: Some(EnergyVad::default()),
            stt: &stt,
            llm: &llm,
            diarizer: &diarizer,
            prompter: &prompter,
            hooks: &hooks,
            chunker_config: small_chunker_config(),
            verify_duration: false,
        };

        let outputs = run(inputs, frames).await.unwrap();
        let audio_bytes = std::fs::read(&outputs.audio_path).unwrap();

        // Decode raw f32 LE — NullEncoder writes interleaved samples
        // verbatim. Stereo means consecutive pairs are (L, R).
        assert_eq!(audio_bytes.len() % 8, 0, "byte count must be 8-aligned");
        let pcm: Vec<f32> = audio_bytes
            .chunks_exact(4)
            .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
            .collect();
        assert!(!pcm.is_empty(), "expected non-empty PCM payload");
        assert_eq!(pcm.len() % 2, 0, "stereo PCM must have even sample count");

        // Every L sample must be the mic value (+0.5) or silence
        // (0.0, from the epoch-delta prefix `interleave_stereo` adds
        // when one source's `first_frame_epoch_ms` lands even a
        // handful of milliseconds after the other's — real under
        // scheduler pressure even for frames generated back-to-back
        // in this test, not just on real hardware). Every R sample
        // must be the system value (-0.5) or silence, symmetrically.
        // The regression this guards against — a channel swap or a
        // collapse back to a single mono stream — would put a +0.5
        // on R or a -0.5 on L, which never happens under legitimate
        // silence-prefix padding.
        let mut mic_real_seen = false;
        let mut system_real_seen = false;
        for (i, pair) in pcm.chunks_exact(2).enumerate() {
            let l = pair[0];
            let r = pair[1];
            assert!(
                l.abs() < 1e-6 || (l - 0.5).abs() < 1e-6,
                "L channel at frame {i}: expected mic sample 0.5 or silence, got {l}"
            );
            assert!(
                r.abs() < 1e-6 || (r + 0.5).abs() < 1e-6,
                "R channel at frame {i}: expected system sample -0.5 or silence, got {r}"
            );
            mic_real_seen |= (l - 0.5).abs() < 1e-6;
            system_real_seen |= (r + 0.5).abs() < 1e-6;
        }
        assert!(mic_real_seen, "expected at least one real mic sample on L");
        assert!(
            system_real_seen,
            "expected at least one real system sample on R"
        );
    }

    #[test]
    fn test_audio_layout_returns_stereo_descriptor_for_two_channels() {
        assert_eq!(audio_layout(true, 2), "stereo:mic-l,system-r");
    }

    #[test]
    fn test_audio_layout_falls_back_to_mono_descriptor() {
        assert_eq!(audio_layout(false, 1), "mono:mic");
        // Defensive: stereo flag without 2 channels is nonsensical and
        // the helper prefers the safe mono descriptor.
        assert_eq!(audio_layout(true, 1), "mono:mic");
    }
}
