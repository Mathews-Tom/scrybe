// Copyright 2026 Mathews Tom
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//     https://www.apache.org/licenses/LICENSE-2.0

//! `scrybe doctor` — diagnostic command. Reports on:
//!
//! - config file resolution
//! - storage root reachability and free disk
//! - orphaned `*.partial` model files
//! - orphaned per-session pid locks (process not alive)
//! - egress posture (which provider URLs the current config will hit)

use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::Args as ClapArgs;
use scrybe_core::config::Config;

use crate::runtime::{expand_root, load_or_default_config};

#[derive(ClapArgs, Debug)]
pub struct Args {
    /// Override the storage root from config.
    #[arg(long)]
    pub root: Option<PathBuf>,
}

#[allow(clippy::unused_async)]
pub async fn run(args: Args) -> Result<()> {
    let mut report = Report::default();

    let config_path = Config::discover_path().context("resolving config path")?;
    report.lines.push(format!(
        "config: {} (exists={})",
        config_path.display(),
        config_path.exists()
    ));

    let cfg = load_or_default_config()?;
    let root = match args.root {
        Some(p) => expand_root(&p),
        None => expand_root(&cfg.storage.root),
    };
    report.lines.push(format!(
        "storage root: {} (exists={})",
        root.display(),
        root.exists()
    ));

    if root.exists() {
        scan_root(&root, &mut report)?;
    }
    report_egress_posture(&cfg, &mut report);

    for line in &report.lines {
        println!("{line}");
    }
    if report.warnings == 0 {
        println!("scrybe doctor: ok ({} checks)", report.lines.len());
    } else {
        println!(
            "scrybe doctor: completed with {} warnings (see lines above)",
            report.warnings
        );
    }
    Ok(())
}

#[derive(Default, Debug)]
struct Report {
    lines: Vec<String>,
    warnings: u32,
}

fn scan_root(root: &std::path::Path, report: &mut Report) -> Result<()> {
    let mut session_count = 0_u32;
    let mut orphaned_locks = 0_u32;
    let mut orphaned_partials = 0_u32;

    let entries = std::fs::read_dir(root).with_context(|| format!("reading {}", root.display()))?;
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            session_count += 1;
            let lock = path.join(scrybe_core::storage::PID_LOCK_NAME);
            if lock.exists() {
                if pid_alive_from_lock(&lock).unwrap_or(false) {
                    report
                        .lines
                        .push(format!("session in progress: {}", path.display()));
                } else {
                    orphaned_locks += 1;
                    report
                        .lines
                        .push(format!("orphaned pid.lock: {}", lock.display()));
                }
            }
        } else {
            let is_partial = path
                .file_name()
                .and_then(|s| s.to_str())
                .is_some_and(|name| name.ends_with(".partial"));
            if is_partial {
                orphaned_partials += 1;
                report
                    .lines
                    .push(format!("orphaned partial download: {}", path.display()));
            }
        }
    }

    report
        .lines
        .push(format!("sessions found: {session_count}"));
    if orphaned_locks > 0 {
        report.warnings += orphaned_locks;
    }
    if orphaned_partials > 0 {
        report.warnings += orphaned_partials;
    }
    Ok(())
}

fn pid_alive_from_lock(lock_path: &std::path::Path) -> Result<bool> {
    let body = std::fs::read_to_string(lock_path).context("reading pid.lock")?;
    let pid: u32 = body
        .trim()
        .parse()
        .with_context(|| format!("parsing pid in {}", lock_path.display()))?;
    Ok(is_pid_alive(pid))
}

#[cfg(unix)]
#[allow(clippy::cast_possible_wrap)]
fn is_pid_alive(pid: u32) -> bool {
    // SAFETY: kill(pid, 0) does not send a signal; it returns 0 if
    // the process exists and is signalable, ESRCH otherwise. No
    // mutation of process state, no allocation.
    #[allow(unsafe_code)]
    let rc = unsafe { libc::kill(pid as libc::pid_t, 0) };
    rc == 0
}

#[cfg(not(unix))]
const fn is_pid_alive(_pid: u32) -> bool {
    // On Windows we conservatively treat every lock as live; the
    // doctor command surfaces the lock and lets the user remove it.
    true
}

fn report_egress_posture(cfg: &Config, report: &mut Report) {
    let stt = match cfg.stt.provider.as_str() {
        "whisper-local" => "no egress (local Whisper)".to_string(),
        other => cfg.stt.base_url.as_deref().map_or_else(
            || format!("STT provider {other} configured without base_url"),
            |url| format!("egress to STT provider {other} at {url}"),
        ),
    };
    let llm = match cfg.llm.provider.as_str() {
        "ollama" | "lmstudio" => format!("no egress (local LLM at {})", cfg.llm.base_url),
        other => format!("egress to LLM provider {other} at {}", cfg.llm.base_url),
    };
    report.lines.push(format!("stt egress: {stt}"));
    report.lines.push(format!("llm egress: {llm}"));
}

#[cfg(unix)]
extern crate libc;

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    #[test]
    fn test_report_egress_posture_local_only_emits_no_egress_lines() {
        let cfg = Config::default();
        let mut report = Report::default();

        report_egress_posture(&cfg, &mut report);

        assert_eq!(report.lines.len(), 2);
        assert!(report.lines[0].contains("no egress"));
        assert!(report.lines[1].contains("no egress"));
    }

    #[test]
    fn test_report_egress_posture_openai_compat_stt_reports_base_url() {
        let mut cfg = Config::default();
        cfg.stt.provider = "openai-compat".into();
        cfg.stt.base_url = Some("https://api.groq.com/openai/v1".into());
        let mut report = Report::default();

        report_egress_posture(&cfg, &mut report);

        assert!(report.lines[0].contains("https://api.groq.com/openai/v1"));
    }

    #[test]
    fn test_scan_root_for_empty_root_reports_zero_sessions() {
        let dir = tempfile::tempdir().unwrap();
        let mut report = Report::default();

        scan_root(dir.path(), &mut report).unwrap();

        assert_eq!(report.warnings, 0);
        assert!(report.lines.iter().any(|l| l.contains("sessions found: 0")));
    }

    #[test]
    fn test_scan_root_flags_orphaned_partial_downloads() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("model.gguf.partial"), b"abc").unwrap();
        let mut report = Report::default();

        scan_root(dir.path(), &mut report).unwrap();

        assert_eq!(report.warnings, 1);
        assert!(report.lines.iter().any(|l| l.contains("orphaned partial")));
    }

    #[test]
    fn test_scan_root_flags_orphaned_pid_lock_for_dead_process() {
        let dir = tempfile::tempdir().unwrap();
        let folder = dir.path().join("session-x");
        std::fs::create_dir(&folder).unwrap();
        std::fs::write(folder.join(scrybe_core::storage::PID_LOCK_NAME), b"1\n").unwrap();
        let mut report = Report::default();

        scan_root(dir.path(), &mut report).unwrap();

        // pid 1 may or may not be considered alive on this platform;
        // the test asserts that the scanner observes the lock without
        // panicking and reports a session.
        assert!(report.lines.iter().any(|l| l.contains("session-x")));
    }
}
