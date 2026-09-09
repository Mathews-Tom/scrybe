// Copyright 2026 Mathews Tom
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//     https://www.apache.org/licenses/LICENSE-2.0

//! `scrybe mcp` — read-only local-agent access server over stdio (M9,
//! `.docs/DEVELOPMENT_PLAN.md` §6).
//!
//! Serves `list_recent_meetings`, `search_meetings`, `get_meeting`,
//! `get_meeting_notes`, and `get_meeting_transcript` as MCP tools over
//! newline-delimited JSON-RPC 2.0 on stdin/stdout. No other transport
//! exists: there is no listener, no network socket, and no
//! authentication surface — the only way to reach this server is to
//! spawn it as a child process and speak to its stdio, which is the
//! access control. Refuses to start unless `[agent_access].enabled =
//! true` in config: this surface is opt-in and off by default.

use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::Args as ClapArgs;
use scrybe_core::agent_access::{handle_message, RealReadOnlyFs};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

use crate::runtime::{expand_root, load_or_default_config};

#[derive(ClapArgs, Debug)]
pub struct Args {
    /// Override the storage root from config.
    #[arg(long)]
    pub root: Option<PathBuf>,
}

/// Runs the stdio JSON-RPC/MCP server until stdin reaches EOF.
///
/// # Errors
///
/// Returns an error if `[agent_access].enabled` is not `true` in
/// config, or if reading from stdin or writing to stdout fails.
pub async fn run(args: Args) -> Result<()> {
    let cfg = load_or_default_config()?;
    if !cfg.agent_access.enabled {
        anyhow::bail!(
            "scrybe mcp: refusing to start — this is an opt-in, read-only surface; enable it \
             first by setting `[agent_access] enabled = true` in config.toml (see README.md's \
             Privacy and Network Posture section)"
        );
    }
    let root = args
        .root
        .as_deref()
        .map_or_else(|| expand_root(&cfg.storage.root), expand_root);

    let fs = RealReadOnlyFs;
    let mut lines = BufReader::new(tokio::io::stdin()).lines();
    let mut stdout = tokio::io::stdout();

    while let Some(line) = lines.next_line().await.context("reading stdin")? {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        if let Some(response) = handle_message(&fs, &root, trimmed) {
            stdout
                .write_all(response.as_bytes())
                .await
                .context("writing stdout")?;
            stdout.write_all(b"\n").await.context("writing stdout")?;
            stdout.flush().await.context("flushing stdout")?;
        }
    }
    Ok(())
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_run_refuses_to_start_when_agent_access_disabled() {
        let cfg_dir = tempfile::tempdir().unwrap();
        let config_path = cfg_dir.path().join("nonexistent-config.toml");
        // `load_or_default_config` falls back to `Config::default()`
        // when the discovered path does not exist, and the default
        // has `agent_access.enabled = false`.
        std::env::set_var("SCRYBE_CONFIG", &config_path);

        let err = run(Args { root: None }).await.unwrap_err();

        std::env::remove_var("SCRYBE_CONFIG");
        assert!(err.to_string().contains("refusing to start"));
    }

    // The full read-serve-write loop against real stdio is proven by
    // the `tests/mcp_stdio.rs` integration test, which spawns the
    // actual compiled binary with piped stdin/stdout — calling
    // `run()` in-process here would block on this test binary's own
    // real stdin once past the config gate.
}
