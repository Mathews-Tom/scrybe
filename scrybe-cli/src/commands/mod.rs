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
pub mod doctor;
pub mod init;
pub mod list;
pub mod record;
pub mod show;

#[derive(Subcommand, Debug)]
pub enum Command {
    /// Bootstrap config and verify environment.
    Init(init::Args),
    /// Record a session end-to-end.
    Record(record::Args),
    /// List recorded sessions under the configured root.
    List(list::Args),
    /// Render a session's transcript and notes.
    Show(show::Args),
    /// Diagnostic checks: egress, disk, permissions, model checksums.
    Doctor(doctor::Args),
    /// Aggregate Criterion bench results into a versioned snapshot.
    Bench(bench::BenchArgs),
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
        Command::List(a) => list::run(a).await,
        Command::Show(a) => show::run(a).await,
        Command::Doctor(a) => doctor::run(a).await,
        Command::Bench(a) => bench::run(a).await,
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
        }))
        .await
        .unwrap();
    }
}
