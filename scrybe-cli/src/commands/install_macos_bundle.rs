// Copyright 2026 Mathews Tom
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.

//! Install a locally self-signed macOS bundle from the running CLI binary.

#[cfg(target_os = "macos")]
use std::fs;
use std::path::PathBuf;
#[cfg(target_os = "macos")]
use std::process::Command;

#[cfg(target_os = "macos")]
use anyhow::Context;
use anyhow::Result;
use clap::Args as ClapArgs;

#[cfg(target_os = "macos")]
const PLIST_TEMPLATE: &str = include_str!("../../assets/macos/Info.plist.template");
#[cfg(target_os = "macos")]
const ENTITLEMENTS: &str = include_str!("../../assets/macos/entitlements.plist");

#[derive(ClapArgs, Debug)]
pub struct Args {
    /// Named self-signed Keychain code-signing identity.
    #[arg(long)]
    sign_self: String,

    /// Bundle destination. Defaults to `~/Applications/scrybe.app`.
    #[arg(long)]
    output: Option<PathBuf>,
}

pub fn run(args: &Args) -> Result<()> {
    #[cfg(not(target_os = "macos"))]
    {
        let _ = args;
        anyhow::bail!("install-macos-bundle is only available on macOS");
    }
    #[cfg(target_os = "macos")]
    install(args)
}

#[cfg(target_os = "macos")]
fn install(args: &Args) -> Result<()> {
    verify_identity(&args.sign_self)?;
    let output = args.output.clone().unwrap_or(default_output()?);
    let binary = std::env::current_exe().context("resolving installed scrybe executable")?;

    if output.exists() {
        fs::remove_dir_all(&output)
            .with_context(|| format!("removing existing bundle {}", output.display()))?;
    }
    let macos = output.join("Contents/MacOS");
    let resources = output.join("Contents/Resources");
    fs::create_dir_all(&macos).with_context(|| format!("creating {}", macos.display()))?;
    fs::create_dir_all(&resources).with_context(|| format!("creating {}", resources.display()))?;
    fs::copy(&binary, macos.join("scrybe")).context("copying installed scrybe into bundle")?;
    fs::write(
        output.join("Contents/Info.plist"),
        render_plist(env!("CARGO_PKG_VERSION")),
    )
    .context("writing bundled Info.plist")?;
    let entitlements = resources.join("entitlements.plist");
    fs::write(&entitlements, ENTITLEMENTS).context("writing bundle entitlements")?;

    run_codesign(&[
        "--force",
        "--options",
        "runtime",
        "--sign",
        &args.sign_self,
        "--entitlements",
        entitlements.to_str().context("encoding entitlement path")?,
        output.to_str().context("encoding bundle path")?,
    ])?;
    run_codesign(&[
        "--verify",
        "--deep",
        "--strict",
        "--verbose=2",
        output.to_str().context("encoding bundle path")?,
    ])?;
    println!("macOS bundle installed: {}", output.display());
    Ok(())
}

#[cfg(target_os = "macos")]
fn verify_identity(identity: &str) -> Result<()> {
    let output = Command::new("security")
        .args(["find-identity", "-v", "-p", "codesigning"])
        .output()
        .context("querying Keychain code-signing identities")?;
    let identities = String::from_utf8_lossy(&output.stdout);
    if output.status.success() && identities.contains(&format!("\"{identity}\"")) {
        return Ok(());
    }
    anyhow::bail!(
        "self-signed Keychain identity `{identity}` was not found; create one in Keychain Access → Certificate Assistant → Create a Certificate (Identity Type: Self Signed Root, Certificate Type: Code Signing)"
    );
}

#[cfg(target_os = "macos")]
fn default_output() -> Result<PathBuf> {
    let home = directories::UserDirs::new()
        .context("resolving home directory")?
        .home_dir()
        .to_path_buf();
    Ok(home.join("Applications/scrybe.app"))
}

#[cfg(target_os = "macos")]
fn run_codesign(args: &[&str]) -> Result<()> {
    let status = Command::new("codesign")
        .args(args)
        .status()
        .context("running codesign")?;
    if status.success() {
        return Ok(());
    }
    anyhow::bail!("codesign failed with {status}");
}

#[cfg(target_os = "macos")]
fn render_plist(version: &str) -> String {
    PLIST_TEMPLATE.replace("{{VERSION}}", version)
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::*;

    #[test]
    fn rendered_plist_has_no_unexpanded_version_marker() {
        let plist = render_plist("1.3.1");

        assert!(plist.contains("<string>1.3.1</string>"));
        assert!(!plist.contains("{{VERSION}}"));
    }
}
