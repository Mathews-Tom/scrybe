// Copyright 2026 Mathews Tom
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//     https://www.apache.org/licenses/LICENSE-2.0

//! Redacted verification of a completed macOS qualification session.
//!
//! This command never records audio. It reads only the structural session
//! artifacts required by the macOS release-qualification contract and writes a
//! receipt that deliberately excludes audio, transcript, notes, titles, paths,
//! prompts, and credentials.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use clap::{Args as ClapArgs, ValueEnum};
use serde::{Deserialize, Serialize};

const RECEIPT_SCHEMA_VERSION: u8 = 1;

#[derive(ClapArgs, Debug)]
pub struct Args {
    /// Which ordered qualification stage this completed session represents.
    #[arg(long, value_enum)]
    stage: Stage,

    /// Finished Scrybe session directory to verify.
    #[arg(long)]
    session: PathBuf,

    /// Destination for the redacted qualification receipt.
    #[arg(long)]
    receipt: PathBuf,

    /// Exact provider name expected in `meta.toml` for speech-to-text.
    #[arg(long)]
    expected_stt: String,

    /// Exact provider name expected in `meta.toml` for note generation.
    #[arg(long)]
    expected_llm: String,

    /// Passing mic-only receipt required before the mic-plus-system stage.
    #[arg(long, requires = "stage")]
    stage_a_receipt: Option<PathBuf>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize, ValueEnum)]
#[serde(rename_all = "kebab-case")]
pub enum Stage {
    MicOnly,
    MicSystem,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
enum Verdict {
    Pass,
    Fail,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
enum Failure {
    StageAReceiptMissing,
    StageAReceiptInvalid,
    StageAFailed,
    MissingArtifact,
    EmptyArtifact,
    InvalidMetadata,
    InvalidAudioLayout,
    ProviderMismatch,
}

impl Failure {
    const fn message(&self) -> &'static str {
        match self {
            Self::StageAReceiptMissing => {
                "a passing mic-only receipt is required before mic+system"
            }
            Self::StageAReceiptInvalid => {
                "the supplied mic-only receipt is unreadable or mismatched"
            }
            Self::StageAFailed => "the supplied mic-only receipt did not pass",
            Self::MissingArtifact => "the session is missing a required artifact",
            Self::EmptyArtifact => "a required session artifact is empty",
            Self::InvalidMetadata => "meta.toml does not meet the qualification contract",
            Self::InvalidAudioLayout => "the session audio layout does not match its stage",
            Self::ProviderMismatch => {
                "the session providers do not match the expected local providers"
            }
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
struct Receipt {
    schema_version: u8,
    stage: Stage,
    command: String,
    permission: String,
    session: Option<SessionEvidence>,
    verdict: Verdict,
    failure: Option<Failure>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
struct SessionEvidence {
    session_id: String,
    duration_secs: u64,
    stt_provider: String,
    llm_provider: String,
    channels: u64,
    layout: String,
    audio_present: bool,
    transcript_present: bool,
    notes_present: bool,
}

#[derive(Deserialize)]
struct Meta {
    session_id: String,
    duration_secs: u64,
    providers: Providers,
    audio: Audio,
}

#[derive(Deserialize)]
struct Providers {
    stt: String,
    llm: String,
}

#[derive(Deserialize)]
struct Audio {
    channels: u64,
    layout: String,
}

pub fn run(args: &Args) -> Result<()> {
    let receipt = inspect(args);
    write_receipt(&args.receipt, &receipt)?;
    println!("qualification receipt: {}", receipt_status(&receipt));
    if let Some(failure) = receipt.failure {
        anyhow::bail!("macOS qualification failed: {}", failure.message());
    }
    Ok(())
}

fn inspect(args: &Args) -> Receipt {
    let failure = stage_dependency_failure(args).or_else(|| session_failure(args));
    let session = inspect_session(&args.session).ok();
    Receipt {
        schema_version: RECEIPT_SCHEMA_VERSION,
        stage: args.stage,
        command: match args.stage {
            Stage::MicOnly => "scrybe record --source mic",
            Stage::MicSystem => "scrybe record --source mic+system",
        }
        .to_string(),
        permission: "not-probed".to_string(),
        session,
        verdict: if failure.is_some() {
            Verdict::Fail
        } else {
            Verdict::Pass
        },
        failure,
    }
}

fn stage_dependency_failure(args: &Args) -> Option<Failure> {
    if args.stage != Stage::MicSystem {
        return None;
    }
    let path = args.stage_a_receipt.as_deref()?;
    let parsed = fs::read_to_string(path)
        .ok()
        .and_then(|body| serde_json::from_str::<Receipt>(&body).ok());
    match parsed {
        Some(receipt) if receipt.stage == Stage::MicOnly && receipt.verdict == Verdict::Pass => {
            None
        }
        Some(receipt) if receipt.stage == Stage::MicOnly => Some(Failure::StageAFailed),
        Some(_) | None => Some(Failure::StageAReceiptInvalid),
    }
}

fn session_failure(args: &Args) -> Option<Failure> {
    if args.stage == Stage::MicSystem && args.stage_a_receipt.is_none() {
        return Some(Failure::StageAReceiptMissing);
    }
    let evidence = match inspect_session(&args.session) {
        Ok(evidence) => evidence,
        Err(failure) => return Some(failure),
    };
    if evidence.stt_provider != args.expected_stt || evidence.llm_provider != args.expected_llm {
        return Some(Failure::ProviderMismatch);
    }
    let expected = match args.stage {
        Stage::MicOnly => (1, "mono:mic"),
        Stage::MicSystem => (2, "stereo:mic-l,system-r"),
    };
    if evidence.channels != expected.0 || evidence.layout != expected.1 {
        return Some(Failure::InvalidAudioLayout);
    }
    None
}

fn inspect_session(dir: &Path) -> std::result::Result<SessionEvidence, Failure> {
    for name in ["audio.opus", "transcript.md", "notes.md"] {
        let path = dir.join(name);
        let metadata = fs::metadata(path).map_err(|_| Failure::MissingArtifact)?;
        if metadata.len() == 0 {
            return Err(Failure::EmptyArtifact);
        }
    }
    let body = fs::read_to_string(dir.join("meta.toml")).map_err(|_| Failure::MissingArtifact)?;
    let meta = toml::from_str::<Meta>(&body).map_err(|_| Failure::InvalidMetadata)?;
    if meta.duration_secs == 0 || meta.audio.channels == 0 || meta.audio.layout.is_empty() {
        return Err(Failure::InvalidMetadata);
    }
    Ok(SessionEvidence {
        session_id: meta.session_id,
        duration_secs: meta.duration_secs,
        stt_provider: meta.providers.stt,
        llm_provider: meta.providers.llm,
        channels: meta.audio.channels,
        layout: meta.audio.layout,
        audio_present: true,
        transcript_present: true,
        notes_present: true,
    })
}

fn write_receipt(path: &Path, receipt: &Receipt) -> Result<()> {
    let body = serde_json::to_vec_pretty(receipt).context("serializing qualification receipt")?;
    fs::write(path, body).with_context(|| format!("writing receipt to {}", path.display()))
}

const fn receipt_status(receipt: &Receipt) -> &'static str {
    match receipt.verdict {
        Verdict::Pass => "PASS",
        Verdict::Fail => "FAIL",
    }
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]
mod tests {
    use super::*;

    fn args(session: PathBuf, stage: Stage) -> Args {
        Args {
            stage,
            session,
            receipt: PathBuf::from("unused.json"),
            expected_stt: "whisper-local".into(),
            expected_llm: "openai-compat:qwen2.5:7b-instruct".into(),
            stage_a_receipt: None,
        }
    }

    fn write_session(dir: &Path, channels: u64, layout: &str) {
        fs::write(dir.join("audio.opus"), b"opus").unwrap();
        fs::write(dir.join("transcript.md"), b"private transcript").unwrap();
        fs::write(dir.join("notes.md"), b"private notes").unwrap();
        fs::write(
            dir.join("meta.toml"),
            format!(
                "session_id = \"01QUALIFY\"\nduration_secs = 15\n\n[providers]\nstt = \"whisper-local\"\nllm = \"openai-compat:qwen2.5:7b-instruct\"\n\n[audio]\nchannels = {channels}\nlayout = \"{layout}\"\n"
            ),
        )
        .unwrap();
    }

    #[test]
    fn mic_only_receipt_passes_without_content() {
        let dir = tempfile::tempdir().unwrap();
        write_session(dir.path(), 1, "mono:mic");

        let receipt = inspect(&args(dir.path().to_path_buf(), Stage::MicOnly));
        let rendered = serde_json::to_string(&receipt).unwrap();

        assert_eq!(receipt.verdict, Verdict::Pass);
        assert!(!rendered.contains("private transcript"));
        assert!(!rendered.contains("private notes"));
    }

    #[test]
    fn system_stage_rejects_mono_session() {
        let dir = tempfile::tempdir().unwrap();
        write_session(dir.path(), 1, "mono:mic");
        let mut args = args(dir.path().to_path_buf(), Stage::MicSystem);
        let stage_a = dir.path().join("stage-a.json");
        write_receipt(
            &stage_a,
            &Receipt {
                schema_version: RECEIPT_SCHEMA_VERSION,
                stage: Stage::MicOnly,
                command: "scrybe record --source mic".to_string(),
                permission: "not-probed".to_string(),
                session: None,
                verdict: Verdict::Pass,
                failure: None,
            },
        )
        .unwrap();
        args.stage_a_receipt = Some(stage_a);

        let receipt = inspect(&args);

        assert_eq!(receipt.failure, Some(Failure::InvalidAudioLayout));
    }

    #[test]
    fn missing_artifact_produces_a_failing_receipt() {
        let dir = tempfile::tempdir().unwrap();

        let receipt = inspect(&args(dir.path().to_path_buf(), Stage::MicOnly));

        assert_eq!(receipt.verdict, Verdict::Fail);
        assert_eq!(receipt.failure, Some(Failure::MissingArtifact));
    }

    #[test]
    fn system_stage_requires_passing_mic_receipt() {
        let dir = tempfile::tempdir().unwrap();
        write_session(dir.path(), 2, "stereo:mic-l,system-r");

        let receipt = inspect(&args(dir.path().to_path_buf(), Stage::MicSystem));

        assert_eq!(receipt.failure, Some(Failure::StageAReceiptMissing));
    }
}
