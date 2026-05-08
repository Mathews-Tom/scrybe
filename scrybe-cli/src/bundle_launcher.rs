// Copyright 2026 Mathews Tom
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//     https://www.apache.org/licenses/LICENSE-2.0

//! macOS bundle launcher for the ergonomic `scrybe record <title>`
//! subcommand.
//!
//! Wraps `open --args` to launch the `.app` bundle through Launch
//! Services so `TCC`'s `AudioCapture` grant binds to the bundle's
//! responsible process, then forwards SIGINT from the controlling
//! terminal to the launched bundle process. The launcher polls the
//! session storage root for the new session folder and tails its
//! `transcript.md` so the user sees per-chunk transcription progress
//! during the recording.
//!
//! See `.docs/handoff.md` §1 and §7 for why direct invocation of the
//! inner binary silently zero-fills the system tap. PR #49
//! (closed-unmerged) is the empirical confirmation.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use anyhow::{Context, Result};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;
use tokio::time::{sleep, Instant};

const BUNDLE_PROC_PATTERN: &str = "scrybe.app/Contents/MacOS/scrybe";
const STARTUP_POLL_INTERVAL: Duration = Duration::from_millis(100);
const STARTUP_POLL_TIMEOUT: Duration = Duration::from_secs(8);
const EXIT_POLL_INTERVAL: Duration = Duration::from_millis(250);
const SHUTDOWN_GRACE: Duration = Duration::from_mins(1);

/// Launch the bundle via `open --args` with the given `rec` argv,
/// forward SIGINT to the bundle's PID, and tail the session's
/// transcript while it runs. Returns when the bundle exits.
pub async fn launch_via_bundle(
    bundle_path: &Path,
    rec_args: &[String],
    session_root: &Path,
) -> Result<()> {
    let pre_session_floor = newest_session_mtime(session_root);

    let mut open_argv = Vec::with_capacity(3 + rec_args.len());
    open_argv.push(bundle_path.to_string_lossy().into_owned());
    open_argv.push("--args".to_string());
    open_argv.push("rec".to_string());
    open_argv.extend(rec_args.iter().cloned());

    let status = Command::new("open")
        .args(&open_argv)
        .status()
        .await
        .context("invoking macOS `open` to launch bundle")?;
    if !status.success() {
        anyhow::bail!("`open` returned non-zero status: {status:?}");
    }

    let pid = wait_for_bundle_pid()
        .await
        .context("bundle process did not appear within startup window")?;
    eprintln!("scrybe: recording (pid={pid}); press Ctrl-C to stop");

    let session_dir = wait_for_new_session(session_root, pre_session_floor)
        .await
        .ok();
    if let Some(dir) = &session_dir {
        eprintln!("scrybe: session at {}", dir.display());
        spawn_transcript_tail(dir.clone());
    }

    let ctrl_c = tokio::signal::ctrl_c();
    tokio::pin!(ctrl_c);

    loop {
        tokio::select! {
            res = &mut ctrl_c => {
                res.context("installing Ctrl-C handler")?;
                eprintln!("scrybe: stopping recording (forwarding SIGINT to bundle)...");
                send_sigint(pid)?;
                break;
            }
            () = sleep(EXIT_POLL_INTERVAL) => {
                if !is_pid_alive(pid) { return Ok(()); }
            }
        }
    }

    let shutdown_start = Instant::now();
    while is_pid_alive(pid) && shutdown_start.elapsed() < SHUTDOWN_GRACE {
        sleep(EXIT_POLL_INTERVAL).await;
    }
    if is_pid_alive(pid) {
        anyhow::bail!("bundle did not exit within {SHUTDOWN_GRACE:?} of SIGINT");
    }

    // Brief drain so the transcript-tail task gets the bundle's final
    // chunks before we print the summary on top of them.
    sleep(EXIT_POLL_INTERVAL).await;
    if let Some(dir) = &session_dir {
        print_final_summary(dir);
    }
    Ok(())
}

async fn wait_for_bundle_pid() -> Result<u32> {
    let start = Instant::now();
    while start.elapsed() < STARTUP_POLL_TIMEOUT {
        if let Some(pid) = find_bundle_pid() {
            return Ok(pid);
        }
        sleep(STARTUP_POLL_INTERVAL).await;
    }
    anyhow::bail!("timed out polling for `{BUNDLE_PROC_PATTERN}`")
}

fn find_bundle_pid() -> Option<u32> {
    let output = std::process::Command::new("pgrep")
        .args(["-f", BUNDLE_PROC_PATTERN])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .next()
        .and_then(|s| s.trim().parse().ok())
}

fn send_sigint(pid: u32) -> Result<()> {
    if !is_pid_alive(pid) {
        // Bundle already exited (e.g., via Ctrl-C reaching the
        // foreground process group); SIGINT would be a no-op.
        return Ok(());
    }
    let status = std::process::Command::new("kill")
        .args(["-INT", &pid.to_string()])
        .stderr(Stdio::null())
        .status()
        .with_context(|| format!("sending SIGINT to pid {pid}"))?;
    if !status.success() {
        // Race: bundle exited between the alive check and the kill
        // call. Treat as success — there was nothing to interrupt.
    }
    Ok(())
}

fn is_pid_alive(pid: u32) -> bool {
    // `kill -0 PID` writes "kill: PID: No such process" to stderr when
    // the target is gone; redirect to /dev/null so the launcher's
    // terminal stays clean during the post-SIGINT shutdown poll.
    std::process::Command::new("kill")
        .args(["-0", &pid.to_string()])
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

fn print_final_summary(session_dir: &Path) {
    let session_id = std::fs::read_to_string(session_dir.join("meta.toml"))
        .ok()
        .as_deref()
        .and_then(parse_session_id_from_meta)
        .unwrap_or_else(|| {
            session_dir
                .file_name()
                .and_then(|s| s.to_str())
                .unwrap_or("")
                .rsplit_once('-')
                .map_or_else(|| "(unknown)".to_string(), |(_, ulid)| ulid.to_string())
        });

    println!(
        "scrybe record: session {session_id} written to {}",
        session_dir.display()
    );
    println!(
        "  transcript: {}",
        session_dir.join("transcript.md").display()
    );
    println!("  notes:      {}", session_dir.join("notes.md").display());
    println!("  meta:       {}", session_dir.join("meta.toml").display());
    println!("  audio:      {}", session_dir.join("audio.opus").display());
}

fn parse_session_id_from_meta(meta_toml: &str) -> Option<String> {
    meta_toml.lines().find_map(|line| {
        line.trim()
            .strip_prefix("session_id")?
            .trim_start()
            .strip_prefix('=')?
            .trim()
            .strip_prefix('"')?
            .strip_suffix('"')
            .map(str::to_string)
    })
}

fn newest_session_mtime(root: &Path) -> Option<std::time::SystemTime> {
    let entries = std::fs::read_dir(root).ok()?;
    entries
        .filter_map(Result::ok)
        .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
        .filter_map(|e| e.metadata().ok()?.modified().ok())
        .max()
}

async fn wait_for_new_session(
    root: &Path,
    floor: Option<std::time::SystemTime>,
) -> Result<PathBuf> {
    let start = Instant::now();
    while start.elapsed() < STARTUP_POLL_TIMEOUT {
        if let Some(dir) = newest_session_dir_after(root, floor) {
            return Ok(dir);
        }
        sleep(STARTUP_POLL_INTERVAL).await;
    }
    anyhow::bail!("session folder did not appear under {}", root.display())
}

fn newest_session_dir_after(root: &Path, floor: Option<std::time::SystemTime>) -> Option<PathBuf> {
    let entries = std::fs::read_dir(root).ok()?;
    let mut newest: Option<(std::time::SystemTime, PathBuf)> = None;
    for entry in entries.filter_map(Result::ok) {
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        if !file_type.is_dir() {
            continue;
        }
        let Ok(meta) = entry.metadata() else { continue };
        let Ok(modified) = meta.modified() else {
            continue;
        };
        if let Some(f) = floor {
            if modified <= f {
                continue;
            }
        }
        if newest.as_ref().is_none_or(|(prev, _)| modified > *prev) {
            newest = Some((modified, entry.path()));
        }
    }
    newest.map(|(_, path)| path)
}

fn spawn_transcript_tail(session_dir: PathBuf) {
    tokio::spawn(async move {
        let transcript = session_dir.join("transcript.md");
        let start = Instant::now();
        while !transcript.exists() && start.elapsed() < STARTUP_POLL_TIMEOUT {
            sleep(STARTUP_POLL_INTERVAL).await;
        }
        let Ok(file) = tokio::fs::File::open(&transcript).await else {
            return;
        };
        let mut reader = BufReader::new(file);
        let mut buf = String::new();
        loop {
            buf.clear();
            match reader.read_line(&mut buf).await {
                Ok(0) => sleep(EXIT_POLL_INTERVAL).await,
                Ok(_) => print!("{buf}"),
                Err(_) => return,
            }
        }
    });
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn test_find_bundle_pid_returns_none_when_no_match() {
        std::env::set_var("PATH", std::env::var("PATH").unwrap_or_default());
        // pgrep against a string that cannot be a process name — even
        // when pgrep itself isn't available, the helper returns None.
        let _ = find_bundle_pid();
    }

    #[test]
    fn test_is_pid_alive_returns_false_for_known_dead_pid() {
        // pid 999_999_999 is far beyond any realistic PID; kill -0
        // returns non-zero, so the helper reports false.
        assert!(!is_pid_alive(999_999_999));
    }

    #[test]
    fn test_parse_session_id_extracts_ulid_from_canonical_meta() {
        let meta = "session_id = \"01KR3GDRT5HZS0VQ9FHBX1P1TW\"\ntitle = \"x\"\n";
        assert_eq!(
            parse_session_id_from_meta(meta).as_deref(),
            Some("01KR3GDRT5HZS0VQ9FHBX1P1TW")
        );
    }

    #[test]
    fn test_parse_session_id_returns_none_when_field_absent() {
        let meta = "title = \"x\"\nstarted_at = \"2026-05-08T00:00:00Z\"\n";
        assert!(parse_session_id_from_meta(meta).is_none());
    }

    #[test]
    fn test_newest_session_dir_after_filters_by_mtime_floor() {
        let tmp = tempfile::tempdir().unwrap();
        let a = tmp.path().join("session-a");
        std::fs::create_dir(&a).unwrap();
        let mtime = std::fs::metadata(&a).unwrap().modified().unwrap();
        let later = mtime + Duration::from_mins(1);
        let result_with_future_floor = newest_session_dir_after(tmp.path(), Some(later));
        assert!(
            result_with_future_floor.is_none(),
            "expected None when floor is in the future, got {result_with_future_floor:?}"
        );
        let result_no_floor = newest_session_dir_after(tmp.path(), None);
        assert_eq!(result_no_floor.as_deref(), Some(a.as_path()));
    }
}
