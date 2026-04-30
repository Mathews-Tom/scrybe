// Copyright 2026 Mathews Tom
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//     https://www.apache.org/licenses/LICENSE-2.0

//! `scrybe` CLI — record, list, show, doctor, init.

use anyhow::{Context, Result};
use clap::Parser;
use tokio::runtime::{Builder, Runtime};

mod commands;
mod prompter;
mod runtime;

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

    #[cfg(feature = "cli-shell")]
    {
        if let commands::Command::Record(args) = &cli.command {
            if args.shell {
                return shell::run_record_with_shell(args.clone(), &runtime);
            }
        }
    }

    #[cfg(not(feature = "cli-shell"))]
    {
        if let commands::Command::Record(args) = &cli.command {
            if args.shell {
                tracing::info!(
                    "scrybe record --shell: this binary was built without the cli-shell \
                     feature; running headless and stopping on SIGINT only."
                );
            }
        }
    }

    runtime.block_on(commands::run(cli.command))
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
        .init();
}
