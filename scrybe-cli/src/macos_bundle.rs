// Copyright 2026 Mathews Tom
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//     https://www.apache.org/licenses/LICENSE-2.0

//! Discovery, validation, and safe replacement of the local macOS TCC bundle.

use std::ffi::{OsStr, OsString};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result};

const BUNDLE_FILE_NAME: &str = "scrybe.app";
const BUNDLE_IDENTIFIER: &str = "dev.scrybe.scrybe";
const BUNDLE_IDENTIFIER_KEY: &str = "<key>CFBundleIdentifier</key>";
const BUNDLE_VERSION_KEY: &str = "<key>CFBundleShortVersionString</key>";
#[cfg(feature = "system-capture-mac")]
const SCRYBE_BUNDLE_ENV: &str = "SCRYBE_BUNDLE";
const PLIST_TEMPLATE: &str = include_str!("../assets/macos/Info.plist.template");
const ENTITLEMENTS: &str = include_str!("../assets/macos/entitlements.plist");

pub const PROJECT_SIGNING_IDENTITY: &str = "scrybe-local-signing";

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum BundleState {
    Missing,
    Invalid { reason: String },
    Stale { found_version: String },
    Ready,
}

#[derive(Debug)]
struct CommandOutput {
    success: bool,
    stdout: Vec<u8>,
    stderr: Vec<u8>,
}

trait CommandRunner {
    fn output(&self, program: &OsStr, args: &[OsString]) -> Result<CommandOutput>;
}

struct SystemCommandRunner;

impl CommandRunner for SystemCommandRunner {
    fn output(&self, program: &OsStr, args: &[OsString]) -> Result<CommandOutput> {
        let output = Command::new(program)
            .args(args)
            .output()
            .with_context(|| format!("running {}", Path::new(program).display()))?;
        Ok(CommandOutput {
            success: output.status.success(),
            stdout: output.stdout,
            stderr: output.stderr,
        })
    }
}

pub fn default_bundle_path() -> Result<PathBuf> {
    let home = directories::UserDirs::new()
        .context("resolving home directory")?
        .home_dir()
        .to_path_buf();
    Ok(home.join("Applications").join(BUNDLE_FILE_NAME))
}

#[cfg(feature = "system-capture-mac")]
pub fn find_existing_bundle() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os(SCRYBE_BUNDLE_ENV).map(PathBuf::from) {
        if path.is_dir() {
            return Some(path);
        }
    }
    if let Ok(cwd) = std::env::current_dir() {
        let path = cwd.join(BUNDLE_FILE_NAME);
        if path.is_dir() {
            return Some(path);
        }
    }
    let system = PathBuf::from("/Applications").join(BUNDLE_FILE_NAME);
    if system.is_dir() {
        return Some(system);
    }
    default_bundle_path().ok().filter(|path| path.is_dir())
}
#[cfg(feature = "system-capture-mac")]
pub fn repair_destination() -> Result<PathBuf> {
    if let Some(path) = std::env::var_os(SCRYBE_BUNDLE_ENV).map(PathBuf::from) {
        return Ok(path);
    }
    Ok(find_existing_bundle().map_or(default_bundle_path()?, |path| path))
}

#[cfg(feature = "system-capture-mac")]
pub fn already_inside_bundle() -> bool {
    std::env::current_exe()
        .ok()
        .is_some_and(|path| path.to_string_lossy().contains(".app/Contents/MacOS/"))
}

pub fn inspect_bundle(path: &Path) -> BundleState {
    inspect_bundle_with(path, env!("CARGO_PKG_VERSION"), &SystemCommandRunner)
}

fn inspect_bundle_with(
    path: &Path,
    expected_version: &str,
    runner: &impl CommandRunner,
) -> BundleState {
    if !path.exists() {
        return BundleState::Missing;
    }
    if !path.is_dir() {
        return invalid("bundle path is not a directory");
    }

    let plist = path.join("Contents/Info.plist");
    let executable = path.join("Contents/MacOS/scrybe");
    let entitlements = path.join("Contents/Resources/entitlements.plist");
    for (label, required) in [
        ("Info.plist", &plist),
        ("executable", &executable),
        ("entitlements", &entitlements),
    ] {
        if !required.is_file() {
            return invalid(format!("missing required {label}"));
        }
    }

    let plist_text = match fs::read_to_string(&plist) {
        Ok(value) => value,
        Err(error) => return invalid(format!("reading Info.plist failed: {error}")),
    };
    let Some(bundle_identifier) = plist_string_after_key(&plist_text, BUNDLE_IDENTIFIER_KEY) else {
        return invalid("Info.plist has no CFBundleIdentifier string");
    };
    if bundle_identifier != BUNDLE_IDENTIFIER {
        return invalid(format!("bundle identifier is not {BUNDLE_IDENTIFIER}"));
    }
    let Some(found_version) = plist_string_after_key(&plist_text, BUNDLE_VERSION_KEY) else {
        return invalid("Info.plist has no CFBundleShortVersionString");
    };
    if found_version.is_empty() {
        return invalid("Info.plist has an empty CFBundleShortVersionString");
    }

    let verify_args = os_args(["--verify", "--deep", "--strict"])
        .into_iter()
        .chain([path.as_os_str().to_owned()])
        .collect::<Vec<_>>();
    match runner.output(OsStr::new("codesign"), &verify_args) {
        Ok(output) if output.success => {}
        Ok(output) => {
            return invalid(command_failure("codesign verification", &output));
        }
        Err(error) => return invalid(format!("codesign verification failed: {error:#}")),
    }

    if found_version != expected_version {
        return BundleState::Stale {
            found_version: found_version.to_string(),
        };
    }
    BundleState::Ready
}

fn plist_string_after_key<'a>(plist: &'a str, key: &str) -> Option<&'a str> {
    let (_, after_key) = plist.split_once(key)?;
    let (_, value_and_rest) = after_key.split_once("<string>")?;
    let (value, _) = value_and_rest.split_once("</string>")?;
    Some(value.trim())
}

pub fn resolve_signing_identity(requested: Option<&str>) -> Result<String> {
    resolve_signing_identity_with(requested, &SystemCommandRunner)
}

fn resolve_signing_identity_with(
    requested: Option<&str>,
    runner: &impl CommandRunner,
) -> Result<String> {
    let identity = requested.unwrap_or(PROJECT_SIGNING_IDENTITY);
    let output = runner.output(
        OsStr::new("security"),
        &os_args(["find-identity", "-v", "-p", "codesigning"]),
    )?;
    let identities = String::from_utf8_lossy(&output.stdout);
    if output.success && identities.contains(&format!("\"{identity}\"")) {
        return Ok(identity.to_string());
    }
    anyhow::bail!(missing_identity_message(identity));
}

pub fn install_bundle(binary: &Path, destination: &Path, identity: &str) -> Result<()> {
    install_bundle_with(binary, destination, identity, &SystemCommandRunner)
}

fn install_bundle_with(
    binary: &Path,
    destination: &Path,
    identity: &str,
    runner: &impl CommandRunner,
) -> Result<()> {
    let parent = destination
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent)
        .with_context(|| format!("creating bundle parent {}", parent.display()))?;
    let candidate = unused_sibling_path(destination, "install")?;
    fs::create_dir(&candidate)
        .with_context(|| format!("creating bundle candidate {}", candidate.display()))?;

    let prepare_result = prepare_candidate(binary, &candidate, identity, runner);
    if let Err(error) = prepare_result {
        let _ = fs::remove_dir_all(&candidate);
        return Err(error);
    }

    if let Err(error) = replace_verified_candidate(&candidate, destination) {
        let _ = fs::remove_dir_all(&candidate);
        return Err(error);
    }
    Ok(())
}

fn prepare_candidate(
    binary: &Path,
    candidate: &Path,
    identity: &str,
    runner: &impl CommandRunner,
) -> Result<()> {
    let macos = candidate.join("Contents/MacOS");
    let resources = candidate.join("Contents/Resources");
    fs::create_dir_all(&macos).with_context(|| format!("creating {}", macos.display()))?;
    fs::create_dir_all(&resources).with_context(|| format!("creating {}", resources.display()))?;
    fs::copy(binary, macos.join("scrybe")).context("copying installed scrybe into bundle")?;
    fs::write(
        candidate.join("Contents/Info.plist"),
        render_plist(env!("CARGO_PKG_VERSION")),
    )
    .context("writing bundled Info.plist")?;
    let entitlements = resources.join("entitlements.plist");
    fs::write(&entitlements, ENTITLEMENTS).context("writing bundle entitlements")?;

    let sign_args = vec![
        OsString::from("--force"),
        OsString::from("--options"),
        OsString::from("runtime"),
        OsString::from("--sign"),
        OsString::from(identity),
        OsString::from("--entitlements"),
        entitlements.into_os_string(),
        candidate.as_os_str().to_owned(),
    ];
    run_checked(runner, "codesign", &sign_args, "signing bundle candidate")?;

    match inspect_bundle_with(candidate, env!("CARGO_PKG_VERSION"), runner) {
        BundleState::Ready => Ok(()),
        state => anyhow::bail!("verifying bundle candidate failed: {state:?}"),
    }
}

fn replace_verified_candidate(candidate: &Path, destination: &Path) -> Result<()> {
    if !destination.exists() {
        return fs::rename(candidate, destination)
            .with_context(|| format!("installing verified bundle at {}", destination.display()));
    }

    let backup = unused_sibling_path(destination, "backup")?;
    fs::rename(destination, &backup).with_context(|| {
        format!(
            "moving existing bundle {} to temporary backup {}",
            destination.display(),
            backup.display()
        )
    })?;
    if let Err(error) = fs::rename(candidate, destination) {
        fs::rename(&backup, destination).with_context(|| {
            format!("restoring existing bundle after replacement failed: {error}")
        })?;
        return Err(error)
            .with_context(|| format!("installing verified bundle at {}", destination.display()));
    }
    fs::remove_dir_all(&backup)
        .with_context(|| format!("removing replaced bundle backup {}", backup.display()))?;
    Ok(())
}

fn unused_sibling_path(destination: &Path, label: &str) -> Result<PathBuf> {
    let parent = destination
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let pid = std::process::id();
    for attempt in 0_u8..100 {
        let candidate = parent.join(format!(".scrybe-{label}-{pid}-{attempt}.app"));
        if !candidate.exists() {
            return Ok(candidate);
        }
    }
    anyhow::bail!(
        "could not reserve a temporary bundle path beside {}",
        destination.display()
    );
}

fn run_checked(
    runner: &impl CommandRunner,
    program: &str,
    args: &[OsString],
    operation: &str,
) -> Result<()> {
    let output = runner.output(OsStr::new(program), args)?;
    if output.success {
        return Ok(());
    }
    anyhow::bail!(command_failure(operation, &output));
}

fn command_failure(operation: &str, output: &CommandOutput) -> String {
    let stderr = String::from_utf8_lossy(&output.stderr);
    let detail = stderr.trim();
    if detail.is_empty() {
        format!("{operation} returned a non-zero status")
    } else {
        format!("{operation} failed: {detail}")
    }
}

fn invalid(reason: impl Into<String>) -> BundleState {
    BundleState::Invalid {
        reason: reason.into(),
    }
}

fn missing_identity_message(identity: &str) -> String {
    format!(
        "self-signed Keychain identity `{identity}` was not found; create one in Keychain Access → Certificate Assistant → Create a Certificate (Identity Type: Self Signed Root, Certificate Type: Code Signing)"
    )
}

fn render_plist(version: &str) -> String {
    PLIST_TEMPLATE.replace("{{VERSION}}", version)
}

fn os_args<const N: usize>(values: [&str; N]) -> Vec<OsString> {
    values.into_iter().map(OsString::from).collect()
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used)]
#[path = "../tests/unit/macos_bundle.rs"]
mod tests;
