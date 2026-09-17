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

use std::io::{BufRead, IsTerminal, Write};
use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::Args as ClapArgs;
use scrybe_application::diagnostics::{DiagnosticCode, DiagnosticReport, Severity};
use scrybe_core::config::{Config, RECORD_SOURCE_MIC_SYSTEM, RECORD_SYSTEM_BACKEND_TAP};
use scrybe_core::record_defaults;

use crate::runtime::application;

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

    /// Probe the macOS `ScreenCaptureKit` system-audio adapter
    /// end-to-end. Requires Screen & System Audio Recording permission.
    #[arg(long, default_value_t = false)]
    pub check_sck: bool,

    /// Repair the configured Core Audio Tap bundle without prompting.
    /// Requires `--sign-self`.
    #[arg(long, default_value_t = false, requires = "sign_self")]
    pub fix: bool,

    /// Named self-signed Keychain identity used by `--fix`.
    #[arg(long, requires = "fix")]
    pub sign_self: Option<String>,
}

#[allow(clippy::unused_async)]
pub async fn run(args: Args) -> Result<()> {
    let mut report = Report::default();

    let app = application(args.root.as_deref())?;
    report.lines.push(format!(
        "config: {} (exists={})",
        app.config().path().display(),
        app.config().path().exists()
    ));

    let cfg = app.config().load()?;
    let root = app.root().path();
    report.lines.push(format!(
        "storage root: {} (exists={})",
        root.display(),
        root.exists()
    ));

    let diagnosis = app
        .diagnostics()
        .diagnose(app.config(), app.sessions())
        .map_err(anyhow::Error::from)?;
    absorb(&diagnosis, &mut report);

    run_capture_onboarding(&cfg, &args, &mut report).await?;

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

/// Folds one read-only diagnosis into the textual report this command
/// has always printed.
///
/// Severity decides the warning count, so what counts as a warning is
/// the diagnosis's judgement rather than a second one made here.
fn absorb(diagnosis: &DiagnosticReport, report: &mut Report) {
    for found in &diagnosis.findings {
        let line = match found.code {
            DiagnosticCode::SttEgressLocal | DiagnosticCode::SttEgressRemote => {
                format!("stt egress: {}", found.summary)
            }
            DiagnosticCode::LlmEgressLocal | DiagnosticCode::LlmEgressRemote => {
                format!("llm egress: {}", found.summary)
            }
            _ => found.summary.clone(),
        };
        report.lines.push(line);
        if found.severity >= Severity::Warning {
            report.warnings += 1;
        }
    }
}

#[derive(Default, Debug)]
struct Report {
    lines: Vec<String>,
    warnings: u32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum OnboardingTarget {
    MicrophoneOnly,
    ScreenCaptureKit,
    CoreAudioTap,
}

fn effective_onboarding_target(cfg: &Config) -> OnboardingTarget {
    if record_defaults::ergonomic_source(&cfg.record) != RECORD_SOURCE_MIC_SYSTEM {
        return OnboardingTarget::MicrophoneOnly;
    }
    if cfg.record.validated_system_backend() == Some(RECORD_SYSTEM_BACKEND_TAP) {
        OnboardingTarget::CoreAudioTap
    } else {
        OnboardingTarget::ScreenCaptureKit
    }
}

async fn run_capture_onboarding(cfg: &Config, args: &Args, report: &mut Report) -> Result<()> {
    let target = effective_onboarding_target(cfg);
    if args.check_sck {
        check_sck(report).await;
    }
    if args.check_tap {
        return run_tap_onboarding(args, report, true).await;
    }
    if args.fix {
        if target == OnboardingTarget::CoreAudioTap {
            return run_tap_onboarding(args, report, false).await;
        }
        report
            .lines
            .push("macOS onboarding: no Core Audio Tap bundle repair is applicable".to_string());
        return Ok(());
    }
    if args.check_sck {
        return Ok(());
    }

    match target {
        OnboardingTarget::MicrophoneOnly => {
            report.lines.push(
                "capture onboarding: microphone-only; no system-audio probe required".to_string(),
            );
        }
        OnboardingTarget::ScreenCaptureKit => {
            report
                .lines
                .push("system audio backend: ScreenCaptureKit".to_string());
            if terminal_is_interactive() {
                if confirm_optional("Run the live system-audio permission check now? [y/N] ")
                    .await?
                {
                    check_sck(report).await;
                } else {
                    report.lines.push(
                        "sck probe: declined; run `scrybe doctor --check-sck` later".to_string(),
                    );
                }
            } else {
                report.lines.push(
                    "sck probe: skipped (non-interactive); run `scrybe doctor --check-sck`"
                        .to_string(),
                );
            }
        }
        OnboardingTarget::CoreAudioTap => {
            run_tap_onboarding(args, report, false).await?;
        }
    }
    Ok(())
}

fn terminal_is_interactive() -> bool {
    std::io::stdin().is_terminal() && std::io::stderr().is_terminal()
}

async fn confirm_optional(prompt: &str) -> Result<bool> {
    let prompt = prompt.to_string();
    tokio::task::spawn_blocking(move || -> Result<bool> {
        let stderr = std::io::stderr();
        let mut writer = stderr.lock();
        writer
            .write_all(prompt.as_bytes())
            .context("writing doctor prompt")?;
        writer.flush().context("flushing doctor prompt")?;
        drop(writer);

        let stdin = std::io::stdin();
        let mut answer = String::new();
        stdin
            .lock()
            .read_line(&mut answer)
            .context("reading doctor response")?;
        Ok(crate::prompter::is_affirmative_response(&answer))
    })
    .await
    .context("joining doctor prompt task")?
}

#[cfg(all(target_os = "macos", feature = "system-capture-mac"))]
async fn run_tap_onboarding(args: &Args, report: &mut Report, explicit_probe: bool) -> Result<()> {
    use crate::macos_bundle::BundleState;

    report
        .lines
        .push("system audio backend: Core Audio Tap".to_string());
    if crate::macos_bundle::already_inside_bundle() {
        check_tap(report).await;
        return Ok(());
    }

    let destination = crate::macos_bundle::repair_destination()?;
    let state = crate::macos_bundle::inspect_bundle(&destination);
    report.lines.push(bundle_state_line(&destination, &state));
    let ready = matches!(state, BundleState::Ready);

    if !ready {
        if args.fix {
            let identity =
                crate::macos_bundle::resolve_signing_identity(args.sign_self.as_deref())?;
            install_current_bundle(&destination, &identity)?;
            report.lines.push(format!(
                "tap bundle repaired: {} (identity={identity})",
                destination.display()
            ));
        } else if terminal_is_interactive() {
            let identity = match crate::macos_bundle::resolve_signing_identity(None) {
                Ok(identity) => identity,
                Err(error) => {
                    report
                        .lines
                        .push(format!("tap bundle repair unavailable: {error}"));
                    report.warnings += 1;
                    return Ok(());
                }
            };
            eprintln!(
                "Core Audio Tap bundle repair:\n  destination: {}\n  identity: {identity}",
                destination.display()
            );
            if !confirm_optional("Repair the Core Audio Tap bundle now? [y/N] ").await? {
                report.lines.push(format!(
                    "tap bundle repair: declined; run `scrybe doctor --check-tap --fix --sign-self {identity}`"
                ));
                report.warnings += 1;
                return Ok(());
            }
            install_current_bundle(&destination, &identity)?;
            report.lines.push(format!(
                "tap bundle repaired: {} (identity={identity})",
                destination.display()
            ));
        } else {
            report.lines.push(
                "tap bundle repair: skipped (non-interactive); run `scrybe doctor --check-tap --fix --sign-self <identity>`"
                    .to_string(),
            );
            report.warnings += 1;
            return Ok(());
        }
    }

    if explicit_probe {
        run_bundled_tap_probe(&destination, report).await;
    } else if args.fix {
        return Ok(());
    } else if terminal_is_interactive() {
        if confirm_optional("Run the live Core Audio Tap permission check now? [y/N] ").await? {
            run_bundled_tap_probe(&destination, report).await;
        } else {
            report
                .lines
                .push("tap probe: declined; run `scrybe doctor --check-tap` later".to_string());
        }
    } else {
        report.lines.push(
            "tap probe: skipped (non-interactive); run `scrybe doctor --check-tap`".to_string(),
        );
    }
    Ok(())
}

#[cfg(all(target_os = "macos", feature = "system-capture-mac"))]
fn bundle_state_line(path: &std::path::Path, state: &crate::macos_bundle::BundleState) -> String {
    use crate::macos_bundle::BundleState;

    match state {
        BundleState::Missing => format!("tap bundle: missing ({})", path.display()),
        BundleState::Invalid { reason } => {
            format!("tap bundle: invalid ({reason}; {})", path.display())
        }
        BundleState::Stale { found_version } => format!(
            "tap bundle: stale (found {found_version}, need {}; {})",
            env!("CARGO_PKG_VERSION"),
            path.display()
        ),
        BundleState::Ready => format!("tap bundle: ready ({})", path.display()),
    }
}

#[cfg(all(target_os = "macos", feature = "system-capture-mac"))]
fn install_current_bundle(destination: &std::path::Path, identity: &str) -> Result<()> {
    let binary = std::env::current_exe().context("resolving installed scrybe executable")?;
    crate::macos_bundle::install_bundle(&binary, destination, identity)?;
    match crate::macos_bundle::inspect_bundle(destination) {
        crate::macos_bundle::BundleState::Ready => Ok(()),
        state => anyhow::bail!("repaired Tap bundle failed final validation: {state:?}"),
    }
}

#[cfg(all(target_os = "macos", feature = "system-capture-mac"))]
async fn run_bundled_tap_probe(destination: &std::path::Path, report: &mut Report) {
    eprintln!(
        "scrybe: launching Core Audio Tap diagnostic via {}",
        destination.display()
    );
    match crate::bundle_launcher::launch_doctor_probe_via_bundle(destination).await {
        Ok(output) => {
            report.lines.extend(
                output
                    .stdout
                    .lines()
                    .map(|line| format!("tap bundle stdout: {line}")),
            );
            report.lines.extend(
                output
                    .stderr
                    .lines()
                    .map(|line| format!("tap bundle stderr: {line}")),
            );
            if !output.success {
                report.warnings += 1;
                report.lines.push(
                    "tap bundle probe: bundled diagnostic did not report success".to_string(),
                );
            }
        }
        Err(error) => {
            report.warnings += 1;
            report
                .lines
                .push(format!("tap bundle probe: launch failed: {error:#}"));
        }
    }
}

#[cfg(not(all(target_os = "macos", feature = "system-capture-mac")))]
async fn run_tap_onboarding(args: &Args, report: &mut Report, _explicit_probe: bool) -> Result<()> {
    report
        .lines
        .push("system audio backend: Core Audio Tap".to_string());
    if args.fix {
        anyhow::bail!("Tap bundle repair requires macOS and the `system-capture-mac` feature");
    }
    check_tap(report).await;
    Ok(())
}

/// Capture window during the tap probe. Long enough to outlast
/// `CoreAudio`'s `IOProc` startup delay (~200 ms in practice) and to
/// hear the calibration chime loop at least once, short enough that
/// a tap silent under TCC denial fails fast.
#[cfg(all(target_os = "macos", feature = "system-capture-mac"))]
const TAP_PROBE_WINDOW: std::time::Duration = std::time::Duration::from_millis(1_500);

#[cfg(all(target_os = "macos", feature = "system-capture-mac"))]
async fn check_sck(report: &mut Report) {
    use futures::StreamExt;
    use scrybe_capture_mac::probe_chime::{play_probe_chime, PROBE_CHIME_PASS_THRESHOLD};
    use scrybe_capture_mac::SckCapture;
    use scrybe_core::capture::AudioCapture;

    let mut capture = SckCapture::new();
    if let Err(e) = capture.start() {
        report.lines.push(format!("sck probe: start failed: {e}"));
        report.warnings += 1;
        return;
    }

    let chime_handle = tokio::task::spawn_blocking(move || play_probe_chime(TAP_PROBE_WINDOW));
    let mut frames = capture.frames();
    let deadline = tokio::time::Instant::now() + TAP_PROBE_WINDOW;
    let mut frame_count: u64 = 0;
    let mut peak: f32 = 0.0;
    loop {
        match tokio::time::timeout_at(deadline, frames.next()).await {
            Ok(Some(Ok(frame))) => {
                frame_count += 1;
                for sample in frame.samples.iter() {
                    peak = peak.max(sample.abs());
                }
            }
            Ok(Some(Err(e))) => {
                report
                    .lines
                    .push(format!("sck probe: capture error mid-stream: {e}"));
                report.warnings += 1;
                break;
            }
            Ok(None) | Err(_) => break,
        }
    }
    let _ = capture.stop();
    match chime_handle.await {
        Ok(Ok(())) => {}
        Ok(Err(e)) => {
            report
                .lines
                .push(format!("sck probe: chime playback failed: {e}"));
            report.warnings += 1;
        }
        Err(e) => {
            report
                .lines
                .push(format!("sck probe: chime task failed: {e}"));
            report.warnings += 1;
        }
    }
    let verdict = if frame_count == 0 {
        report.warnings += 1;
        "FAIL: no frames received"
    } else if peak < PROBE_CHIME_PASS_THRESHOLD {
        report.warnings += 1;
        "FAIL: silent frames (Screen & System Audio Recording not granted)"
    } else {
        "OK"
    };
    report.lines.push(format!(
        "sck probe: frames={frame_count} peak={peak:.5} → {verdict}"
    ));
}
#[cfg(all(target_os = "macos", feature = "system-capture-mac"))]
async fn check_tap(report: &mut Report) {
    use futures::StreamExt;
    use scrybe_capture_mac::probe_chime::{play_probe_chime, PROBE_CHIME_PASS_THRESHOLD};
    use scrybe_capture_mac::MacCapture;
    use scrybe_core::capture::AudioCapture;

    let mut capture = MacCapture::new();
    if let Err(e) = capture.start() {
        report.lines.push(format!("tap probe: start failed: {e}"));
        report.warnings += 1;
        return;
    }

    // Play the calibration chime in-process, concurrently with the
    // capture loop below, for exactly `TAP_PROBE_WINDOW`. A Core
    // Audio Tap reads the digital pre-mix stream, so this chime still
    // lands at the tap as real nonzero samples when the tap is
    // granted, while a TCC-denied tap reads exact digital zeros
    // regardless of what is playing.
    let chime_handle = tokio::task::spawn_blocking(move || play_probe_chime(TAP_PROBE_WINDOW));

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

    let _ = capture.stop();

    match chime_handle.await {
        Ok(Ok(())) => {}
        Ok(Err(e)) => {
            report
                .lines
                .push(format!("tap probe: chime playback failed: {e}"));
            report.warnings += 1;
        }
        Err(e) => {
            report
                .lines
                .push(format!("tap probe: chime playback task panicked: {e}"));
            report.warnings += 1;
        }
    }

    let verdict = if frame_count == 0 {
        report.warnings += 1;
        "FAIL: IOProc never fired (entitlement, sandbox, or aggregate-device construction failure)"
    } else if peak < PROBE_CHIME_PASS_THRESHOLD {
        report.warnings += 1;
        "FAIL: tap delivered silent frames (Audio Capture permission denied, stale, or routed away)"
    } else {
        "OK"
    };
    report.lines.push(format!(
        "tap probe: frames={frame_count} peak={peak:.5} → {verdict}"
    ));

    // A running tap with zero-valued samples most often means macOS withheld
    // Audio Capture data from an otherwise valid bundle. Keep remediation on
    // the guided doctor path rather than asking users to invoke the app or
    // packaging script directly.
    if frame_count > 0 && peak < PROBE_CHIME_PASS_THRESHOLD {
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
        "    1. Remove stale TCC entry: System Settings → Privacy & Security \
         → Audio Recording → click `-` next to scrybe"
            .to_string(),
    );
    report.lines.push(
        "    2. Re-run `scrybe doctor --check-tap` and click Allow on the \
         Audio Capture prompt"
            .to_string(),
    );
    report.lines.push(
        "    3. If Doctor reports a bundle problem, repair it with \
         `scrybe doctor --check-tap --fix --sign-self scrybe-local-signing`"
            .to_string(),
    );

    // Try to discover the TCC service name used by this macOS version.
    // Apple changes this between releases (Sequoia → Tahoe renamed
    // `SystemAudioRecording`), so probing the live framework is more
    // reliable than baking a constant. Failure is non-fatal — the
    // remediation steps still work via the System Settings UI.
    if let Some(service) = discover_tcc_audio_service() {
        report.lines.push(format!(
            "    4. (alternative reset) sudo tccutil reset {service} dev.scrybe.scrybe"
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

#[cfg(not(all(target_os = "macos", feature = "system-capture-mac")))]
#[allow(clippy::unused_async)]
async fn check_sck(report: &mut Report) {
    report.lines.push(
        "sck probe: skipped (binary not built with --features system-capture-mac on macOS)"
            .to_string(),
    );
    report.warnings += 1;
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    fn diagnose(dir: &std::path::Path) -> DiagnosticReport {
        let app = scrybe_application::ScrybeApplication::new(
            scrybe_application::StorageRoot::new(dir.to_path_buf()),
            dir.join("absent-config.toml"),
        );
        app.diagnostics()
            .diagnose(app.config(), app.sessions())
            .unwrap()
    }

    #[test]
    fn test_absorb_renders_egress_findings_under_their_established_prefixes() {
        let dir = tempfile::tempdir().unwrap();
        let mut report = Report::default();

        absorb(&diagnose(dir.path()), &mut report);

        assert!(report
            .lines
            .iter()
            .any(|line| line.starts_with("stt egress: ")));
        assert!(report
            .lines
            .iter()
            .any(|line| line.starts_with("llm egress: ")));
    }

    #[test]
    fn test_absorb_leaves_a_clean_install_free_of_warnings() {
        let dir = tempfile::tempdir().unwrap();
        let mut report = Report::default();

        absorb(&diagnose(dir.path()), &mut report);

        assert_eq!(report.warnings, 0);
    }

    #[test]
    fn test_absorb_counts_one_warning_per_orphaned_partial_download() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("model.gguf.partial"), b"abc").unwrap();
        let mut report = Report::default();

        absorb(&diagnose(dir.path()), &mut report);

        assert_eq!(report.warnings, 1);
        assert!(report
            .lines
            .iter()
            .any(|line| line.contains("orphaned partial")));
    }

    #[test]
    fn test_absorb_reports_a_session_holding_a_lock() {
        let dir = tempfile::tempdir().unwrap();
        let folder = dir.path().join("2026-04-29-1430-session-01HXYZ");
        std::fs::create_dir(&folder).unwrap();
        std::fs::create_dir(folder.join("journal")).unwrap();
        std::fs::write(folder.join(scrybe_core::storage::PID_LOCK_NAME), b"1\n").unwrap();
        let mut report = Report::default();

        absorb(&diagnose(dir.path()), &mut report);

        assert!(report
            .lines
            .iter()
            .any(|line| line.contains("2026-04-29-1430-session-01HXYZ")));
    }

    #[test]
    fn onboarding_target_is_microphone_only_for_mic_source() {
        let mut cfg = Config::default();
        cfg.record.source = "mic".to_string();

        assert_eq!(
            effective_onboarding_target(&cfg),
            OnboardingTarget::MicrophoneOnly
        );
    }

    #[test]
    fn onboarding_target_uses_configured_system_backend() {
        let mut cfg = Config::default();
        cfg.record.source = RECORD_SOURCE_MIC_SYSTEM.to_string();
        cfg.record.system_backend = RECORD_SYSTEM_BACKEND_TAP.to_string();
        assert_eq!(
            effective_onboarding_target(&cfg),
            OnboardingTarget::CoreAudioTap
        );

        cfg.record.system_backend = "sck".to_string();
        assert_eq!(
            effective_onboarding_target(&cfg),
            OnboardingTarget::ScreenCaptureKit
        );
    }
}
