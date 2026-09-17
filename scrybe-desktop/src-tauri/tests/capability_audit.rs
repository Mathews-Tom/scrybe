// Copyright 2026 Mathews Tom
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//     https://www.apache.org/licenses/LICENSE-2.0

//! The capability files and the registered commands describe one
//! surface.
//!
//! Tauri rejects an invoke that no capability grants, so a command
//! missing from the capability files is unreachable and a grant with no
//! command behind it is a stale widening of the trust boundary. Neither
//! shows up as a compile error, so both are asserted here.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use scrybe_desktop::commands::COMMANDS;

fn host_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).to_path_buf()
}

/// Every `allow-<command>` grant across every capability file.
fn granted_commands() -> BTreeSet<String> {
    let mut granted = BTreeSet::new();
    let directory = host_root().join("capabilities");
    for entry in std::fs::read_dir(&directory).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().is_none_or(|extension| extension != "json") {
            continue;
        }
        let capability: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        let permissions = capability["permissions"].as_array().unwrap();
        for permission in permissions {
            let permission = permission.as_str().unwrap();
            if let Some(command) = permission.strip_prefix("allow-") {
                granted.insert(command.replace('-', "_"));
            }
        }
    }
    granted
}

#[test]
fn test_every_registered_command_is_granted_to_exactly_one_window() {
    let registered: BTreeSet<String> = COMMANDS.iter().map(|name| (*name).to_owned()).collect();

    assert_eq!(
        granted_commands(),
        registered,
        "\nthe capability files and `commands::COMMANDS` disagree. A command \
         the files do not grant is rejected at the IPC boundary; a grant with \
         no command behind it widens the trust boundary for nothing.\n",
    );
}

#[test]
fn test_the_access_control_manifest_declares_the_same_commands_the_host_registers() {
    // `build.rs` names the commands Tauri generates `allow-`/`deny-`
    // permissions from. A build script runs before the crate it builds,
    // so it cannot import `commands::COMMANDS`; the two lists are
    // compared here instead. A command missing from `build.rs` has no
    // permission, so no capability can grant it and every invoke of it
    // is rejected.
    let build_script = std::fs::read_to_string(host_root().join("build.rs")).unwrap();
    let declaration = build_script
        .split_once("const COMMANDS: &[&str] = &[")
        .and_then(|(_, rest)| rest.split_once("];"))
        .map(|(body, _)| body)
        .expect("build.rs no longer declares a COMMANDS array");

    let declared: BTreeSet<String> = declaration
        .split('"')
        .skip(1)
        .step_by(2)
        .map(str::to_owned)
        .collect();
    let registered: BTreeSet<String> = COMMANDS.iter().map(|name| (*name).to_owned()).collect();

    assert_eq!(declared, registered);
}

#[test]
fn test_no_capability_applies_to_an_unnamed_window() {
    let directory = host_root().join("capabilities");
    for entry in std::fs::read_dir(&directory).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().is_none_or(|extension| extension != "json") {
            continue;
        }
        let capability: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        let windows = capability["windows"].as_array().unwrap_or_else(|| {
            panic!(
                "{}: a capability without `windows` applies everywhere",
                path.display()
            )
        });
        assert!(
            !windows.is_empty(),
            "{}: an empty `windows` list applies everywhere",
            path.display(),
        );
    }
}
