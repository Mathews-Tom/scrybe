// Copyright 2026 Mathews Tom
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//     https://www.apache.org/licenses/LICENSE-2.0

//! `scrybe` CLI — record, list, show, doctor, init.

use anyhow::{Context, Result};
use clap::Parser;
use tokio::runtime::{Builder, Runtime};

#[cfg(feature = "system-capture-mac")]
mod bundle_launcher;
mod capture_control;
mod commands;
#[cfg(target_os = "macos")]
mod macos_bundle;
mod prompter;
mod runtime;

#[cfg(all(feature = "cli-shell", target_os = "macos"))]
mod floating_panel;
#[cfg(feature = "cli-shell")]
mod hotkey;
#[cfg(feature = "cli-shell")]
mod shell;
#[cfg(feature = "cli-shell")]
mod tray;

#[derive(Parser, Debug)]
#[command(
    name = "scrybe",
    version,
    about = "Open-source local-first meeting transcription and notes",
    long_about = None,
)]
struct Cli {
    #[command(subcommand)]
    command: commands::Command,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    init_tracing();

    let runtime = build_runtime()?;

    match cli.command {
        commands::Command::Rec(args) if args.shell => run_rec_shell(args, &runtime),
        commands::Command::Record(args) if args.shell => run_record_shell(&args, &runtime),
        command => runtime.block_on(commands::run(command)),
    }
}

#[cfg(feature = "cli-shell")]
fn run_rec_shell(args: commands::rec::Args, runtime: &Runtime) -> Result<()> {
    shell::run_record_with_shell(args, runtime)
}

#[cfg(not(feature = "cli-shell"))]
fn run_rec_shell(_args: commands::rec::Args, _runtime: &Runtime) -> Result<()> {
    anyhow::bail!(
        "`scrybe rec --shell` requires the `cli-shell` build feature; install the standard \
         binary or rebuild with `--features cli-shell`"
    )
}

#[cfg(feature = "cli-shell")]
fn run_record_shell(args: &commands::record::Args, runtime: &Runtime) -> Result<()> {
    commands::record::run_with_shell(args, runtime)
}

#[cfg(not(feature = "cli-shell"))]
fn run_record_shell(_args: &commands::record::Args, _runtime: &Runtime) -> Result<()> {
    anyhow::bail!(
        "`scrybe record --shell` requires the `cli-shell` build feature; install the standard \
         binary or rebuild with `--features cli-shell`"
    )
}

fn build_runtime() -> Result<Runtime> {
    Builder::new_multi_thread()
        .enable_all()
        .build()
        .context("building tokio runtime")
}

fn init_tracing() {
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("warn"));
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(false)
        .compact()
        .with_writer(std::io::stderr)
        .init();
}

#[cfg(test)]
#[allow(clippy::panic)]
mod tests {
    use super::*;

    #[test]
    fn both_recording_commands_parse_the_shell_flag() {
        let rec = Cli::try_parse_from(["scrybe", "rec", "--shell"]);
        assert!(matches!(
            rec,
            Ok(Cli {
                command: commands::Command::Rec(commands::rec::Args { shell: true, .. })
            })
        ));

        let record = Cli::try_parse_from(["scrybe", "record", "standup", "--shell"]);
        assert!(matches!(
            record,
            Ok(Cli {
                command: commands::Command::Record(commands::record::Args { shell: true, .. })
            })
        ));
    }
}
