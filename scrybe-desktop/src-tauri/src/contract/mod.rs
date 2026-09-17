// Copyright 2026 Mathews Tom
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//     https://www.apache.org/licenses/LICENSE-2.0

//! The transport contract, and the TypeScript the frontend reads it
//! through.
//!
//! Every type here is a projection of a `scrybe-application` contract,
//! narrowed to what one view renders. The narrowing is the point: the
//! service layer's types are shared with the CLI and the agent surface
//! and carry more than a `WebView` should see, and a projection is the
//! one place that difference is written down.
//!
//! [`render`] emits the TypeScript declarations for all of them.
//! `src/generated/bindings.ts` is that output, checked in;
//! [`tests::test_checked_in_bindings_match_the_rust_contract`] fails
//! when the two disagree, so a contract change that skips regeneration
//! cannot reach a merge.

pub mod error;
pub mod recording;
pub mod session;
pub mod settings;

use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use ts_rs::{Config, TS};

pub use error::{CommandFailure, FailureCode};
pub use recording::{RecordingState, RecordingStatus, RecordingTransition, TRANSITION_EVENT};
pub use session::{SessionProgress, SessionRow, SessionRows};
pub use settings::{SettingsSummary, SettingsWarning, WarningSeverity};

const HEADER: &str = "\
// Generated from `src-tauri/src/contract` — do not edit by hand.
//
// Regenerate with `pnpm --dir scrybe-desktop run bindings`. Verify with
// `pnpm --dir scrybe-desktop run check:bindings`, which fails when this
// file and the Rust contract disagree.
//
// Declarations appear in dependency order, so the file reads top to
// bottom and the drift check compares a stable byte sequence rather
// than whatever order a hash map happened to produce.
";

/// Emits every transport declaration, in dependency order.
///
/// Order is explicit rather than derived so the emitted file is stable
/// across compiler and hash-map iteration changes; a generator whose
/// output reorders itself would make the drift check useless.
macro_rules! contract {
    ($($ty:ty),+ $(,)?) => {
        fn declarations() -> Vec<String> {
            let config = Config::default();
            vec![$(format!("export {}", <$ty as TS>::decl(&config))),+]
        }
    };
}

contract![
    FailureCode,
    CommandFailure,
    SessionProgress,
    SessionRow,
    SessionRows,
    RecordingState,
    RecordingStatus,
    RecordingTransition,
    WarningSeverity,
    SettingsWarning,
    SettingsSummary,
];

/// The complete `bindings.ts` file content.
#[must_use]
pub fn render() -> String {
    let mut out = String::from(HEADER);
    for declaration in declarations() {
        out.push('\n');
        out.push_str(&declaration);
        out.push('\n');
    }
    // The event name is emitted as a constant so the frontend
    // subscribes to what the host publishes rather than to a literal it
    // keeps in step by hand.
    let _ = writeln!(
        out,
        "\nexport const RECORDING_TRANSITION_EVENT = \"{TRANSITION_EVENT}\";"
    );
    out
}

/// Where the checked-in bindings live, relative to this crate.
#[must_use]
pub fn bindings_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../src/generated/bindings.ts")
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    #[test]
    fn test_checked_in_bindings_match_the_rust_contract() {
        let path = bindings_path();
        let checked_in = std::fs::read_to_string(&path).unwrap_or_else(|error| {
            panic!("{}: {error}", path.display());
        });
        let expected = render();
        if checked_in == expected {
            return;
        }

        // Reporting the whole file on both sides would bury the change
        // that caused the drift; the first differing line is the one a
        // reader needs.
        let mut observed_lines = checked_in.lines();
        let mut expected_lines = expected.lines();
        let mut line = 0;
        loop {
            line += 1;
            match (expected_lines.next(), observed_lines.next()) {
                (None, None) => break,
                (expected_line, observed_line) if expected_line == observed_line => {}
                (expected_line, observed_line) => panic!(
                    "\n{} is stale at line {line}. \
                     Run `pnpm --dir scrybe-desktop run bindings`.\n  \
                     expected: {expected_line:?}\n  observed: {observed_line:?}\n",
                    path.display(),
                ),
            }
        }
    }

    /// The transport surface is a trust boundary, so its membership is
    /// asserted rather than left to whatever happens to derive `TS`.
    /// Widening it is a deliberate edit here, reviewed as such.
    #[test]
    fn test_the_transport_surface_is_exactly_the_declared_set() {
        let rendered = render();
        let exported: Vec<&str> = rendered
            .lines()
            .filter_map(|line| line.strip_prefix("export type "))
            .filter_map(|line| line.split_whitespace().next())
            .collect();

        assert_eq!(
            exported,
            [
                "FailureCode",
                "CommandFailure",
                "SessionProgress",
                "SessionRow",
                "SessionRows",
                "RecordingState",
                "RecordingStatus",
                "RecordingTransition",
                "WarningSeverity",
                "SettingsWarning",
                "SettingsSummary",
            ],
        );
    }
}
