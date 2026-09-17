// Copyright 2026 Mathews Tom
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//     https://www.apache.org/licenses/LICENSE-2.0

//! Declares the application's access-control manifest.
//!
//! Naming the commands here makes the frontend's reachable surface a
//! reviewed list: Tauri autogenerates an `allow-<command>` permission
//! for each one, and once an application manifest exists, a command
//! that no capability grants is rejected at the IPC boundary instead of
//! being callable by default.

/// Kept in step with `src/commands.rs`; `tests/capability_audit.rs`
/// proves the two agree.
const COMMANDS: &[&str] = &[
    "list_sessions",
    "search_sessions",
    "settings_summary",
    "recording_status",
];

fn main() {
    let attributes = tauri_build::Attributes::new()
        .app_manifest(tauri_build::AppManifest::new().commands(COMMANDS));
    if let Err(error) = tauri_build::try_build(attributes) {
        println!("cargo::error=tauri-build failed: {error}");
        std::process::exit(1);
    }
}
