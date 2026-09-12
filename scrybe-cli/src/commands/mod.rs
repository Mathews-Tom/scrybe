// Copyright 2026 Mathews Tom
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//     https://www.apache.org/licenses/LICENSE-2.0

//! Subcommand definitions and dispatcher. Each subcommand lives in its
//! own module and exposes a single `run` async function so the
//! dispatcher remains a flat match.

use anyhow::Result;
use clap::Subcommand;

pub mod bench;
mod bench_stt;
pub mod devices;
pub mod doctor;
pub mod init;
pub mod install_macos_bundle;
pub mod list;
#[cfg(feature = "agent-access")]
pub mod mcp;
pub mod notes;
pub mod qualify;
pub mod rec;
pub mod record;
pub mod repair;
pub mod show;

#[derive(Subcommand, Debug)]
pub enum Command {
    /// Bootstrap config and verify environment.
    Init(init::Args),
    /// List macOS Core Audio input devices by stable UID.
    Devices(devices::Args),
    /// Record a session with sensible defaults: `scrybe record TITLE`.
    /// Resolves capture source, Whisper model, and LLM kind from
    /// config plus platform probes. On macOS, only the Core Audio Tap
    /// backend auto-launches through the `.app` bundle for TCC.
    Record(record::Args),
    /// Record a session end-to-end with explicit flags. Used by CI,
    /// scripts, and advanced users; prefer `scrybe record TITLE` for
    /// interactive use.
    Rec(rec::Args),
    /// List recorded sessions under the configured root.
    List(list::Args),
    /// Render a session's transcript and notes.
    Show(show::Args),
    /// Regenerate `notes.md` from a session's durable transcript.
    Notes(notes::Args),
    /// Diagnostic checks: egress, disk, permissions, model checksums.
    Doctor(doctor::Args),
    /// Aggregate Criterion bench results into a versioned snapshot.
    Bench(bench::BenchArgs),
    /// Recover `audio.opus` from a crashed or `SIGKILL`ed session's
    /// journal.
    Repair(repair::Args),
    /// Create a locally self-signed macOS app bundle for Core Audio Tap TCC.
    InstallMacosBundle(install_macos_bundle::Args),
    /// Verify a completed macOS qualification session and write a redacted receipt.
    Qualify(qualify::Args),
    /// Read-only local-agent access server over stdio (M9). Refuses
    /// to start unless `[agent_access].enabled = true` in config.
    #[cfg(feature = "agent-access")]
    Mcp(mcp::Args),
}

/// Dispatch the parsed subcommand.
///
/// # Errors
///
/// Propagates the subcommand's error verbatim, with `anyhow::Context`
/// applied at the call site for user-facing prefixes.
pub async fn run(cmd: Command) -> Result<()> {
    match cmd {
        Command::Init(a) => init::run(a).await,
        Command::Record(a) => record::run(a).await,
        Command::Rec(a) => rec::run(a).await,
        Command::Devices(a) => devices::run(a),
        Command::List(a) => list::run(a).await,
        Command::Show(a) => show::run(a).await,
        Command::Notes(a) => notes::run(a).await,
        Command::Doctor(a) => doctor::run(a).await,
        Command::Bench(a) => bench::run(a).await,
        Command::Repair(a) => repair::run(a).await,
        Command::Qualify(a) => qualify::run(&a),
        Command::InstallMacosBundle(a) => install_macos_bundle::run(&a),
        #[cfg(feature = "agent-access")]
        Command::Mcp(a) => mcp::run(a).await,
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_run_dispatches_list_command_to_list_run() {
        let dir = tempfile::tempdir().unwrap();

        run(Command::List(list::Args {
            root: Some(dir.path().to_path_buf()),
        }))
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn test_run_dispatches_show_command_and_propagates_resolve_error() {
        let dir = tempfile::tempdir().unwrap();

        let err = run(Command::Show(show::Args {
            id_or_folder: "missing".into(),
            root: Some(dir.path().to_path_buf()),
            no_transcript: false,
        }))
        .await
        .unwrap_err();

        assert!(err.to_string().contains("missing"));
    }

    #[tokio::test]
    async fn test_run_dispatches_doctor_command() {
        let dir = tempfile::tempdir().unwrap();

        run(Command::Doctor(doctor::Args {
            root: Some(dir.path().to_path_buf()),
            check_tap: false,
            check_sck: false,
            fix: false,
            sign_self: None,
        }))
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn test_run_dispatches_repair_command_and_propagates_resolve_error() {
        let dir = tempfile::tempdir().unwrap();

        let err = run(Command::Repair(repair::Args {
            id_or_folder: "missing".into(),
            root: Some(dir.path().to_path_buf()),
        }))
        .await
        .unwrap_err();

        assert!(err.to_string().contains("missing"));
    }
}
