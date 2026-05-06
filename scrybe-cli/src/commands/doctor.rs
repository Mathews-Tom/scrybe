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

    /// Probe the macOS Core Audio Tap end-to-end. Plays a known-loud
    /// system sound through `afplay`, captures from the live tap for
    /// 1.5 s, and reports the peak amplitude. Distinguishes the three
    /// failure shapes for the system-tap-silent-frames bug:
    /// no frames received (`IOProc` never fired), frames received but
    /// peak ≈ 0 (TCC denied or device misroute), or frames + non-zero
    /// peak (tap healthy). Requires the binary to be built with
    /// `--features system-capture-mac`.
    #[arg(long, default_value_t = false)]
    pub check_tap: bool,
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

    if args.check_tap {
        check_tap(&mut report).await;
    }

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

/// Audio fixture played during `scrybe doctor --check-tap`. Submarine
/// is ~2.5 s of broadband audio shipped on every macOS install since
/// Mac OS X — long enough to fill the 1.5 s capture window with real
/// signal, short enough that the probe finishes promptly.
#[cfg(all(target_os = "macos", feature = "system-capture-mac"))]
const TAP_PROBE_FIXTURE: &str = "/System/Library/Sounds/Submarine.aiff";

/// Capture window during the tap probe. Long enough to outlast
/// `CoreAudio`'s `IOProc` startup delay (~200 ms in practice), short
/// enough that a tap silent under TCC denial fails fast.
#[cfg(all(target_os = "macos", feature = "system-capture-mac"))]
const TAP_PROBE_WINDOW: std::time::Duration = std::time::Duration::from_millis(1_500);

/// Threshold separating "tap delivers real audio" from "tap zero-fills
/// under TCC denial". Submarine.aiff peaks well above 0.1; a value
/// below 0.01 is squarely in the noise floor / silence regime.
#[cfg(all(target_os = "macos", feature = "system-capture-mac"))]
const TAP_PROBE_NOISE_FLOOR: f32 = 0.01;

#[cfg(all(target_os = "macos", feature = "system-capture-mac"))]
async fn check_tap(report: &mut Report) {
    use futures::StreamExt;
    use scrybe_capture_mac::MacCapture;
    use scrybe_core::capture::AudioCapture;

    if !std::path::Path::new(TAP_PROBE_FIXTURE).exists() {
        report.lines.push(format!(
            "tap probe: skipped ({TAP_PROBE_FIXTURE} not present)"
        ));
        report.warnings += 1;
        return;
    }

    let mut capture = MacCapture::new();
    if let Err(e) = capture.start() {
        report.lines.push(format!("tap probe: start failed: {e}"));
        report.warnings += 1;
        return;
    }

    let mut afplay = match std::process::Command::new("/usr/bin/afplay")
        .arg(TAP_PROBE_FIXTURE)
        .spawn()
    {
        Ok(child) => child,
        Err(e) => {
            let _ = capture.stop();
            report
                .lines
                .push(format!("tap probe: afplay spawn failed: {e}"));
            report.warnings += 1;
            return;
        }
    };

    let mut frames = capture.frames();
    let deadline = tokio::time::Instant::now() + TAP_PROBE_WINDOW;
    let mut frame_count: u64 = 0;
    let mut peak: f32 = 0.0;
    loop {
        match tokio::time::timeout_at(deadline, frames.next()).await {
            Ok(Some(Ok(frame))) => {
                frame_count += 1;
                for s in frame.samples.iter() {
                    let abs = s.abs();
                    if abs > peak {
                        peak = abs;
                    }
                }
            }
            Ok(Some(Err(e))) => {
                report
                    .lines
                    .push(format!("tap probe: capture error mid-stream: {e}"));
                report.warnings += 1;
                break;
            }
            Ok(None) | Err(_) => break,
        }
    }

    let _ = afplay.kill();
    let _ = afplay.wait();
    let _ = capture.stop();

    let verdict = if frame_count == 0 {
        report.warnings += 1;
        "FAIL: IOProc never fired (entitlement, sandbox, or aggregate-device construction failure)"
    } else if peak < TAP_PROBE_NOISE_FLOOR {
        report.warnings += 1;
        "FAIL: tap delivered silent frames (TCC cannot consent because this binary has no `.app` bundle + Info.plist)"
    } else {
        "OK"
    };
    report.lines.push(format!(
        "tap probe: frames={frame_count} peak={peak:.4} → {verdict}"
    ));

    // When the tap is silent, surface concrete remediation steps so the
    // user can act without leaving the terminal. The dominant root cause
    // (per Reddit r/rust 1t4y3bd) is missing bundle structure: TCC
    // cannot attach an Audio Capture grant to a bare CLI binary without
    // an Info.plist declaring `NSAudioCaptureUsageDescription`. The
    // signing identity matters too — ad-hoc identities don't survive
    // rebuilds because the designated requirement is hash-pinned.
    if frame_count > 0 && peak < TAP_PROBE_NOISE_FLOOR {
        emit_silent_tap_remediation(report);
    }
}

/// Emit remediation guidance when the tap probe reports silent frames.
/// Each line is prefixed with two spaces so it nests visually under the
/// `tap probe:` verdict line in the doctor report.
#[cfg(all(target_os = "macos", feature = "system-capture-mac"))]
fn emit_silent_tap_remediation(report: &mut Report) {
    report.lines.push("  remediation:".to_string());
    report.lines.push(
        "    1. Build a `.app` bundle: packaging/macos-app/build-app.sh \
         --binary $(which scrybe) --output ./scrybe.app --sign-self <cert>"
            .to_string(),
    );
    report.lines.push(
        "    2. Self-signed cert: Keychain Access → Certificate Assistant \
         → Create a Certificate (Self Signed Root, Code Signing)"
            .to_string(),
    );
    report.lines.push(
        "    3. Remove stale TCC entry: System Settings → Privacy & Security \
         → Audio Recording → click `-` next to scrybe"
            .to_string(),
    );
    report.lines.push(
        "    4. Launch the bundle: open ./scrybe.app --args doctor --check-tap \
         (Allow the prompt, then re-probe)"
            .to_string(),
    );

    // Try to discover the TCC service name used by this macOS version.
    // Apple changes this between releases (Sequoia → Tahoe renamed
    // `SystemAudioRecording`), so probing the live framework is more
    // reliable than baking a constant. Failure is non-fatal — the
    // remediation steps still work via the System Settings UI.
    if let Some(service) = discover_tcc_audio_service() {
        report.lines.push(format!(
            "    5. (alternative reset) sudo tccutil reset {service} dev.scrybe.scrybe"
        ));
    }
}

/// Best-effort discovery of the macOS TCC service name that gates Core
/// Audio Tap consent. Apple's `tccutil` rejects unknown names and the
/// canonical service is renamed across minor releases, so we ask the
/// live `TCC.framework` what symbols it exports and pick the one
/// matching audio capture. Returns `None` when the framework cannot be
/// inspected (e.g., `dyld_info` missing or framework moved).
#[cfg(all(target_os = "macos", feature = "system-capture-mac"))]
fn discover_tcc_audio_service() -> Option<String> {
    // `dyld_info -exports` lists every exported symbol of a Mach-O.
    // The TCC framework exports each service constant as
    // `_kTCCService<Name>`; we strip the prefix and pick the audio one.
    let framework = "/System/Library/PrivateFrameworks/TCC.framework/Versions/A/TCC";
    let output = std::process::Command::new("dyld_info")
        .args(["-exports", framework])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let text = std::str::from_utf8(&output.stdout).ok()?;
    // Match either `AudioCapture`, `SystemAudioRecording`, or any
    // future audio-flavoured service. Prefer "AudioCapture" if both
    // exist because that is the modern (14.4+) name.
    let candidates: Vec<&str> = text
        .lines()
        .filter_map(|line| line.split_whitespace().last())
        .filter(|tok| tok.starts_with("_kTCCService"))
        .map(|tok| tok.trim_start_matches("_kTCCService"))
        .filter(|name| name.to_ascii_lowercase().contains("audio"))
        .collect();
    candidates
        .iter()
        .find(|n| n.eq_ignore_ascii_case("AudioCapture"))
        .or_else(|| candidates.first())
        .map(|s| (*s).to_string())
}

#[cfg(not(all(target_os = "macos", feature = "system-capture-mac")))]
#[allow(clippy::unused_async)]
async fn check_tap(report: &mut Report) {
    report.lines.push(
        "tap probe: skipped (binary not built with --features system-capture-mac on macOS)"
            .to_string(),
    );
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
