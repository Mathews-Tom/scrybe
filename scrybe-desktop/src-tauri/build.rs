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

/// Kept in step with `src/commands.rs` and `src/setup.rs`;
/// `tests/capability_audit.rs` proves the three agree.
///
/// A build script cannot import the crate it builds, so this list is
/// restated rather than borrowed. What keeps it honest is the audit:
/// a command registered without an entry here has no generated
/// permission, and a capability file granting one that is not here
/// fails the audit.
const COMMANDS: &[&str] = &[
    "list_sessions",
    "search_sessions",
    "cancel_query",
    "get_session",
    "read_notes",
    "read_transcript_page",
    "repair_session",
    "regenerate_notes",
    "reveal_session",
    "delete_session",
    "archive_session",
    "copy_notes",
    "copy_transcript",
    "settings_summary",
    "recording_status",
    "recording_preflight",
    "start_recording",
    "stop_recording",
    "acknowledge_recording",
    "settings_form",
    "apply_settings",
    "diagnostics_report",
    "apply_recovery",
    "readiness_report",
    "model_offer",
    "install_model",
    "cancel_model_install",
    "open_system_settings",
    "open_advanced_configuration",
];

fn main() {
    let attributes = tauri_build::Attributes::new()
        .app_manifest(tauri_build::AppManifest::new().commands(COMMANDS));
    if let Err(error) = tauri_build::try_build(attributes) {
        println!("cargo::error=tauri-build failed: {error}");
        std::process::exit(1);
    }
}
