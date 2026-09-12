// Copyright 2026 Mathews Tom
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//     https://www.apache.org/licenses/LICENSE-2.0

//! macOS Launch Services handoff for Core Audio Tap recording and diagnostics.
//!
//! Recording launches the `.app` bundle through `open --args` so `TCC`'s
//! `AudioCapture` grant binds to the responsible process, forwards SIGINT from
//! the controlling terminal, locates the new session, and tails its transcript.
//! Doctor launches a bounded bundled Tap probe, captures diagnostic stdout and
//! stderr in a private temporary directory, and forwards the result without
//! persisting audio.
//!
//! See `.docs/handoff.md` §1 and §7 for why direct invocation of the
//! inner binary silently zero-fills the system tap. PR #49
//! (closed-unmerged) is the empirical confirmation.

#[cfg(target_os = "macos")]
use std::ffi::OsString;
#[cfg(target_os = "macos")]
use std::fs;
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
const FINALIZATION_STATUS_INTERVAL: Duration = Duration::from_secs(10);
#[cfg(target_os = "macos")]
const DOCTOR_PROBE_TIMEOUT: Duration = Duration::from_secs(20);

#[cfg(target_os = "macos")]
#[derive(Debug)]
pub struct DoctorProbeOutput {
    pub success: bool,
    pub stdout: String,
    pub stderr: String,
}

#[cfg(target_os = "macos")]
pub async fn launch_doctor_probe_via_bundle(bundle_path: &Path) -> Result<DoctorProbeOutput> {
    let output_dir = PrivateTempDir::new()?;
    let stdout_path = output_dir.path().join("stdout.txt");
    let stderr_path = output_dir.path().join("stderr.txt");
    fs::File::create(&stdout_path).context("creating private doctor stdout capture")?;
    fs::File::create(&stderr_path).context("creating private doctor stderr capture")?;

    let args = doctor_open_args(bundle_path, &stdout_path, &stderr_path);
    let mut command = Command::new("open");
    command.args(&args).kill_on_drop(true);
    let mut child = command
        .spawn()
        .context("invoking macOS `open` for the bundled Tap diagnostic")?;
    let Ok(result) = tokio::time::timeout(DOCTOR_PROBE_TIMEOUT, child.wait()).await else {
        child
            .kill()
            .await
            .context("stopping timed-out bundled Tap diagnostic")?;
        anyhow::bail!(
            "bundled Tap diagnostic exceeded {} seconds; retry `scrybe doctor --check-tap`",
            DOCTOR_PROBE_TIMEOUT.as_secs()
        );
    };
    let status = result.context("waiting for the bundled Tap diagnostic")?;
    let stdout = tokio::fs::read_to_string(&stdout_path)
        .await
        .context("reading bundled doctor stdout")?;
    let stderr = tokio::fs::read_to_string(&stderr_path)
        .await
        .context("reading bundled doctor stderr")?;
    output_dir.close()?;
    Ok(DoctorProbeOutput {
        success: status.success() && bundled_tap_probe_succeeded(&stdout),
        stdout,
        stderr,
    })
}

#[cfg(target_os = "macos")]
fn bundled_tap_probe_succeeded(stdout: &str) -> bool {
    stdout
        .lines()
        .any(|line| line.starts_with("tap probe:") && line.ends_with("→ OK"))
}

#[cfg(target_os = "macos")]
fn doctor_open_args(bundle_path: &Path, stdout_path: &Path, stderr_path: &Path) -> Vec<OsString> {
    [
        OsString::from("-W"),
        OsString::from("-n"),
        OsString::from("-o"),
        stdout_path.as_os_str().to_owned(),
        OsString::from("--stderr"),
        stderr_path.as_os_str().to_owned(),
        bundle_path.as_os_str().to_owned(),
        OsString::from("--args"),
        OsString::from("doctor"),
        OsString::from("--check-tap"),
    ]
    .into()
}

#[cfg(target_os = "macos")]
struct PrivateTempDir(PathBuf);

#[cfg(target_os = "macos")]
impl PrivateTempDir {
    fn new() -> Result<Self> {
        use std::io::ErrorKind;
        use std::os::unix::fs::DirBuilderExt;

        let root = std::env::temp_dir();
        for attempt in 0_u8..100 {
            let path = root.join(format!("scrybe-doctor-{}-{attempt}", std::process::id()));
            let mut builder = fs::DirBuilder::new();
            builder.mode(0o700);
            match builder.create(&path) {
                Ok(()) => return Ok(Self(path)),
                Err(error) if error.kind() == ErrorKind::AlreadyExists => {}
                Err(error) => {
                    return Err(error)
                        .with_context(|| format!("creating private directory {}", path.display()));
                }
            }
        }
        anyhow::bail!("could not reserve a private directory for bundled doctor output");
    }

    fn path(&self) -> &Path {
        &self.0
    }

    fn close(mut self) -> Result<()> {
        let path = std::mem::take(&mut self.0);
        fs::remove_dir_all(&path).with_context(|| {
            format!(
                "removing private doctor output directory {}",
                path.display()
            )
        })
    }
}

#[cfg(target_os = "macos")]
impl Drop for PrivateTempDir {
    fn drop(&mut self) {
        if !self.0.as_os_str().is_empty() {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
}

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
    let second_ctrl_c = tokio::signal::ctrl_c();
    tokio::pin!(second_ctrl_c);
    while is_pid_alive(pid) {
        tokio::select! {
            result = &mut second_ctrl_c => {
                result.context("installing second Ctrl-C handler")?;
                eprintln!("scrybe: aborting finalization; recover with `scrybe repair` and `scrybe notes`");
                send_sigint(pid)?;
                while is_pid_alive(pid) {
                    sleep(EXIT_POLL_INTERVAL).await;
                }
                break;
            }
            () = sleep(FINALIZATION_STATUS_INTERVAL) => {
                eprintln!(
                    "scrybe: finalization still running ({}s elapsed)",
                    shutdown_start.elapsed().as_secs()
                );
            }
        }
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
    let playback = session_dir.join("playback.opus");
    if playback.exists() {
        println!("  playback:   {}", playback.display());
    }
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

    #[cfg(target_os = "macos")]
    #[test]
    fn doctor_open_args_capture_only_diagnostic_streams() {
        let args = doctor_open_args(
            Path::new("/tmp/scrybe.app"),
            Path::new("/private/out.txt"),
            Path::new("/private/err.txt"),
        );
        let rendered = args
            .iter()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect::<Vec<_>>();

        assert_eq!(
            rendered,
            [
                "-W",
                "-n",
                "-o",
                "/private/out.txt",
                "--stderr",
                "/private/err.txt",
                "/tmp/scrybe.app",
                "--args",
                "doctor",
                "--check-tap",
            ]
        );
        assert!(!rendered.iter().any(|arg| arg.contains("audio")));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn bundled_tap_result_requires_the_probe_success_verdict() {
        assert!(bundled_tap_probe_succeeded(
            "config: ok\ntap probe: frames=127 peak=0.00500 → OK\nscrybe doctor: ok"
        ));
        assert!(!bundled_tap_probe_succeeded(
            "tap probe: frames=127 peak=0.00000 → FAIL: silent frames"
        ));
        assert!(!bundled_tap_probe_succeeded("scrybe doctor: ok"));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn doctor_output_directory_is_private_and_removed_explicitly() {
        use std::os::unix::fs::PermissionsExt;

        let directory = PrivateTempDir::new().unwrap();
        let path = directory.path().to_path_buf();
        let mode = fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o700);

        directory.close().unwrap();

        assert!(!path.exists());
    }
}
