// Copyright 2026 Mathews Tom
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//     https://www.apache.org/licenses/LICENSE-2.0

//! Per-source audio journal writer.
//!
//! Captured audio must reach disk independently of the live encode
//! path so a crash mid-session loses at most the current segment, not
//! the whole recording (`docs/development-plan.md` §19.2 defect D1).
//! Each `FrameSource` gets an independent [`JournalWriter`] running on
//! its own OS thread, fed via an unbounded channel so a slow disk
//! never stalls the caller. `FrameSource::Mixed` shares the `mic`
//! journal with `FrameSource::Mic` — both already route to the same
//! chunker in `session::drive_session` — so a session produces at
//! most two journals, matching `meta.toml`'s `mic_epoch_ms` /
//! `system_epoch_ms` pair.
//!
//! Frames are appended as raw little-endian f32 PCM at the source's
//! native sample rate into rotating `journal/<source>-<seq>.f32`
//! segments. Rotation is driven by sample count (not wall clock), so
//! a throttled or silent source still rotates on the audio it
//! actually received rather than drifting from its peers. Each
//! segment is durably committed (`storage::atomic::full_fsync`) when
//! it closes, so a completed segment always survives a crash; only
//! the still-open segment can lose data.
//!
//! The anchor metadata (`first_frame_epoch_ms`, `journal/manifest.toml`)
//! and the offline merge that turns these segments into `audio.opus`
//! are separate stages layered on top of this module.

use std::fs::{File, OpenOptions};
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Arc;
use std::thread::JoinHandle;

use crate::error::{CoreError, StorageError};
use crate::storage::full_fsync;
use crate::types::FrameSource;

/// Target segment duration before rotating to the next `.f32` file.
/// Rotation checks the running sample count after each frame, so
/// actual segments are `>=` this duration, never split mid-frame.
const ROTATE_INTERVAL_SECS: u32 = 30;

/// Message sent from the pushing side to the writer thread.
enum JournalMsg {
    Frame(Arc<[f32]>),
    Finish,
}

/// What a writer thread reports once its journal is closed.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct JournalSummary {
    pub source: FrameSource,
    pub sample_rate: u32,
    pub channels: u16,
    /// Frames per channel actually written across every segment.
    pub frames_written: u64,
    /// Number of non-empty segment files created.
    pub segment_count: u32,
}

/// Anchor metadata for one source's journal.
///
/// `first_frame_epoch_ms` is the wall-clock time (milliseconds since
/// the Unix epoch) captured when the session orchestrator first
/// observed a frame from this source — the anchor the offline merge
/// uses to align two independently-clocked sources, since
/// `AudioFrame::timestamp_ns` is only comparable within one source's
/// own stream (`docs/development-plan.md` §19.2 defect D2).
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct JournalAnchor {
    pub first_frame_epoch_ms: i64,
    pub sample_rate: u32,
    pub channels: u16,
    pub frames_written: u64,
}

/// `journal/manifest.toml` contents: one optional anchor per source.
/// Written once at session end, after both writers have closed.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct JournalManifest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mic: Option<JournalAnchor>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub system: Option<JournalAnchor>,
}

/// Durably writes `manifest.toml` under `journal_dir`.
///
/// Creates the directory first if it does not exist yet. Callers may
/// write a manifest immediately after spawning a `JournalWriter` —
/// whose own `create_dir_all` runs asynchronously on its writer
/// thread — so this must not assume the directory is already there.
///
/// # Errors
///
/// Returns `CoreError::Pipeline` if serialization fails, or
/// `CoreError::Storage` if creating the directory or the atomic
/// write fails.
pub fn write_manifest(journal_dir: &Path, manifest: &JournalManifest) -> Result<(), CoreError> {
    std::fs::create_dir_all(journal_dir).map_err(|e| CoreError::Storage(StorageError::from(e)))?;
    let toml = toml::to_string(manifest).map_err(|e| {
        CoreError::Pipeline(crate::error::PipelineError::MetaSerialize(Box::new(e)))
    })?;
    crate::storage::atomic_replace(&journal_dir.join("manifest.toml"), toml.as_bytes())
        .map_err(CoreError::Storage)
}

/// Reads and parses `journal/manifest.toml` under `journal_dir`.
///
/// # Errors
///
/// Returns `CoreError::Storage` if the file cannot be read, or
/// `CoreError::Pipeline` if it cannot be parsed as a `JournalManifest`.
pub fn read_manifest(journal_dir: &Path) -> Result<JournalManifest, CoreError> {
    let raw = std::fs::read_to_string(journal_dir.join("manifest.toml"))
        .map_err(|e| CoreError::Storage(StorageError::from(e)))?;
    toml::from_str(&raw)
        .map_err(|e| CoreError::Pipeline(crate::error::PipelineError::MetaSerialize(Box::new(e))))
}

/// Handle to a running per-source journal writer thread.
///
/// `push` is non-blocking: it only enqueues onto an unbounded channel,
/// so a stalled disk backs up memory rather than the capture path.
/// The actual file I/O — including the per-segment `fsync` — happens
/// entirely on the writer's dedicated thread.
pub struct JournalWriter {
    tx: Sender<JournalMsg>,
    handle: Option<JoinHandle<Result<JournalSummary, CoreError>>>,
}

impl JournalWriter {
    /// Spawns the writer thread for `source`, appending rotating
    /// segments under `journal_dir` (created if absent). `sample_rate`
    /// and `channels` are taken from the first frame this source ever
    /// produces this session and are assumed stable for its lifetime,
    /// matching every other adapter's stability guarantee.
    ///
    /// # Errors
    ///
    /// Returns `CoreError::Storage` if the OS thread cannot be
    /// spawned.
    pub fn spawn(
        journal_dir: &Path,
        source: FrameSource,
        sample_rate: u32,
        channels: u16,
    ) -> Result<Self, CoreError> {
        let (tx, rx) = channel();
        let journal_dir = journal_dir.to_path_buf();
        let rotate_at_samples =
            u64::from(ROTATE_INTERVAL_SECS) * u64::from(sample_rate) * u64::from(channels.max(1));
        let handle = std::thread::Builder::new()
            .name(format!("scrybe-journal-{}", source_tag(source)))
            .spawn(move || {
                run_writer(
                    &rx,
                    &journal_dir,
                    source,
                    sample_rate,
                    channels,
                    rotate_at_samples,
                )
            })
            .map_err(|e| CoreError::Storage(StorageError::from(e)))?;
        Ok(Self {
            tx,
            handle: Some(handle),
        })
    }

    /// Enqueues `samples` (interleaved PCM at this writer's native
    /// rate/channel count) for the writer thread. Never touches disk
    /// on the calling thread. Silently dropped if the writer thread
    /// has already exited after a prior I/O failure — `finish` (or
    /// `Drop`) surfaces that failure to the caller.
    pub fn push(&self, samples: Arc<[f32]>) {
        let _ = self.tx.send(JournalMsg::Frame(samples));
    }

    /// Signals the writer to close its current segment and exit, then
    /// joins the thread and returns its summary.
    ///
    /// # Errors
    ///
    /// Returns `CoreError::Storage` if any segment write, flush, or
    /// `fsync` failed on the writer thread, or if the writer thread
    /// panicked.
    pub fn finish(mut self) -> Result<JournalSummary, CoreError> {
        let _ = self.tx.send(JournalMsg::Finish);
        join_writer(self.handle.take())
    }
}

impl Drop for JournalWriter {
    fn drop(&mut self) {
        if self.handle.is_some() {
            let _ = self.tx.send(JournalMsg::Finish);
            let _ = join_writer(self.handle.take());
        }
    }
}

fn join_writer(
    handle: Option<JoinHandle<Result<JournalSummary, CoreError>>>,
) -> Result<JournalSummary, CoreError> {
    let Some(handle) = handle else {
        return Err(CoreError::Storage(StorageError::Io(std::io::Error::other(
            "journal writer already finished",
        ))));
    };
    handle.join().unwrap_or_else(|_| {
        Err(CoreError::Storage(StorageError::Io(std::io::Error::other(
            "journal writer thread panicked",
        ))))
    })
}

const fn source_tag(source: FrameSource) -> &'static str {
    match source {
        FrameSource::Mic | FrameSource::Mixed => "mic",
        FrameSource::System => "system",
    }
}

fn samples_to_le_bytes(samples: &[f32]) -> Vec<u8> {
    let mut buf = Vec::with_capacity(samples.len() * 4);
    for sample in samples {
        buf.extend_from_slice(&sample.to_le_bytes());
    }
    buf
}

fn open_segment(dir: &Path, tag: &str, seq: u32) -> Result<File, CoreError> {
    let path = segment_path(dir, tag, seq);
    OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open(path)
        .map_err(|e| CoreError::Storage(StorageError::from(e)))
}

/// Path a segment `seq` for `tag` is written to under `dir`. Exposed
/// so the offline merge stage can rediscover segments by the same
/// naming contract without re-deriving it.
#[must_use]
pub fn segment_path(dir: &Path, tag: &str, seq: u32) -> PathBuf {
    dir.join(format!("{tag}-{seq:04}.f32"))
}

fn close_segment(mut writer: BufWriter<File>) -> Result<(), CoreError> {
    writer
        .flush()
        .map_err(|e| CoreError::Storage(StorageError::from(e)))?;
    full_fsync(writer.get_ref()).map_err(|e| CoreError::Storage(StorageError::from(e)))?;
    Ok(())
}

fn run_writer(
    rx: &Receiver<JournalMsg>,
    journal_dir: &Path,
    source: FrameSource,
    sample_rate: u32,
    channels: u16,
    rotate_at_samples: u64,
) -> Result<JournalSummary, CoreError> {
    std::fs::create_dir_all(journal_dir).map_err(|e| CoreError::Storage(StorageError::from(e)))?;
    let tag = source_tag(source);
    let mut seq: u32 = 0;
    let mut segment_samples: u64 = 0;
    let mut frames_written: u64 = 0;
    let mut segment_count: u32 = 0;
    let mut writer: Option<BufWriter<File>> = None;
    let channel_divisor = u64::from(channels.max(1));

    while let Ok(msg) = rx.recv() {
        let samples = match msg {
            JournalMsg::Frame(samples) => samples,
            JournalMsg::Finish => break,
        };
        if samples.is_empty() {
            continue;
        }
        let mut w = if let Some(w) = writer.take() {
            w
        } else {
            let file = open_segment(journal_dir, tag, seq)?;
            segment_count += 1;
            BufWriter::new(file)
        };
        w.write_all(&samples_to_le_bytes(&samples))
            .map_err(|e| CoreError::Storage(StorageError::from(e)))?;
        segment_samples += samples.len() as u64;
        frames_written += samples.len() as u64 / channel_divisor;
        if segment_samples >= rotate_at_samples {
            close_segment(w)?;
            seq += 1;
            segment_samples = 0;
        } else {
            writer = Some(w);
        }
    }
    if let Some(w) = writer.take() {
        close_segment(w)?;
    }

    Ok(JournalSummary {
        source,
        sample_rate,
        channels,
        frames_written,
        segment_count,
    })
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::cast_precision_loss
)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;
    use tempfile::tempdir;

    fn read_segment_samples(path: &Path) -> Vec<f32> {
        let bytes = std::fs::read(path).unwrap();
        bytes
            .chunks_exact(4)
            .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
            .collect()
    }

    #[test]
    fn test_journal_writer_writes_samples_to_single_segment_below_rotation_threshold() {
        let dir = tempdir().unwrap();
        let journal_dir = dir.path().join("journal");
        let writer = JournalWriter::spawn(&journal_dir, FrameSource::Mic, 100, 1).unwrap();

        let samples: Arc<[f32]> = (0..50).map(|i| i as f32 * 0.01).collect();
        writer.push(samples.clone());
        let summary = writer.finish().unwrap();

        assert_eq!(summary.source, FrameSource::Mic);
        assert_eq!(summary.sample_rate, 100);
        assert_eq!(summary.channels, 1);
        assert_eq!(summary.frames_written, 50);
        assert_eq!(summary.segment_count, 1);

        let seg0 = segment_path(&journal_dir, "mic", 0);
        assert!(seg0.exists());
        assert!(!segment_path(&journal_dir, "mic", 1).exists());
        let on_disk = read_segment_samples(&seg0);
        assert_eq!(on_disk.len(), 50);
        for (a, b) in on_disk.iter().zip(samples.iter()) {
            assert!((a - b).abs() < 1e-9);
        }
    }

    #[test]
    fn test_journal_writer_rotates_after_crossing_sample_threshold() {
        let dir = tempdir().unwrap();
        let journal_dir = dir.path().join("journal");
        // 30s rotation window at 100 Hz mono = 3000 samples.
        let writer = JournalWriter::spawn(&journal_dir, FrameSource::System, 100, 1).unwrap();

        let first: Arc<[f32]> = vec![0.1_f32; 3000].into();
        let second: Arc<[f32]> = vec![0.2_f32; 10].into();
        writer.push(first);
        writer.push(second);
        let summary = writer.finish().unwrap();

        assert_eq!(summary.frames_written, 3010);
        assert_eq!(summary.segment_count, 2);

        let seg0 = read_segment_samples(&segment_path(&journal_dir, "system", 0));
        let seg1 = read_segment_samples(&segment_path(&journal_dir, "system", 1));
        assert_eq!(
            seg0.len(),
            3000,
            "first segment holds exactly the frame that crossed the threshold"
        );
        assert_eq!(
            seg1.len(),
            10,
            "second segment holds only what arrived after rotation"
        );
    }

    #[test]
    fn test_journal_writer_rotation_never_splits_a_frame() {
        let dir = tempdir().unwrap();
        let journal_dir = dir.path().join("journal");
        let writer = JournalWriter::spawn(&journal_dir, FrameSource::Mic, 100, 1).unwrap();

        // A single frame larger than the rotation threshold (3000)
        // must land entirely in segment 0 — rotation only takes
        // effect between frames, never inside one.
        let oversized: Arc<[f32]> = vec![0.5_f32; 5000].into();
        writer.push(oversized);
        let next: Arc<[f32]> = vec![0.6_f32; 1].into();
        writer.push(next);
        let summary = writer.finish().unwrap();

        assert_eq!(summary.segment_count, 2);
        let seg0 = read_segment_samples(&segment_path(&journal_dir, "mic", 0));
        assert_eq!(seg0.len(), 5000);
    }

    #[test]
    fn test_journal_writer_multi_channel_frame_written_interleaved() {
        let dir = tempdir().unwrap();
        let journal_dir = dir.path().join("journal");
        let writer = JournalWriter::spawn(&journal_dir, FrameSource::Mic, 100, 2).unwrap();

        let stereo: Arc<[f32]> = vec![1.0_f32, -1.0_f32, 0.5_f32, -0.5_f32].into();
        writer.push(stereo);
        let summary = writer.finish().unwrap();

        // 4 interleaved samples / 2 channels = 2 frames per channel.
        assert_eq!(summary.frames_written, 2);
        let on_disk = read_segment_samples(&segment_path(&journal_dir, "mic", 0));
        assert_eq!(on_disk, vec![1.0, -1.0, 0.5, -0.5]);
    }

    #[test]
    fn test_journal_writer_mixed_source_shares_mic_tag() {
        let dir = tempdir().unwrap();
        let journal_dir = dir.path().join("journal");
        let writer = JournalWriter::spawn(&journal_dir, FrameSource::Mixed, 100, 1).unwrap();

        writer.push(vec![0.0_f32; 4].into());
        let summary = writer.finish().unwrap();

        assert_eq!(summary.source, FrameSource::Mixed);
        assert!(segment_path(&journal_dir, "mic", 0).exists());
        assert!(!segment_path(&journal_dir, "system", 0).exists());
    }

    #[test]
    fn test_journal_writer_empty_frame_is_ignored() {
        let dir = tempdir().unwrap();
        let journal_dir = dir.path().join("journal");
        let writer = JournalWriter::spawn(&journal_dir, FrameSource::Mic, 100, 1).unwrap();

        writer.push(Arc::from(Vec::<f32>::new()));
        let summary = writer.finish().unwrap();

        assert_eq!(summary.frames_written, 0);
        assert_eq!(
            summary.segment_count, 0,
            "no segment file created for an all-empty journal"
        );
    }

    #[test]
    fn test_journal_writer_finish_with_no_frames_creates_no_segments() {
        let dir = tempdir().unwrap();
        let journal_dir = dir.path().join("journal");
        let writer = JournalWriter::spawn(&journal_dir, FrameSource::System, 100, 1).unwrap();

        let summary = writer.finish().unwrap();

        assert_eq!(summary.frames_written, 0);
        assert_eq!(summary.segment_count, 0);
        assert!(journal_dir.exists(), "directory is still created up front");
    }

    #[test]
    fn test_journal_writer_push_after_finish_is_a_silent_no_op() {
        // Regression guard for the Drop/finish interaction: pushing
        // after `finish()` consumed the writer is a compile error, so
        // this test instead proves `Drop` on an un-finished writer
        // still durably closes the open segment.
        let dir = tempdir().unwrap();
        let journal_dir = dir.path().join("journal");
        {
            let writer = JournalWriter::spawn(&journal_dir, FrameSource::Mic, 100, 1).unwrap();
            writer.push(vec![0.3_f32; 20].into());
            // Dropped without calling `finish()`.
        }

        let seg0 = segment_path(&journal_dir, "mic", 0);
        assert!(seg0.exists(), "Drop must flush and close the open segment");
        assert_eq!(read_segment_samples(&seg0).len(), 20);
    }

    #[test]
    fn test_journal_manifest_round_trips_both_sources_through_toml() {
        let dir = tempdir().unwrap();
        let journal_dir = dir.path().join("journal");
        std::fs::create_dir_all(&journal_dir).unwrap();
        let manifest = JournalManifest {
            mic: Some(JournalAnchor {
                first_frame_epoch_ms: 1_735_000_000_123,
                sample_rate: 16_000,
                channels: 1,
                frames_written: 960_000,
            }),
            system: Some(JournalAnchor {
                first_frame_epoch_ms: 1_735_000_000_163,
                sample_rate: 48_000,
                channels: 2,
                frames_written: 2_880_000,
            }),
        };

        write_manifest(&journal_dir, &manifest).unwrap();
        let read_back = read_manifest(&journal_dir).unwrap();

        assert_eq!(read_back, manifest);
    }

    #[test]
    fn test_journal_manifest_round_trips_mic_only_session() {
        let dir = tempdir().unwrap();
        let journal_dir = dir.path().join("journal");
        std::fs::create_dir_all(&journal_dir).unwrap();
        let manifest = JournalManifest {
            mic: Some(JournalAnchor {
                first_frame_epoch_ms: 1_735_000_000_000,
                sample_rate: 16_000,
                channels: 1,
                frames_written: 480_000,
            }),
            system: None,
        };

        write_manifest(&journal_dir, &manifest).unwrap();
        let read_back = read_manifest(&journal_dir).unwrap();

        assert_eq!(read_back, manifest);
        assert!(read_back.system.is_none());
    }

    #[test]
    fn test_journal_manifest_omits_absent_source_from_toml_text() {
        let dir = tempdir().unwrap();
        let journal_dir = dir.path().join("journal");
        std::fs::create_dir_all(&journal_dir).unwrap();
        let manifest = JournalManifest {
            mic: Some(JournalAnchor {
                first_frame_epoch_ms: 0,
                sample_rate: 16_000,
                channels: 1,
                frames_written: 0,
            }),
            system: None,
        };
        write_manifest(&journal_dir, &manifest).unwrap();

        let raw = std::fs::read_to_string(journal_dir.join("manifest.toml")).unwrap();

        assert!(raw.contains("[mic]"));
        assert!(
            !raw.contains("[system]"),
            "absent source must not appear in the TOML at all, not as a null/empty table"
        );
    }

    #[test]
    fn test_journal_manifest_read_missing_file_returns_storage_error() {
        let dir = tempdir().unwrap();
        let journal_dir = dir.path().join("journal");
        std::fs::create_dir_all(&journal_dir).unwrap();

        let err = read_manifest(&journal_dir).unwrap_err();

        assert!(matches!(err, CoreError::Storage(_)));
    }

    #[test]
    fn test_journal_manifest_read_malformed_toml_returns_pipeline_error() {
        let dir = tempdir().unwrap();
        let journal_dir = dir.path().join("journal");
        std::fs::create_dir_all(&journal_dir).unwrap();
        std::fs::write(journal_dir.join("manifest.toml"), b"not = [valid toml").unwrap();

        let err = read_manifest(&journal_dir).unwrap_err();

        assert!(matches!(err, CoreError::Pipeline(_)));
    }
}
