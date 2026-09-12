// Copyright 2026 Mathews Tom
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.

//! Install a locally self-signed macOS bundle from the running CLI binary.

use std::path::PathBuf;

#[cfg(target_os = "macos")]
use anyhow::Context;
use anyhow::Result;
use clap::Args as ClapArgs;

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
    let output = args
        .output
        .clone()
        .unwrap_or(crate::macos_bundle::default_bundle_path()?);
    let binary = std::env::current_exe().context("resolving installed scrybe executable")?;
    let identity = crate::macos_bundle::resolve_signing_identity(Some(&args.sign_self))?;
    crate::macos_bundle::install_bundle(&binary, &output, &identity)?;
    match crate::macos_bundle::inspect_bundle(&output) {
        crate::macos_bundle::BundleState::Ready => {}
        state => anyhow::bail!("installed bundle failed final validation: {state:?}"),
    }
    println!("macOS bundle installed: {}", output.display());
    Ok(())
}
