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
    }
}
