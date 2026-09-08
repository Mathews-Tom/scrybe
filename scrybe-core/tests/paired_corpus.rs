// Copyright 2026 Mathews Tom
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//     https://www.apache.org/licenses/LICENSE-2.0

//! Integration tests for `scrybe_core::testing::paired`: manifest
//! loading/validation (including the strict PCM16 WAV decode path)
//! and aggregate-report computation. Everything here drives the
//! module through its public API (`load_corpus`, `aggregate`) rather
//! than reaching into implementation details, so this file doubles as
//! the paired-corpus consumer contract (mirrors the sibling
//! `multilingual_corpus.rs` integration-test convention).

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::fmt::Write;
use std::path::Path;

use scrybe_core::testing::multilingual::word_error_rate;
use scrybe_core::testing::paired::{
    aggregate, load_corpus, AggregateError, ClipMeasurement, PairedCorpusError, PairedReport,
    BACKEND_SHERPA_ONNX, BACKEND_WHISPER_LOCAL, CURRENT_PAIRED_MANIFEST_VERSION,
};

const SAMPLE_RATE: u32 = 16_000;

fn pcm16_wav_bytes(samples: &[i16]) -> Vec<u8> {
    let channels: u16 = 1;
    let bits_per_sample: u16 = 16;
    let block_align = channels * bits_per_sample / 8;
    let byte_rate = SAMPLE_RATE * u32::from(block_align);
    let data_len = u32::try_from(samples.len() * 2).unwrap();

    let mut out = Vec::new();
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + data_len).to_le_bytes());
    out.extend_from_slice(b"WAVE");
    out.extend_from_slice(b"fmt ");
    out.extend_from_slice(&16_u32.to_le_bytes());
    out.extend_from_slice(&1_u16.to_le_bytes());
    out.extend_from_slice(&channels.to_le_bytes());
    out.extend_from_slice(&SAMPLE_RATE.to_le_bytes());
    out.extend_from_slice(&byte_rate.to_le_bytes());
    out.extend_from_slice(&block_align.to_le_bytes());
    out.extend_from_slice(&bits_per_sample.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&data_len.to_le_bytes());
    for sample in samples {
        out.extend_from_slice(&sample.to_le_bytes());
    }
    out
}

/// Build a raw WAV byte buffer with an arbitrary sample rate/bit
/// depth, for the format-rejection tests below.
fn wav_bytes_with(sample_rate: u32, bits_per_sample: u16, samples_le_bytes: &[u8]) -> Vec<u8> {
    let channels: u16 = 1;
    let block_align = channels * (bits_per_sample / 8);
    let byte_rate = sample_rate * u32::from(block_align);
    let data_len = u32::try_from(samples_le_bytes.len()).unwrap();

    let mut out = Vec::new();
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + data_len).to_le_bytes());
    out.extend_from_slice(b"WAVE");
    out.extend_from_slice(b"fmt ");
    out.extend_from_slice(&16_u32.to_le_bytes());
    out.extend_from_slice(&1_u16.to_le_bytes());
    out.extend_from_slice(&channels.to_le_bytes());
    out.extend_from_slice(&sample_rate.to_le_bytes());
    out.extend_from_slice(&byte_rate.to_le_bytes());
    out.extend_from_slice(&block_align.to_le_bytes());
    out.extend_from_slice(&bits_per_sample.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&data_len.to_le_bytes());
    out.extend_from_slice(samples_le_bytes);
    out
}

fn hex_encode_lower(bytes: &[u8]) -> String {
    const HEX_DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(HEX_DIGITS[usize::from(byte >> 4)] as char);
        out.push(HEX_DIGITS[usize::from(byte & 0x0f)] as char);
    }
    out
}

fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    hex_encode_lower(&Sha256::digest(bytes))
}

/// Write a single-clip manifest + its audio file into a fresh tempdir
/// and return `(dir, manifest_path)`. `audio_bytes` is written
/// verbatim (already-encoded WAV bytes, valid or deliberately not),
/// and the manifest's `sha256` is always the real hash of exactly
/// those bytes so only the intended validation step fires.
fn write_single_clip_corpus(
    id: &str,
    language: &str,
    reference: &str,
    provenance: &str,
    audio_bytes: &[u8],
) -> (tempfile::TempDir, std::path::PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let filename = format!("{id}.wav");
    std::fs::write(dir.path().join(&filename), audio_bytes).unwrap();
    let sha256 = sha256_hex(audio_bytes);
    let manifest_path = dir.path().join("MANIFEST.toml");
    std::fs::write(
        &manifest_path,
        format!(
            "schema_version = 1\n\n[[clips]]\nid = \"{id}\"\nlanguage = \"{language}\"\naudio = \"{filename}\"\nsha256 = \"{sha256}\"\nreference = \"{reference}\"\nprovenance = \"{provenance}\"\n"
        ),
    )
    .unwrap();
    (dir, manifest_path)
}

/// Lay out a multi-clip manifest + its referenced WAV files in a
/// fresh tempdir and return `(dir, manifest_path)`.
fn write_corpus(
    clips: &[(&str, &str, &str, &str, &[i16])],
) -> (tempfile::TempDir, std::path::PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let mut manifest = String::from("schema_version = 1\n");
    for &(id, language, reference, provenance, samples) in clips {
        let wav_bytes = pcm16_wav_bytes(samples);
        let filename = format!("{id}.wav");
        std::fs::write(dir.path().join(&filename), &wav_bytes).unwrap();
        let sha256 = sha256_hex(&wav_bytes);
        writeln!(manifest,
            "\n[[clips]]\nid = \"{id}\"\nlanguage = \"{language}\"\naudio = \"{filename}\"\nsha256 = \"{sha256}\"\nreference = \"{reference}\"\nprovenance = \"{provenance}\""
        ).unwrap();
    }
    let manifest_path = dir.path().join("MANIFEST.toml");
    std::fs::write(&manifest_path, manifest).unwrap();
    (dir, manifest_path)
}

const SAMPLES_A: [i16; 4] = [100, -100, 200, -200];
const SAMPLES_B: [i16; 6] = [1, 2, 3, 4, 5, 6];

// ---------------------------------------------------------------
// load_corpus: manifest-level validation
// ---------------------------------------------------------------

#[test]
fn test_load_corpus_returns_not_found_for_absent_manifest() {
    let err = load_corpus(Path::new("/no/such/MANIFEST.toml"))
        .err()
        .unwrap();

    assert!(matches!(err, PairedCorpusError::NotFound { .. }));
}

#[test]
fn test_load_corpus_rejects_unparseable_toml() {
    let dir = tempfile::tempdir().unwrap();
    let manifest_path = dir.path().join("MANIFEST.toml");
    std::fs::write(&manifest_path, "not = [valid").unwrap();

    let err = load_corpus(&manifest_path).err().unwrap();

    assert!(matches!(err, PairedCorpusError::Parse(_)));
}

#[test]
fn test_load_corpus_loads_valid_multi_clip_manifest_with_real_decoded_audio() {
    let (_dir, manifest_path) = write_corpus(&[
        (
            "clip-a",
            "en",
            "hello world",
            "recorded 2026-01-01",
            &SAMPLES_A,
        ),
        (
            "clip-b",
            "en",
            "a much longer reference sentence here",
            "recorded 2026-01-02",
            &SAMPLES_B,
        ),
    ]);

    let corpus = load_corpus(&manifest_path).unwrap();

    assert_eq!(corpus.clips.len(), 2);
    let clip_a = corpus.clips.iter().find(|c| c.id == "clip-a").unwrap();
    assert_eq!(clip_a.reference, "hello world");
    assert_eq!(clip_a.provenance, "recorded 2026-01-01");
    assert_eq!(clip_a.audio.samples.len(), SAMPLES_A.len());
    assert_eq!(clip_a.audio_sha256.len(), 64);
    assert!(clip_a.audio_sha256.bytes().all(|b| b.is_ascii_hexdigit()));
    let expected_secs = f64::from(u32::try_from(SAMPLES_A.len()).unwrap()) / f64::from(SAMPLE_RATE);
    assert!((clip_a.audio.duration.as_secs_f64() - expected_secs).abs() < 1e-9);
}

#[test]
fn test_load_corpus_rejects_schema_version_zero_and_future_version() {
    for bad_version in [0_u32, 2_u32] {
        let (_dir, manifest_path) = write_corpus(&[("clip-a", "en", "hello", "src", &SAMPLES_A)]);
        let text = std::fs::read_to_string(&manifest_path).unwrap();
        let text = text.replacen(
            "schema_version = 1",
            &format!("schema_version = {bad_version}"),
            1,
        );
        std::fs::write(&manifest_path, text).unwrap();

        let err = load_corpus(&manifest_path).err().unwrap();

        match err {
            PairedCorpusError::UnsupportedSchemaVersion { found, target } => {
                assert_eq!(found, bad_version);
                assert_eq!(target, CURRENT_PAIRED_MANIFEST_VERSION);
            }
            other => panic!("expected UnsupportedSchemaVersion for {bad_version}, got {other:?}"),
        }
    }
}

#[test]
fn test_load_corpus_rejects_empty_clip_list() {
    let dir = tempfile::tempdir().unwrap();
    let manifest_path = dir.path().join("MANIFEST.toml");
    std::fs::write(&manifest_path, "schema_version = 1\nclips = []\n").unwrap();

    let err = load_corpus(&manifest_path).err().unwrap();

    assert!(matches!(err, PairedCorpusError::Empty));
}

#[test]
fn test_load_corpus_rejects_duplicate_clip_id() {
    let dir = tempfile::tempdir().unwrap();
    let wav_bytes = pcm16_wav_bytes(&SAMPLES_A);
    std::fs::write(dir.path().join("dup.wav"), &wav_bytes).unwrap();
    let sha256 = sha256_hex(&wav_bytes);
    let manifest_path = dir.path().join("MANIFEST.toml");
    // Two `[[clips]]` array-of-tables entries (always legal TOML)
    // sharing the same id — only the id-uniqueness check should fire,
    // not a TOML-level duplicate-key parse error.
    std::fs::write(
        &manifest_path,
        format!(
            "schema_version = 1\n\n[[clips]]\nid = \"dup\"\nlanguage = \"en\"\naudio = \"dup.wav\"\nsha256 = \"{sha256}\"\nreference = \"one\"\nprovenance = \"src\"\n\n[[clips]]\nid = \"dup\"\nlanguage = \"en\"\naudio = \"dup.wav\"\nsha256 = \"{sha256}\"\nreference = \"two\"\nprovenance = \"src2\"\n"
        ),
    )
    .unwrap();

    let err = load_corpus(&manifest_path).err().unwrap();

    match err {
        PairedCorpusError::DuplicateClipId(id) => assert_eq!(id, "dup"),
        other => panic!("expected DuplicateClipId, got {other:?}"),
    }
}

#[test]
fn test_load_corpus_rejects_non_english_language() {
    let (_dir, manifest_path) = write_corpus(&[("clip-a", "fr", "bonjour", "src", &SAMPLES_A)]);

    let err = load_corpus(&manifest_path).err().unwrap();

    match err {
        PairedCorpusError::UnsupportedLanguage { id, found } => {
            assert_eq!(id, "clip-a");
            assert_eq!(found, "fr");
        }
        other => panic!("expected UnsupportedLanguage, got {other:?}"),
    }
}

#[test]
fn test_load_corpus_rejects_empty_reference_and_reports_field_name() {
    let (_dir, manifest_path) = write_corpus(&[("clip-a", "en", "", "src", &SAMPLES_A)]);

    let err = load_corpus(&manifest_path).err().unwrap();

    match err {
        PairedCorpusError::EmptyField { id, field } => {
            assert_eq!(id, "clip-a");
            assert_eq!(field, "reference");
        }
        other => panic!("expected EmptyField, got {other:?}"),
    }
}

#[test]
fn test_load_corpus_rejects_empty_provenance() {
    let (_dir, manifest_path) = write_corpus(&[("clip-a", "en", "hello", "", &SAMPLES_A)]);

    let err = load_corpus(&manifest_path).err().unwrap();

    match err {
        PairedCorpusError::EmptyField { field, .. } => assert_eq!(field, "provenance"),
        other => panic!("expected EmptyField, got {other:?}"),
    }
}

#[test]
fn test_load_corpus_rejects_checksum_mismatch() {
    let wav_bytes = pcm16_wav_bytes(&SAMPLES_A);
    let (_dir, manifest_path) =
        write_single_clip_corpus("clip-a", "en", "hello", "src", &wav_bytes);
    let text = std::fs::read_to_string(&manifest_path).unwrap();
    let corrupted = text.replacen(&sha256_hex(&wav_bytes), &"0".repeat(64), 1);
    assert_ne!(
        text, corrupted,
        "sha256 line must have been present to replace"
    );
    std::fs::write(&manifest_path, corrupted).unwrap();

    let err = load_corpus(&manifest_path).err().unwrap();

    match err {
        PairedCorpusError::ChecksumMismatch { id, expected, .. } => {
            assert_eq!(id, "clip-a");
            assert_eq!(expected, "0".repeat(64));
        }
        other => panic!("expected ChecksumMismatch, got {other:?}"),
    }
}

#[test]
fn test_load_corpus_rejects_malformed_sha256() {
    let wav_bytes = pcm16_wav_bytes(&SAMPLES_A);
    let (_dir, manifest_path) =
        write_single_clip_corpus("clip-a", "en", "hello", "src", &wav_bytes);
    let text = std::fs::read_to_string(&manifest_path).unwrap();
    let corrupted = text.replacen(&sha256_hex(&wav_bytes), "not-a-hash", 1);
    std::fs::write(&manifest_path, corrupted).unwrap();

    let err = load_corpus(&manifest_path).err().unwrap();

    assert!(matches!(err, PairedCorpusError::InvalidSha256Format { .. }));
}

#[test]
fn test_load_corpus_rejects_audio_path_escaping_corpus_root() {
    let outer = tempfile::tempdir().unwrap();
    let corpus_dir = outer.path().join("corpus");
    std::fs::create_dir(&corpus_dir).unwrap();
    let secret_wav = pcm16_wav_bytes(&SAMPLES_A);
    std::fs::write(outer.path().join("secret.wav"), &secret_wav).unwrap();

    let manifest_path = corpus_dir.join("MANIFEST.toml");
    let sha256 = sha256_hex(&secret_wav);
    std::fs::write(
        &manifest_path,
        format!(
            "schema_version = 1\n\n[[clips]]\nid = \"clip-a\"\nlanguage = \"en\"\naudio = \"../secret.wav\"\nsha256 = \"{sha256}\"\nreference = \"hello\"\nprovenance = \"src\"\n"
        ),
    )
    .unwrap();

    let err = load_corpus(&manifest_path).err().unwrap();

    match err {
        PairedCorpusError::InvalidAudioPath { detail, .. } => {
            assert!(
                detail.contains("outside the corpus root"),
                "detail: {detail}"
            );
        }
        other => panic!("expected InvalidAudioPath, got {other:?}"),
    }
}

#[test]
fn test_load_corpus_rejects_absolute_audio_path_inside_corpus_root() {
    let (dir, manifest_path) =
        write_single_clip_corpus("clip-a", "en", "hello", "src", &pcm16_wav_bytes(&SAMPLES_A));
    let audio_path = dir.path().join("clip-a.wav");
    let manifest = std::fs::read_to_string(&manifest_path).unwrap();
    let manifest = manifest.replacen(
        "audio = \"clip-a.wav\"",
        &format!("audio = {audio_path:?}"),
        1,
    );
    std::fs::write(&manifest_path, manifest).unwrap();

    let err = load_corpus(&manifest_path).err().unwrap();

    match err {
        PairedCorpusError::InvalidAudioPath { detail, .. } => {
            assert_eq!(detail, "audio must be a relative path");
        }
        other => panic!("expected InvalidAudioPath, got {other:?}"),
    }
}

#[test]
fn test_load_corpus_rejects_absent_audio_file() {
    let dir = tempfile::tempdir().unwrap();
    let manifest_path = dir.path().join("MANIFEST.toml");
    std::fs::write(
        &manifest_path,
        format!(
            "schema_version = 1\n\n[[clips]]\nid = \"clip-a\"\nlanguage = \"en\"\naudio = \"missing.wav\"\nsha256 = \"{}\"\nreference = \"hello\"\nprovenance = \"src\"\n",
            "0".repeat(64)
        ),
    )
    .unwrap();

    let err = load_corpus(&manifest_path).err().unwrap();

    assert!(matches!(err, PairedCorpusError::InvalidAudioPath { .. }));
}

// ---------------------------------------------------------------
// load_corpus: WAV-format strictness (through the public API, since
// the decoder itself has no public surface of its own)
// ---------------------------------------------------------------

#[test]
fn test_load_corpus_rejects_wrong_sample_rate() {
    let samples_le: Vec<u8> = [1_i16, 2, 3, 4]
        .iter()
        .flat_map(|s| s.to_le_bytes())
        .collect();
    let wav = wav_bytes_with(8_000, 16, &samples_le);
    let (_dir, manifest_path) = write_single_clip_corpus("clip-a", "en", "hello", "src", &wav);

    let err = load_corpus(&manifest_path).err().unwrap();

    match err {
        PairedCorpusError::InvalidAudioFormat { detail, .. } => {
            assert!(detail.contains("8000"), "detail: {detail}");
        }
        other => panic!("expected InvalidAudioFormat, got {other:?}"),
    }
}

#[test]
fn test_load_corpus_rejects_non_16_bit_depth() {
    let samples_le: Vec<u8> = vec![10, 20, 30, 40]; // 4 bytes of "8-bit" samples
    let wav = wav_bytes_with(16_000, 8, &samples_le);
    let (_dir, manifest_path) = write_single_clip_corpus("clip-a", "en", "hello", "src", &wav);

    let err = load_corpus(&manifest_path).err().unwrap();

    assert!(matches!(err, PairedCorpusError::InvalidAudioFormat { .. }));
}

#[test]
fn test_load_corpus_rejects_non_pcm_format_code() {
    let mut wav = pcm16_wav_bytes(&[1, 2, 3, 4]);
    // The `fmt ` audio-format field is the first u16 in its body, at
    // byte 20 (12-byte RIFF header + 8-byte "fmt "+size header).
    wav[20] = 3; // IEEE float, not PCM
    wav[21] = 0;
    let (_dir, manifest_path) = write_single_clip_corpus("clip-a", "en", "hello", "src", &wav);

    let err = load_corpus(&manifest_path).err().unwrap();

    match err {
        PairedCorpusError::InvalidAudioFormat { detail, .. } => {
            assert!(detail.contains("format code"), "detail: {detail}");
        }
        other => panic!("expected InvalidAudioFormat, got {other:?}"),
    }
}

#[test]
fn test_load_corpus_rejects_missing_riff_magic() {
    let mut wav = pcm16_wav_bytes(&[1, 2, 3, 4]);
    wav[0] = b'X';
    let (_dir, manifest_path) = write_single_clip_corpus("clip-a", "en", "hello", "src", &wav);

    let err = load_corpus(&manifest_path).err().unwrap();

    match err {
        PairedCorpusError::InvalidAudioFormat { detail, .. } => {
            assert!(detail.contains("RIFF"), "detail: {detail}");
        }
        other => panic!("expected InvalidAudioFormat, got {other:?}"),
    }
}

#[test]
fn test_load_corpus_rejects_truncated_audio_data() {
    let mut wav = pcm16_wav_bytes(&[1, 2, 3, 4, 5]);
    wav.truncate(wav.len() - 4); // chop off the last two samples' bytes
    let (_dir, manifest_path) = write_single_clip_corpus("clip-a", "en", "hello", "src", &wav);

    let err = load_corpus(&manifest_path).err().unwrap();

    assert!(matches!(err, PairedCorpusError::InvalidAudioFormat { .. }));
}

#[test]
fn test_load_corpus_rejects_missing_data_chunk() {
    let mut wav = Vec::new();
    wav.extend_from_slice(b"RIFF");
    wav.extend_from_slice(&28_u32.to_le_bytes());
    wav.extend_from_slice(b"WAVE");
    wav.extend_from_slice(b"fmt ");
    wav.extend_from_slice(&16_u32.to_le_bytes());
    wav.extend_from_slice(&1_u16.to_le_bytes());
    wav.extend_from_slice(&1_u16.to_le_bytes());
    wav.extend_from_slice(&16_000_u32.to_le_bytes());
    wav.extend_from_slice(&32_000_u32.to_le_bytes());
    wav.extend_from_slice(&2_u16.to_le_bytes());
    wav.extend_from_slice(&16_u16.to_le_bytes());
    let (_dir, manifest_path) = write_single_clip_corpus("clip-a", "en", "hello", "src", &wav);

    let err = load_corpus(&manifest_path).err().unwrap();

    match err {
        PairedCorpusError::InvalidAudioFormat { detail, .. } => {
            assert!(detail.contains("'data'"), "detail: {detail}");
        }
        other => panic!("expected InvalidAudioFormat, got {other:?}"),
    }
}

#[test]
fn test_load_corpus_rejects_audio_that_decodes_to_zero_samples() {
    let wav = pcm16_wav_bytes(&[]);
    let (_dir, manifest_path) = write_single_clip_corpus("clip-a", "en", "hello", "src", &wav);

    let err = load_corpus(&manifest_path).err().unwrap();

    match err {
        PairedCorpusError::InvalidAudioFormat { detail, .. } => {
            assert!(detail.contains("zero samples"), "detail: {detail}");
        }
        other => panic!("expected InvalidAudioFormat, got {other:?}"),
    }
}

#[test]
fn test_load_corpus_accepts_wav_with_padded_metadata_chunks() {
    // Odd-length metadata chunks require a pad byte, before or after audio.
    let mut wav = pcm16_wav_bytes(&[10, -10, 20, -20]);
    let riff_body_len_offset = 4;
    let mut junk = vec![];
    junk.extend_from_slice(b"JUNK");
    junk.extend_from_slice(&3_u32.to_le_bytes());
    junk.extend_from_slice(&[0xAA, 0xBB, 0xCC, 0x00]); // 3 bytes + 1 pad byte
    let insert_at = 12; // right after the RIFF/WAVE header
    wav.splice(insert_at..insert_at, junk.iter().copied());
    wav.extend_from_slice(&junk);
    let new_riff_body_len = u32::try_from(wav.len() - 8).unwrap();
    wav[riff_body_len_offset..riff_body_len_offset + 4]
        .copy_from_slice(&new_riff_body_len.to_le_bytes());

    let (_dir, manifest_path) = write_single_clip_corpus("clip-a", "en", "hello", "src", &wav);

    let corpus = load_corpus(&manifest_path).unwrap();

    assert_eq!(corpus.clips[0].audio.samples.len(), 4);
}

#[test]
fn test_load_corpus_rejects_unscoreable_reference() {
    let (_dir, path) = write_corpus(&[("clip-a", "en", "... !!!", "src", &SAMPLES_A)]);
    assert!(load_corpus(&path).is_err());
}

#[test]
fn test_load_corpus_rejects_inconsistent_pcm_frame_layout() {
    let mut wav = pcm16_wav_bytes(&[1, 2, 3]);
    wav[32..34].copy_from_slice(&1_u16.to_le_bytes());
    let (_dir, path) = write_single_clip_corpus("clip-a", "en", "hello", "src", &wav);
    assert!(matches!(
        load_corpus(&path),
        Err(PairedCorpusError::InvalidAudioFormat { .. })
    ));
}

#[test]
fn test_load_corpus_rejects_incomplete_riff_container() {
    let mut wav = pcm16_wav_bytes(&[1, 2, 3]);
    wav[4..8].copy_from_slice(&1000_u32.to_le_bytes());
    let (_dir, path) = write_single_clip_corpus("clip-a", "en", "hello", "src", &wav);
    assert!(matches!(
        load_corpus(&path),
        Err(PairedCorpusError::InvalidAudioFormat { .. })
    ));
}

#[test]
fn test_load_corpus_rejects_duplicate_audio_chunks() {
    let mut wav = pcm16_wav_bytes(&[1, 2, 3]);
    let data_chunk = wav[36..].to_vec();
    wav.extend_from_slice(&data_chunk);
    let size = u32::try_from(wav.len() - 8).unwrap();
    wav[4..8].copy_from_slice(&size.to_le_bytes());
    let (_dir, path) = write_single_clip_corpus("clip-a", "en", "hello", "src", &wav);
    assert!(matches!(
        load_corpus(&path),
        Err(PairedCorpusError::InvalidAudioFormat { .. })
    ));
}

#[test]
fn test_pcm_extrema_remain_within_normalized_audio_range() {
    let (_dir, path) = write_corpus(&[("clip-a", "en", "hello", "src", &[i16::MIN, 0, i16::MAX])]);
    let corpus = load_corpus(&path).unwrap();
    assert_eq!(
        corpus.clips[0].audio.samples.as_ref(),
        &[-1.0, 0.0, 32767.0 / 32768.0]
    );
}

#[test]
fn test_aggregate_rejects_empty_or_unscoreable_corpus() {
    let (_dir, mut corpus) = two_clip_corpus();
    corpus.clips.clear();
    assert!(aggregate(&corpus, vec![]).is_err());
}

#[test]
fn test_aggregate_rejects_overflowing_realtime_measurement() {
    let (_dir, corpus) = two_clip_corpus();
    let measurements = corpus
        .clips
        .iter()
        .flat_map(|clip| {
            [BACKEND_WHISPER_LOCAL, BACKEND_SHERPA_ONNX]
                .map(|backend| measurement(&clip.id, backend, &clip.reference, f64::MAX))
        })
        .collect();
    assert!(aggregate(&corpus, measurements).is_err());
}

// ---------------------------------------------------------------
// aggregate
// ---------------------------------------------------------------

fn measurement(
    clip_id: &str,
    backend: &str,
    hypothesis: &str,
    provider_lifecycle_secs: f64,
) -> ClipMeasurement {
    ClipMeasurement {
        clip_id: clip_id.to_string(),
        backend: backend.to_string(),
        hypothesis: hypothesis.to_string(),
        provider_lifecycle_secs,
    }
}

fn two_clip_corpus() -> (
    tempfile::TempDir,
    scrybe_core::testing::paired::PairedCorpus,
) {
    let (dir, manifest_path) = write_corpus(&[
        ("clip-a", "en", "hello world", "src-a", &SAMPLES_A),
        (
            "clip-b",
            "en",
            "a much longer reference sentence here today",
            "src-b",
            &SAMPLES_B,
        ),
    ]);
    let corpus = load_corpus(&manifest_path).unwrap();
    (dir, corpus)
}

#[test]
fn test_aggregate_computes_weighted_wer_rtf_and_carries_provenance() {
    let (_dir, corpus) = two_clip_corpus();
    let measurements = vec![
        measurement("clip-a", BACKEND_WHISPER_LOCAL, "hello world", 0.4),
        measurement(
            "clip-b",
            BACKEND_WHISPER_LOCAL,
            "a much longer reference sentence today",
            0.9,
        ),
        measurement("clip-a", BACKEND_SHERPA_ONNX, "hello word", 0.1),
        measurement(
            "clip-b",
            BACKEND_SHERPA_ONNX,
            "a much longer reference sentence here today",
            0.2,
        ),
    ];

    let report = aggregate(&corpus, measurements).unwrap();

    assert_eq!(report.report_version, 2);
    assert_eq!(report.clips.len(), 4);
    assert_eq!(report.backends.len(), 2);

    let clip_a_whisper = report
        .clips
        .iter()
        .find(|c| c.clip_id == "clip-a" && c.backend == BACKEND_WHISPER_LOCAL)
        .unwrap();
    assert!(clip_a_whisper.wer.abs() < f64::EPSILON);
    assert_eq!(clip_a_whisper.ref_word_count, 2);
    assert_eq!(clip_a_whisper.provenance, "src-a");
    assert_eq!(clip_a_whisper.audio_sha256.len(), 64);
    assert!((clip_a_whisper.realtime_factor - (0.4 / clip_a_whisper.audio_secs)).abs() < 1e-9);

    let deletion_result = report
        .clips
        .iter()
        .find(|c| c.clip_id == "clip-b" && c.backend == BACKEND_WHISPER_LOCAL)
        .unwrap();
    // Reference has 7 tokens; hypothesis drops "here" — one deletion,
    // independently confirmed against the shared WER helper.
    let expected_wer = word_error_rate(
        "a much longer reference sentence here today",
        "a much longer reference sentence today",
    );
    assert!((deletion_result.wer - expected_wer).abs() < 1e-9);
    assert!((deletion_result.wer - (1.0 / 7.0)).abs() < 1e-9);

    let whisper_summary = report
        .backends
        .iter()
        .find(|b| b.backend == BACKEND_WHISPER_LOCAL)
        .unwrap();
    assert_eq!(whisper_summary.clip_count, 2);
    assert_eq!(whisper_summary.total_ref_words, 9); // 2 + 7
                                                    // total edit distance = 0 (clip-a) + 1 (clip-b) = 1, over 9 ref words.
    assert!((whisper_summary.weighted_wer - (1.0 / 9.0)).abs() < 1e-9);
    assert!((whisper_summary.total_provider_lifecycle_secs - 1.3).abs() < 1e-9);

    let sherpa_summary = report
        .backends
        .iter()
        .find(|b| b.backend == BACKEND_SHERPA_ONNX)
        .unwrap();
    // clip-a: "hello word" vs "hello world" — 1 substitution / 2 ref
    // words. clip-b: exact match, 0 edits / 7 ref words.
    assert!((sherpa_summary.weighted_wer - (1.0 / 9.0)).abs() < 1e-9);
}

#[test]
fn test_aggregate_rejects_unknown_backend() {
    let (_dir, corpus) = two_clip_corpus();
    let measurements = vec![
        measurement("clip-a", "not-a-real-backend", "hello world", 0.1),
        measurement("clip-a", BACKEND_SHERPA_ONNX, "hello world", 0.1),
        measurement("clip-b", BACKEND_WHISPER_LOCAL, "x", 0.1),
        measurement("clip-b", BACKEND_SHERPA_ONNX, "x", 0.1),
    ];

    let err = aggregate(&corpus, measurements).err().unwrap();

    match err {
        AggregateError::UnknownBackend { backend } => assert_eq!(backend, "not-a-real-backend"),
        other => panic!("expected UnknownBackend, got {other:?}"),
    }
}

#[test]
fn test_aggregate_rejects_unknown_clip_id() {
    let (_dir, corpus) = two_clip_corpus();
    let measurements = vec![measurement(
        "clip-does-not-exist",
        BACKEND_WHISPER_LOCAL,
        "x",
        0.1,
    )];

    let err = aggregate(&corpus, measurements).err().unwrap();

    match err {
        AggregateError::UnknownClip { clip_id } => assert_eq!(clip_id, "clip-does-not-exist"),
        other => panic!("expected UnknownClip, got {other:?}"),
    }
}

#[test]
fn test_aggregate_rejects_duplicate_measurement() {
    let (_dir, corpus) = two_clip_corpus();
    let measurements = vec![
        measurement("clip-a", BACKEND_WHISPER_LOCAL, "hello world", 0.1),
        measurement("clip-a", BACKEND_WHISPER_LOCAL, "hello world again", 0.2),
    ];

    let err = aggregate(&corpus, measurements).err().unwrap();

    match err {
        AggregateError::DuplicateMeasurement { clip_id, backend } => {
            assert_eq!(clip_id, "clip-a");
            assert_eq!(backend, BACKEND_WHISPER_LOCAL);
        }
        other => panic!("expected DuplicateMeasurement, got {other:?}"),
    }
}

#[test]
fn test_aggregate_rejects_incomplete_backend_coverage() {
    let (_dir, corpus) = two_clip_corpus();
    // Every clip gets whisper-local but sherpa-onnx is entirely
    // absent — a "partial backend" submission.
    let measurements = vec![
        measurement("clip-a", BACKEND_WHISPER_LOCAL, "hello world", 0.1),
        measurement("clip-b", BACKEND_WHISPER_LOCAL, "x", 0.1),
    ];

    let err = aggregate(&corpus, measurements).err().unwrap();

    assert!(matches!(err, AggregateError::MissingMeasurement { .. }));
}

#[test]
fn test_aggregate_rejects_incomplete_clip_coverage() {
    let (_dir, corpus) = two_clip_corpus();
    // clip-b is missing entirely from both backends.
    let measurements = vec![
        measurement("clip-a", BACKEND_WHISPER_LOCAL, "hello world", 0.1),
        measurement("clip-a", BACKEND_SHERPA_ONNX, "hello world", 0.1),
    ];

    let err = aggregate(&corpus, measurements).err().unwrap();

    match err {
        AggregateError::MissingMeasurement { clip_id, .. } => assert_eq!(clip_id, "clip-b"),
        other => panic!("expected MissingMeasurement, got {other:?}"),
    }
}

#[test]
fn test_aggregate_rejects_non_finite_provider_lifecycle_secs() {
    let (_dir, corpus) = two_clip_corpus();
    let measurements = vec![measurement(
        "clip-a",
        BACKEND_WHISPER_LOCAL,
        "hello",
        f64::NAN,
    )];

    let err = aggregate(&corpus, measurements).unwrap_err();

    assert!(matches!(
        err,
        AggregateError::InvalidProviderLifecycleSecs { .. }
    ));
}

#[test]
fn test_aggregate_rejects_negative_provider_lifecycle_secs() {
    let (_dir, corpus) = two_clip_corpus();
    let measurements = vec![measurement("clip-a", BACKEND_WHISPER_LOCAL, "hello", -0.1)];

    let err = aggregate(&corpus, measurements).unwrap_err();

    assert!(matches!(
        err,
        AggregateError::InvalidProviderLifecycleSecs { .. }
    ));
}

#[test]
fn test_paired_report_round_trips_through_json() {
    let (_dir, corpus) = two_clip_corpus();
    let measurements = vec![
        measurement("clip-a", BACKEND_WHISPER_LOCAL, "hello world", 0.4),
        measurement(
            "clip-b",
            BACKEND_WHISPER_LOCAL,
            "a much longer reference sentence here today",
            0.9,
        ),
        measurement("clip-a", BACKEND_SHERPA_ONNX, "hello world", 0.1),
        measurement(
            "clip-b",
            BACKEND_SHERPA_ONNX,
            "a much longer reference sentence here today",
            0.2,
        ),
    ];
    let report = aggregate(&corpus, measurements).unwrap();

    let json = serde_json::to_string(&report).unwrap();
    let round_tripped: PairedReport = serde_json::from_str(&json).unwrap();

    assert_eq!(round_tripped, report);
}
