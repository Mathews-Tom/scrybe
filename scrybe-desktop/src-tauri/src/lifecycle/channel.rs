// Copyright 2026 Mathews Tom
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//     https://www.apache.org/licenses/LICENSE-2.0

//! A debug-build record of what the process did.
//!
//! Apple ships no standalone `WKWebView` driver, so no `WebDriver`
//! client can observe this application's window and tray on macOS. The
//! qualification harness reads this file instead, alongside
//! operating-system facts it can establish without one: process counts
//! for ownership and `NSRunningApplication` for activation and exit.
//!
//! The entire module is `#[cfg(debug_assertions)]`, and the
//! [`note`](crate::note) macro expands to nothing in a release build,
//! so neither the writer nor any of the event strings reaches release
//! codegen. That is checked rather than assumed: the qualification
//! harness builds the release binary and asserts these event names are
//! absent from it.
//!
//! The file lives inside the configured storage root, which is the
//! directory a hermetic run makes disposable. A run therefore leaves
//! nothing behind outside the root it was given.

use std::fmt::Write as _;
use std::io::Write as _;
use std::path::Path;

use tauri::Manager as _;

use crate::state::Desktop;

/// The record's name inside the storage root.
///
/// A leading dot keeps it out of the way; the session scan only
/// considers directories, so it is never mistaken for a session.
pub const FILE_NAME: &str = ".desktop-lifecycle.jsonl";

/// Appends one observation, as a single JSON object on its own line.
///
/// Append-only and line-delimited so a reader can consume a partially
/// written file without coordination, and so the order of observations
/// is the order they happened.
///
/// A failure here is reported and does not propagate: this is
/// instrumentation, and a debug build that cannot write its own record
/// should still run.
pub fn append(app: &tauri::AppHandle, event: &str, detail: Option<&str>) {
    let root = app
        .state::<Desktop>()
        .application()
        .root()
        .path()
        .to_owned();
    if let Err(error) = write_line(&root, event, detail) {
        eprintln!("scrybe-desktop: could not record `{event}` to the lifecycle channel: {error}");
    }
}

fn write_line(root: &Path, event: &str, detail: Option<&str>) -> std::io::Result<()> {
    std::fs::create_dir_all(root)?;
    let mut line = String::new();
    let _ = write!(
        line,
        r#"{{"event":{},"pid":{}"#,
        quote(event),
        std::process::id()
    );
    if let Some(detail) = detail {
        let _ = write!(line, r#","detail":{}"#, quote(detail));
    }
    line.push_str("}\n");

    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(root.join(FILE_NAME))?;
    file.write_all(line.as_bytes())
}

/// The minimal JSON string escape. The events this module writes are
/// fixed identifiers and short details, never user text, so escaping
/// the two characters JSON forbids unescaped is sufficient and keeps a
/// serializer off the instrumentation path.
fn quote(value: &str) -> String {
    let escaped = value.replace('\\', r"\\").replace('"', "\\\"");
    format!("\"{escaped}\"")
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    #[test]
    fn test_each_observation_is_one_self_contained_json_line() {
        let directory = tempfile::tempdir().unwrap();
        write_line(directory.path(), "window-shown", None).unwrap();
        write_line(directory.path(), "quit-refused", Some("recording")).unwrap();

        let recorded = std::fs::read_to_string(directory.path().join(FILE_NAME)).unwrap();
        let lines: Vec<&str> = recorded.lines().collect();

        assert_eq!(lines.len(), 2);
        let first: serde_json::Value = serde_json::from_str(lines[0]).unwrap();
        let second: serde_json::Value = serde_json::from_str(lines[1]).unwrap();
        assert_eq!(first["event"], "window-shown");
        assert_eq!(first["pid"], std::process::id());
        assert_eq!(second["detail"], "recording");
    }

    #[test]
    fn test_a_detail_containing_json_punctuation_stays_one_readable_line() {
        let directory = tempfile::tempdir().unwrap();
        write_line(
            directory.path(),
            "quit-refused",
            Some(r#"a "quoted" \ detail"#),
        )
        .unwrap();

        let recorded = std::fs::read_to_string(directory.path().join(FILE_NAME)).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(recorded.trim()).unwrap();

        assert_eq!(recorded.lines().count(), 1);
        assert_eq!(parsed["detail"], r#"a "quoted" \ detail"#);
    }
}
