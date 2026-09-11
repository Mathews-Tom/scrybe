// Copyright 2026 Mathews Tom
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//     https://www.apache.org/licenses/LICENSE-2.0

//! Offline journal merge.
//!
//! Turns the per-source `.f32` segments a [`crate::pipeline::journal`]
//! session produced into the final `audio.opus`, entirely after
//! capture ends. Each source is read back from disk, downmixed to
//! mono, resampled to the encoder's target rate, and — when both
//! sources are present — the later-starting one (per each source's
//! `first_frame_epoch_ms` anchor) is silence-prefixed by the wall-
//! clock delta before the two are interleaved into stereo. The merge
//! encodes once, asserts the result is within 1% of the session's
//! real wall-clock duration, and only deletes the journal after that
//! assertion passes — a failed assertion leaves both the journal and
//! any prior `audio.opus` untouched so `scrybe repair` can retry.
//!
//! This closes defect D1 (audio durability: the journal survives a
//! crash, unlike the old push/drain/encode-as-you-go path) and
//! defect D2 (undefined cross-source clock origin: the epoch anchors
//! from `pipeline::journal` give merge a real wall-clock basis for
//! alignment instead of comparing `AudioFrame::timestamp_ns` across
//! sources).

use std::path::Path;

use crate::error::{CoreError, PipelineError, StorageError};
use crate::pipeline::encoder::{default_session_encoder, EncoderConfig};
use crate::pipeline::journal::{segment_path, JournalAnchor, JournalManifest};
use crate::pipeline::resample::resample_linear;
use crate::storage::atomic_replace;

/// Result of a successful merge, carrying what the caller needs to
/// build `meta.toml`'s `[audio]` block without re-deriving it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MergeReport {
    pub encoded_secs: f64,
    pub channels: u16,
}

/// Fraction of a source-local capture-clock span the journal duration may
/// deviate by before the merge fails loudly. 1% per
/// `DEVELOPMENT_PLAN.md`'s audio-correctness acceptance criteria.
const DURATION_TOLERANCE_RATIO: f64 = 0.01;

/// Runs the complete offline merge for one session.
///
/// On success, atomically writes `audio_path` and deletes
/// `journal_dir` — the journal is only ever removed after a
/// verified-correct merge. On a duration-assertion failure,
/// `audio_path` is left untouched and `journal_dir` is preserved so
/// `scrybe repair` can retry once the underlying cause is fixed.
///
/// # Errors
///
/// `CoreError::Pipeline(PipelineError::EmptyJournal)` if `manifest`
/// names a source with no readable segment bytes on disk.
/// `CoreError::Pipeline(PipelineError::DurationMismatch)` if a current-format
/// source manifest's journal duration differs from its source-local capture
/// clock span by more than 1%. Old manifests without source-clock bounds skip
/// that validation so `scrybe repair` remains backward-compatible.
/// `CoreError::Storage` for any underlying I/O failure. Other
/// `CoreError::Pipeline` variants for resample or encoder failures.
pub fn merge_journal(
    journal_dir: &Path,
    audio_path: &Path,
    manifest: &JournalManifest,
    encoder_config: EncoderConfig,
    verify_duration: bool,
) -> Result<MergeReport, CoreError> {
    let mic_pcm = manifest
        .mic
        .map(|anchor| load_source(journal_dir, "mic", anchor, encoder_config.sample_rate))
        .transpose()?;
    let system_pcm = manifest
        .system
        .map(|anchor| load_source(journal_dir, "system", anchor, encoder_config.sample_rate))
        .transpose()?;
    if verify_duration {
        validate_source_duration(mic_pcm.as_deref(), manifest.mic, encoder_config.sample_rate)?;
        validate_source_duration(
            system_pcm.as_deref(),
            manifest.system,
            encoder_config.sample_rate,
        )?;
    }
    let delta_ms = match (manifest.mic, manifest.system) {
        (Some(mic), Some(system)) => system.first_frame_epoch_ms - mic.first_frame_epoch_ms,
        _ => 0,
    };

    let (mut pcm, channels) = match (mic_pcm, system_pcm) {
        (Some(mic), Some(system)) => (
            interleave_stereo(mic, system, delta_ms, encoder_config.sample_rate),
            2u16,
        ),
        (Some(mono), None) | (None, Some(mono)) => (mono, 1u16),
        (None, None) => (Vec::new(), 1u16),
    };

    let final_config = EncoderConfig {
        channels,
        ..encoder_config
    };
    #[allow(clippy::cast_precision_loss)]
    let total_frames = (pcm.len() / usize::from(channels.max(1))) as f64;
    let encoded_secs = total_frames / f64::from(final_config.sample_rate.max(1));

    let mut encoder = default_session_encoder(final_config).map_err(CoreError::Pipeline)?;
    let mut bytes = encoder.push_pcm(&pcm).map_err(CoreError::Pipeline)?;
    bytes.extend(encoder.finish().map_err(CoreError::Pipeline)?);

    if channels == 2 {
        center_stereo_in_place(&mut pcm);
        let mut playback_encoder =
            default_session_encoder(final_config).map_err(CoreError::Pipeline)?;
        let mut playback_bytes = playback_encoder
            .push_pcm(&pcm)
            .map_err(CoreError::Pipeline)?;
        playback_bytes.extend(playback_encoder.finish().map_err(CoreError::Pipeline)?);
        let playback_path = audio_path.with_file_name("playback.opus");
        atomic_replace(&playback_path, &playback_bytes).map_err(CoreError::Storage)?;
    }

    atomic_replace(audio_path, &bytes).map_err(CoreError::Storage)?;
    if let Err(e) = std::fs::remove_dir_all(journal_dir) {
        if e.kind() != std::io::ErrorKind::NotFound {
            return Err(CoreError::Storage(StorageError::from(e)));
        }
    }

    Ok(MergeReport {
        encoded_secs,
        channels,
    })
}

/// Ensures one source's persisted PCM covers the same interval as its own
/// capture clock. Source timestamps are deliberately never compared across
/// sources; epoch anchors remain responsible for cross-source alignment.
fn validate_source_duration(
    pcm: Option<&[f32]>,
    anchor: Option<JournalAnchor>,
    sample_rate: u32,
) -> Result<(), CoreError> {
    let (Some(pcm), Some(anchor)) = (pcm, anchor) else {
        return Ok(());
    };
    let (Some(first_timestamp_ns), Some(last_end_timestamp_ns)) = (
        anchor.first_frame_timestamp_ns,
        anchor.last_frame_end_timestamp_ns,
    ) else {
        return Ok(());
    };
    let capture_span_ns = last_end_timestamp_ns.saturating_sub(first_timestamp_ns);
    #[allow(clippy::cast_precision_loss)]
    let journal_secs = pcm.len() as f64 / f64::from(sample_rate.max(1));
    #[allow(clippy::cast_precision_loss)]
    let capture_secs = capture_span_ns as f64 / 1_000_000_000.0;
    let ratio = if capture_secs > 0.0 {
        (journal_secs - capture_secs).abs() / capture_secs
    } else {
        f64::INFINITY
    };
    if ratio > DURATION_TOLERANCE_RATIO {
        return Err(CoreError::Pipeline(PipelineError::DurationMismatch {
            encoded_secs: journal_secs,
            wall_clock_secs: capture_secs,
            ratio_pct: ratio * 100.0,
        }));
    }
    Ok(())
}

/// Reads every segment for `tag`, downmixes to mono, and resamples to
/// `target_sample_rate`.
fn load_source(
    journal_dir: &Path,
    tag: &str,
    anchor: JournalAnchor,
    target_sample_rate: u32,
) -> Result<Vec<f32>, CoreError> {
    let raw = read_segments(journal_dir, tag)?;
    if raw.is_empty() {
        return Err(CoreError::Pipeline(PipelineError::EmptyJournal {
            source_tag: tag.to_string(),
        }));
    }
    let mono = downmix_to_mono(&raw, anchor.channels);
    if anchor.sample_rate == target_sample_rate {
        Ok(mono)
    } else {
        resample_linear(&mono, anchor.sample_rate, target_sample_rate)
            .map_err(|e| CoreError::Pipeline(e.into()))
    }
}

/// Reads and concatenates every rotated segment for `tag`, in order,
/// starting at segment 0 until a segment file is missing. A
/// truncated final segment (a crash mid-write leaves a byte count
/// that is not a multiple of 4) drops its trailing partial sample
/// rather than erroring — the recovered duration reflects exactly
/// the complete samples on disk.
fn read_segments(journal_dir: &Path, tag: &str) -> Result<Vec<f32>, CoreError> {
    let mut out = Vec::new();
    let mut seq: u32 = 0;
    loop {
        let path = segment_path(journal_dir, tag, seq);
        if !path.exists() {
            break;
        }
        let bytes = std::fs::read(&path).map_err(|e| CoreError::Storage(StorageError::from(e)))?;
        let usable_len = bytes.len() - (bytes.len() % 4);
        out.extend(
            bytes[..usable_len]
                .as_chunks::<4>()
                .0
                .iter()
                .map(|c| f32::from_le_bytes(*c)),
        );
        seq += 1;
    }
    Ok(out)
}

#[allow(clippy::cast_precision_loss)]
fn downmix_to_mono(samples: &[f32], channels: u16) -> Vec<f32> {
    if channels <= 1 {
        return samples.to_vec();
    }
    let chans = usize::from(channels);
    let inv = 1.0_f32 / f32::from(channels);
    samples
        .chunks_exact(chans)
        .map(|chunk| chunk.iter().sum::<f32>() * inv)
        .collect()
}

/// Silence-prefixes whichever source started later (per `delta_ms =
/// system_epoch - mic_epoch`) by `|delta_ms| * sample_rate / 1000`
/// samples, zero-pads the shorter side to match the longer, and
/// interleaves into `[L, R, L, R, ...]` PCM (L=mic, R=system).
fn interleave_stereo(
    mut mic: Vec<f32>,
    mut system: Vec<f32>,
    delta_ms: i64,
    sample_rate: u32,
) -> Vec<f32> {
    let prefix_samples = (delta_ms.unsigned_abs() * u64::from(sample_rate) / 1000) as usize;
    if delta_ms > 0 {
        // System's first frame arrived after mic's: system started
        // later, so it gets the silence prefix.
        let mut prefixed = vec![0.0_f32; prefix_samples];
        prefixed.extend(system);
        system = prefixed;
    } else if delta_ms < 0 {
        let mut prefixed = vec![0.0_f32; prefix_samples];
        prefixed.extend(mic);
        mic = prefixed;
    }
    let len = mic.len().max(system.len());
    mic.resize(len, 0.0);
    system.resize(len, 0.0);
    let mut out = Vec::with_capacity(len * 2);
    for i in 0..len {
        out.push(mic[i]);
        out.push(system[i]);
    }
    out
}
fn center_stereo_in_place(samples: &mut [f32]) {
    for frame in samples.as_chunks_mut::<2>().0 {
        let centered = f32::midpoint(frame[0], frame[1]);
        frame[0] = centered;
        frame[1] = centered;
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
    use super::*;
    use crate::pipeline::journal::JournalWriter;
    use crate::types::FrameSource;
    use pretty_assertions::assert_eq;
    use std::sync::Arc;
    use tempfile::tempdir;

    fn write_journal_segment(dir: &Path, tag: &str, seq: u32, samples: &[f32]) {
        std::fs::create_dir_all(dir).unwrap();
        let mut bytes = Vec::with_capacity(samples.len() * 4);
        for s in samples {
            bytes.extend_from_slice(&s.to_le_bytes());
        }
        std::fs::write(segment_path(dir, tag, seq), bytes).unwrap();
    }

    fn sine(n: usize, freq_scale: f32) -> Vec<f32> {
        (0..n)
            .map(|i| (i as f32 * freq_scale).sin() * 0.5)
            .collect()
    }

    #[test]
    fn test_merge_mono_session_round_trips_within_tolerance() {
        let tmp = tempdir().unwrap();
        let journal_dir = tmp.path().join("journal");
        let audio_path = tmp.path().join("audio.opus");
        // 1 second of mono audio at 16 kHz native rate.
        write_journal_segment(&journal_dir, "mic", 0, &sine(16_000, 0.01));
        let manifest = JournalManifest {
            mic: Some(JournalAnchor {
                first_frame_epoch_ms: 1000,
                sample_rate: 16_000,
                channels: 1,
                frames_written: 16_000,
                first_frame_timestamp_ns: None,
                last_frame_end_timestamp_ns: None,
            }),
            system: None,
        };

        let report = merge_journal(
            &journal_dir,
            &audio_path,
            &manifest,
            EncoderConfig {
                sample_rate: 16_000,
                ..EncoderConfig::default()
            },
            true,
        )
        .unwrap();

        assert_eq!(report.channels, 1);
        assert!((report.encoded_secs - 1.0).abs() < 0.01);
        assert!(audio_path.exists());
        assert!(!journal_dir.exists(), "journal must be deleted on success");
    }

    #[test]
    fn test_merge_stereo_session_silence_prefixes_later_starting_system_by_exact_delta() {
        let tmp = tempdir().unwrap();
        let journal_dir = tmp.path().join("journal");
        let audio_path = tmp.path().join("audio.opus");
        let sample_rate = 1_000_u32; // Small rate so sample-exact assertions are easy.
                                     // 1 second of mic, 1 second of system, both at 1 kHz.
        write_journal_segment(&journal_dir, "mic", 0, &sine(1_000, 0.05));
        write_journal_segment(&journal_dir, "system", 0, &sine(1_000, 0.07));
        let mic_epoch = 1_000_i64;
        let delta_ms = 40_i64;
        let manifest = JournalManifest {
            mic: Some(JournalAnchor {
                first_frame_epoch_ms: mic_epoch,
                sample_rate,
                channels: 1,
                frames_written: 1_000,
                first_frame_timestamp_ns: None,
                last_frame_end_timestamp_ns: None,
            }),
            system: Some(JournalAnchor {
                // System started 40ms after mic.
                first_frame_epoch_ms: mic_epoch + delta_ms,
                sample_rate,
                channels: 1,
                frames_written: 1_000,
                first_frame_timestamp_ns: None,
                last_frame_end_timestamp_ns: None,
            }),
        };

        let report = merge_journal(
            &journal_dir,
            &audio_path,
            &manifest,
            EncoderConfig {
                sample_rate,
                ..EncoderConfig::default()
            },
            true,
        )
        .unwrap();

        assert_eq!(report.channels, 2);
        // 40ms silence prefix at 1kHz = 40 samples on the system
        // (right) channel; those 40 R samples must be exactly zero,
        // and the corresponding L (mic) samples must be the real
        // signal (not silence), proving the prefix landed on the
        // correct (later-starting) side.
        let bytes = std::fs::read(&audio_path).unwrap();
        let pcm: Vec<f32> = bytes
            .as_chunks::<4>()
            .0
            .iter()
            .map(|chunk| f32::from_le_bytes(*chunk))
            .collect();
        for i in 0..40 {
            assert!(
                pcm[i * 2 + 1].abs() < 1e-9,
                "system sample {i} must be silence, got {}",
                pcm[i * 2 + 1]
            );
        }
        assert!(
            pcm[20] != 0.0 || pcm[38] != 0.0,
            "mic (L) channel must carry real signal during the prefix window, got pcm[20]={} pcm[38]={}",
            pcm[20],
            pcm[38]
        );
        let playback_bytes = std::fs::read(tmp.path().join("playback.opus")).unwrap();
        let playback_pcm: Vec<f32> = playback_bytes
            .as_chunks::<4>()
            .0
            .iter()
            .map(|chunk| f32::from_le_bytes(*chunk))
            .collect();
        assert_eq!(playback_pcm.len(), pcm.len());
        assert!(
            playback_pcm
                .as_chunks::<2>()
                .0
                .iter()
                .all(|frame| (frame[0] - frame[1]).abs() < f32::EPSILON),
            "playback.opus must center the source-separated master into both channels"
        );
    }

    #[test]
    fn test_merge_recovers_truncated_final_segment_without_erroring() {
        let tmp = tempdir().unwrap();
        let journal_dir = tmp.path().join("journal");
        let audio_path = tmp.path().join("audio.opus");
        std::fs::create_dir_all(&journal_dir).unwrap();
        // 8000 complete samples (0.5s at 16kHz) plus a truncated
        // trailing partial sample (2 stray bytes, as a crash mid-
        // write would leave).
        let mut bytes = Vec::new();
        for s in sine(8_000, 0.01) {
            bytes.extend_from_slice(&s.to_le_bytes());
        }
        bytes.extend_from_slice(&[0xAB, 0xCD]); // 2 trailing bytes: not a full f32.
        std::fs::write(segment_path(&journal_dir, "mic", 0), bytes).unwrap();
        let manifest = JournalManifest {
            mic: Some(JournalAnchor {
                first_frame_epoch_ms: 1000,
                sample_rate: 16_000,
                channels: 1,
                frames_written: 8_000,
                first_frame_timestamp_ns: None,
                last_frame_end_timestamp_ns: None,
            }),
            system: None,
        };

        // wall_clock_secs matches the recovered (truncated) duration,
        // as scrybe repair would compute after a crash.
        let report = merge_journal(
            &journal_dir,
            &audio_path,
            &manifest,
            EncoderConfig {
                sample_rate: 16_000,
                ..EncoderConfig::default()
            },
            false,
        )
        .unwrap();

        assert!((report.encoded_secs - 0.5).abs() < 0.01);
    }

    #[test]
    fn test_merge_fails_loudly_and_preserves_journal_when_source_clock_has_gap() {
        let tmp = tempdir().unwrap();
        let journal_dir = tmp.path().join("journal");
        let audio_path = tmp.path().join("audio.opus");
        // One second of journaled audio covers a claimed 60-second source-clock
        // span. This is the discontinuity a real capture path must reject.
        write_journal_segment(&journal_dir, "mic", 0, &sine(16_000, 0.01));
        let manifest = JournalManifest {
            mic: Some(JournalAnchor {
                first_frame_epoch_ms: 1000,
                first_frame_timestamp_ns: Some(0),
                last_frame_end_timestamp_ns: Some(60_000_000_000),
                sample_rate: 16_000,
                channels: 1,
                frames_written: 16_000,
            }),
            system: None,
        };
        let err = merge_journal(
            &journal_dir,
            &audio_path,
            &manifest,
            EncoderConfig {
                sample_rate: 16_000,
                ..EncoderConfig::default()
            },
            true,
        )
        .unwrap_err();

        assert!(matches!(
            err,
            CoreError::Pipeline(PipelineError::DurationMismatch { .. })
        ));
        assert!(
            journal_dir.exists(),
            "journal must be preserved on assertion failure for scrybe repair"
        );
        assert!(
            !audio_path.exists(),
            "audio.opus must not be written on assertion failure"
        );
    }

    #[test]
    fn test_merge_empty_journal_source_returns_pipeline_error() {
        let tmp = tempdir().unwrap();
        let journal_dir = tmp.path().join("journal");
        let audio_path = tmp.path().join("audio.opus");
        std::fs::create_dir_all(&journal_dir).unwrap();
        // Manifest claims a mic source but no segment file exists.
        let manifest = JournalManifest {
            mic: Some(JournalAnchor {
                first_frame_epoch_ms: 1000,
                sample_rate: 16_000,
                channels: 1,
                frames_written: 0,
                first_frame_timestamp_ns: None,
                last_frame_end_timestamp_ns: None,
            }),
            system: None,
        };

        let err = merge_journal(
            &journal_dir,
            &audio_path,
            &manifest,
            EncoderConfig::default(),
            true,
        )
        .unwrap_err();

        assert!(matches!(
            err,
            CoreError::Pipeline(PipelineError::EmptyJournal { .. })
        ));
    }

    #[test]
    fn test_merge_no_sources_produces_empty_audio_without_error() {
        let tmp = tempdir().unwrap();
        let journal_dir = tmp.path().join("journal");
        let audio_path = tmp.path().join("audio.opus");
        std::fs::create_dir_all(&journal_dir).unwrap();
        let manifest = JournalManifest::default();

        let report = merge_journal(
            &journal_dir,
            &audio_path,
            &manifest,
            EncoderConfig::default(),
            false,
        )
        .unwrap();

        assert!(report.encoded_secs.abs() < 1e-9);
    }

    #[test]
    fn test_merge_succeeds_when_journal_dir_was_never_created() {
        // Regression guard: a session where no source ever produced a
        // frame never spawns a JournalWriter, so `journal/` is never
        // created on disk at all (JournalWriter::spawn is what calls
        // create_dir_all, lazily, on the first frame). merge_journal
        // must not fail trying to delete a directory that was never
        // there in the first place.
        let tmp = tempdir().unwrap();
        let journal_dir = tmp.path().join("journal-never-created");
        let audio_path = tmp.path().join("audio.opus");
        let manifest = JournalManifest::default();

        let report = merge_journal(
            &journal_dir,
            &audio_path,
            &manifest,
            EncoderConfig::default(),
            false,
        )
        .unwrap();

        assert!(report.encoded_secs.abs() < 1e-9);
        assert!(audio_path.exists());
    }

    #[test]
    fn test_merge_downmixes_multi_channel_journal_source_to_mono() {
        let tmp = tempdir().unwrap();
        let journal_dir = tmp.path().join("journal");
        let audio_path = tmp.path().join("audio.opus");
        // Stereo journal: L=1.0, R=-1.0 throughout. Downmix average = 0.0.
        let stereo: Vec<f32> = (0..1_000).flat_map(|_| [1.0_f32, -1.0_f32]).collect();
        write_journal_segment(&journal_dir, "mic", 0, &stereo);
        let manifest = JournalManifest {
            mic: Some(JournalAnchor {
                first_frame_epoch_ms: 1000,
                sample_rate: 1_000,
                channels: 2,
                frames_written: 1_000,
                first_frame_timestamp_ns: None,
                last_frame_end_timestamp_ns: None,
            }),
            system: None,
        };

        let report = merge_journal(
            &journal_dir,
            &audio_path,
            &manifest,
            EncoderConfig {
                sample_rate: 1_000,
                ..EncoderConfig::default()
            },
            false,
        )
        .unwrap();

        assert_eq!(report.channels, 1);
        let bytes = std::fs::read(&audio_path).unwrap();
        let pcm: Vec<f32> = bytes
            .as_chunks::<4>()
            .0
            .iter()
            .map(|chunk| f32::from_le_bytes(*chunk))
            .collect();
        assert!(pcm.iter().all(|&s| s.abs() < 1e-6));
    }

    #[test]
    fn test_merge_resamples_source_natively_below_target_rate() {
        let tmp = tempdir().unwrap();
        let journal_dir = tmp.path().join("journal");
        let audio_path = tmp.path().join("audio.opus");
        // 1 second at 8kHz native rate, merged to a 16kHz target.
        write_journal_segment(&journal_dir, "mic", 0, &sine(8_000, 0.02));
        let manifest = JournalManifest {
            mic: Some(JournalAnchor {
                first_frame_epoch_ms: 1000,
                sample_rate: 8_000,
                channels: 1,
                frames_written: 8_000,
                first_frame_timestamp_ns: None,
                last_frame_end_timestamp_ns: None,
            }),
            system: None,
        };

        let report = merge_journal(
            &journal_dir,
            &audio_path,
            &manifest,
            EncoderConfig {
                sample_rate: 16_000,
                ..EncoderConfig::default()
            },
            false,
        )
        .unwrap();

        // Still ~1 second: the merge resampled 8kHz -> 16kHz rather
        // than mislabeling the encoder's rate over the source's (the
        // D1 bug this whole release exists to fix).
        assert!((report.encoded_secs - 1.0).abs() < 0.01);
    }

    #[test]
    fn test_merge_uses_journal_writer_output_directly() {
        // End-to-end sanity: feed real JournalWriter output (not a
        // hand-crafted segment file) into merge_journal.
        let tmp = tempdir().unwrap();
        let journal_dir = tmp.path().join("journal");
        let audio_path = tmp.path().join("audio.opus");
        let writer = JournalWriter::spawn(&journal_dir, FrameSource::Mic, 16_000, 1).unwrap();
        let samples: Arc<[f32]> = sine(16_000, 0.01).into();
        writer.push(samples);
        let summary = writer.finish().unwrap();
        let manifest = JournalManifest {
            mic: Some(JournalAnchor {
                first_frame_epoch_ms: 1000,
                sample_rate: summary.sample_rate,
                channels: summary.channels,
                frames_written: summary.frames_written,
                first_frame_timestamp_ns: None,
                last_frame_end_timestamp_ns: None,
            }),
            system: None,
        };

        let report = merge_journal(
            &journal_dir,
            &audio_path,
            &manifest,
            EncoderConfig {
                sample_rate: 16_000,
                ..EncoderConfig::default()
            },
            false,
        )
        .unwrap();

        assert!((report.encoded_secs - 1.0).abs() < 0.01);
    }
}
