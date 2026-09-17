#![allow(clippy::expect_used)]

use std::process::Command;

const fn scrybe_bin() -> &'static str {
    env!("CARGO_BIN_EXE_scrybe")
}

#[cfg(target_os = "macos")]
#[test]
fn plain_nonterminal_doctor_skips_live_sck_probe_without_reading_stdin() {
    let config_home = tempfile::tempdir().expect("create isolated config home");
    let root = tempfile::tempdir().expect("create isolated storage root");
    // An explicit configuration naming remote providers, so neither of
    // the provider probes fires. The built-in defaults name a locally
    // managed transcription model and a loopback notes endpoint, and
    // Doctor now reports on both — so under the defaults this test's
    // exit line would depend on whether this machine happens to have a
    // model installed and something listening on that port, neither of
    // which is what it is about.
    let config = config_home.path().join("config.toml");
    std::fs::write(
        &config,
        "schema_version = 1\n\n\
         [stt]\nprovider = \"openai-compat\"\nbase_url = \"https://stt.example/v1\"\n\n\
         [llm]\nbase_url = \"https://notes.example/v1\"\n",
    )
    .expect("write isolated configuration");
    let output = Command::new(scrybe_bin())
        .args(["doctor", "--root"])
        .arg(root.path())
        .env("SCRYBE_CONFIG", &config)
        .output()
        .expect("run doctor");

    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).expect("doctor stdout is UTF-8");
    assert!(stdout.contains("system audio backend: ScreenCaptureKit"));
    assert!(stdout.contains("sck probe: skipped (non-interactive)"));
    assert!(stdout.contains("scrybe doctor: ok"));
}

#[test]
fn doctor_fix_requires_an_explicit_signing_identity() {
    let output = Command::new(scrybe_bin())
        .args(["doctor", "--fix"])
        .output()
        .expect("run doctor help validation");

    assert!(!output.status.success());
    let stderr = String::from_utf8(output.stderr).expect("doctor stderr is UTF-8");
    assert!(stderr.contains("--sign-self"));
}

#[test]
fn doctor_signing_identity_requires_fix_mode() {
    let output = Command::new(scrybe_bin())
        .args(["doctor", "--sign-self", "scrybe-local-signing"])
        .output()
        .expect("run doctor help validation");

    assert!(!output.status.success());
    let stderr = String::from_utf8(output.stderr).expect("doctor stderr is UTF-8");
    assert!(stderr.contains("--fix"));
}
