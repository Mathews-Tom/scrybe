// Copyright 2026 Mathews Tom
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//     https://www.apache.org/licenses/LICENSE-2.0

//! The read-only filesystem capability the agent-access dispatch layer
//! is built on.
//!
//! [`ReadOnlyFs`] is the only way anything under `agent_access` touches
//! disk. Every method returns owned data (`String`, `Vec<String>`,
//! `bool`) instead of a writable handle, so there is no method a tool
//! handler could call to create, modify, rename, or delete anything —
//! mutation is unrepresentable through this trait regardless of what
//! code is written against it. `protocol::tests::dispatch_never_mutates_fixture_tree`
//! proves this at runtime by hashing a fixture tree before and after
//! exercising every served method.

use std::io;
use std::path::Path;

/// Capability exposing exactly the filesystem reads the agent-access
/// reader needs.
///
/// See the module docs for why this makes mutation unrepresentable
/// rather than merely unused.
pub trait ReadOnlyFs: Send + Sync {
    /// Names (not full paths) of the direct subdirectories of `dir`,
    /// in the order the filesystem reports them.
    ///
    /// Non-directory entries are omitted.
    ///
    /// # Errors
    ///
    /// Returns the underlying `io::Error` if `dir` cannot be read
    /// (missing, not a directory, permission denied).
    fn subdirectories(&self, dir: &Path) -> io::Result<Vec<String>>;

    /// Whether `path` exists at all (file or directory).
    fn exists(&self, path: &Path) -> bool;

    /// Reads the full contents of the file at `path` as UTF-8 text.
    ///
    /// # Errors
    ///
    /// Returns the underlying `io::Error` if `path` cannot be read
    /// (missing, not UTF-8, permission denied).
    fn read_to_string(&self, path: &Path) -> io::Result<String>;
}

/// Production [`ReadOnlyFs`] backed directly by `std::fs`.
///
/// Every method here calls only a read-oriented `std::fs` function —
/// `read_dir`, `Path::exists`, `read_to_string` — never `write`,
/// `File::create`, `remove_file`, `remove_dir_all`, or `rename`.
#[derive(Clone, Copy, Debug, Default)]
pub struct RealReadOnlyFs;

impl ReadOnlyFs for RealReadOnlyFs {
    fn subdirectories(&self, dir: &Path) -> io::Result<Vec<String>> {
        let mut names = Vec::new();
        for entry in std::fs::read_dir(dir)? {
            let entry = entry?;
            if entry.file_type()?.is_dir() {
                names.push(entry.file_name().to_string_lossy().into_owned());
            }
        }
        Ok(names)
    }

    fn exists(&self, path: &Path) -> bool {
        path.exists()
    }

    fn read_to_string(&self, path: &Path) -> io::Result<String> {
        std::fs::read_to_string(path)
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    #[test]
    fn test_subdirectories_lists_only_directories_not_files() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("session-a")).unwrap();
        std::fs::create_dir(dir.path().join("session-b")).unwrap();
        std::fs::write(dir.path().join("stray-file.txt"), b"not a session").unwrap();

        let mut names = RealReadOnlyFs.subdirectories(dir.path()).unwrap();
        names.sort();

        assert_eq!(names, vec!["session-a", "session-b"]);
    }

    #[test]
    fn test_subdirectories_errors_when_dir_missing() {
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("does-not-exist");

        let err = RealReadOnlyFs.subdirectories(&missing).unwrap_err();

        assert_eq!(err.kind(), io::ErrorKind::NotFound);
    }

    #[test]
    fn test_exists_reports_true_for_present_path_and_false_for_absent() {
        let dir = tempfile::tempdir().unwrap();
        let present = dir.path().join("meta.toml");
        std::fs::write(&present, b"session_id = \"x\"").unwrap();

        assert!(RealReadOnlyFs.exists(&present));
        assert!(!RealReadOnlyFs.exists(&dir.path().join("absent.toml")));
    }

    #[test]
    fn test_read_to_string_returns_file_contents() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("notes.md");
        std::fs::write(&path, "# Notes\n\nSummary.\n").unwrap();

        let body = RealReadOnlyFs.read_to_string(&path).unwrap();

        assert_eq!(body, "# Notes\n\nSummary.\n");
    }

    #[test]
    fn test_read_to_string_errors_when_file_missing() {
        let dir = tempfile::tempdir().unwrap();

        let err = RealReadOnlyFs
            .read_to_string(&dir.path().join("missing.md"))
            .unwrap_err();

        assert_eq!(err.kind(), io::ErrorKind::NotFound);
    }
}
