// Copyright 2026 Mathews Tom
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//     https://www.apache.org/licenses/LICENSE-2.0

//! Session-folder layout: name construction (date-time-title-ULID) and
//! the per-session pid lockfile that prevents folder-collision when
//! two `scrybe record` invocations land in the same minute.
//!
//! Layout invariants are documented in `docs/system-design.md` §8.3.

use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};

use crate::error::StorageError;
use crate::types::SessionId;

/// File name of the per-session pid lock written under the session folder.
pub const PID_LOCK_NAME: &str = "pid.lock";

/// Generated `.stignore` so Syncthing leaves the folder alone until
/// the lock is released.
pub const STIGNORE_NAME: &str = ".stignore";

/// Construct the session-folder name `YYYY-MM-DD-HHMM-<title>-<ULID>`.
/// `title` is sanitized to ASCII-alphanumeric + hyphen so the path
/// is safe across all four target platforms.
#[must_use]
pub fn session_folder_name(started_at: DateTime<Utc>, title: &str, id: SessionId) -> String {
    let stamp = started_at.format("%Y-%m-%d-%H%M");
    let slug = sanitize_title(title);
    let suffix = id.to_string_26();
    if slug.is_empty() {
        format!("{stamp}-{suffix}")
    } else {
        format!("{stamp}-{slug}-{suffix}")
    }
}

/// Lower-case, hyphen-separated alphanumerics. Empty input collapses
/// to an empty string. Length capped at 60 so the full folder name
/// stays under typical filesystem limits.
fn sanitize_title(title: &str) -> String {
    let mut out = String::with_capacity(title.len());
    let mut last_was_dash = true;
    for ch in title.chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch.to_ascii_lowercase());
            last_was_dash = false;
        } else if !last_was_dash {
            out.push('-');
            last_was_dash = true;
        }
    }
    if out.ends_with('-') {
        out.pop();
    }
    if out.len() > 60 {
        out.truncate(60);
        if out.ends_with('-') {
            out.pop();
        }
    }
    out
}

/// Acquire the per-session lockfile via `O_CREAT | O_EXCL`.
///
/// Returns `StorageError::SessionLocked` if the file already exists,
/// with the holder's pid for diagnostic display. The caller is
/// responsible for dropping the file handle and removing the lockfile
/// on clean shutdown — see [`release_session_lock`].
///
/// # Errors
///
/// `StorageError::SessionLocked` if the file already exists,
/// `StorageError::Io` for any other I/O failure (e.g. permission denied,
/// parent directory missing).
pub fn acquire_session_lock(folder: &Path, owner_pid: u32) -> Result<PathBuf, StorageError> {
    let lock_path = folder.join(PID_LOCK_NAME);
    let mut handle = match OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&lock_path)
    {
        Ok(file) => file,
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
            let existing_pid = read_pid(&lock_path).unwrap_or(0);
            return Err(StorageError::SessionLocked {
                pid: existing_pid,
                path: lock_path,
            });
        }
        Err(e) => return Err(StorageError::Io(e)),
    };
    writeln!(handle, "{owner_pid}").map_err(StorageError::Io)?;
    Ok(lock_path)
}

/// Release the per-session lock. Idempotent — a missing file is not an
/// error, but other I/O failures (permission) are reported.
///
/// # Errors
///
/// `StorageError::Io` for non-`NotFound` filesystem errors.
pub fn release_session_lock(lock_path: &Path) -> Result<(), StorageError> {
    match std::fs::remove_file(lock_path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(StorageError::Io(e)),
    }
}

fn read_pid(path: &Path) -> Option<u32> {
    let text = std::fs::read_to_string(path).ok()?;
    text.trim().parse().ok()
}

/// Write a default `.stignore` template so Syncthing leaves the
/// session folder alone until the run finishes.
///
/// # Errors
///
/// `StorageError::Io` for filesystem errors.
pub fn write_stignore_template(folder: &Path) -> Result<(), StorageError> {
    let path = folder.join(STIGNORE_NAME);
    std::fs::write(
        &path,
        b"# scrybe-managed: ignore everything in this folder until the\n\
          # session ends and pid.lock is removed. Re-enable sync by\n\
          # deleting this file or removing the matching pattern in\n\
          # your top-level Syncthing .stignore.\n\
          *\n",
    )
    .map_err(StorageError::Io)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use chrono::TimeZone;
    use pretty_assertions::assert_eq;

    fn dt(year: i32, month: u32, day: u32, hour: u32, minute: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(year, month, day, hour, minute, 0)
            .unwrap()
    }

    #[test]
    fn test_session_folder_name_uses_iso_date_minute_title_and_ulid_suffix() {
        let id = SessionId::new();
        let suffix = id.to_string_26();

        let name = session_folder_name(dt(2026, 4, 29, 14, 30), "Acme Discovery", id);

        assert!(name.starts_with("2026-04-29-1430-acme-discovery-"));
        assert!(name.ends_with(&suffix));
    }

    #[test]
    fn test_session_folder_name_for_blank_title_omits_slug_segment() {
        let id = SessionId::new();
        let suffix = id.to_string_26();

        let name = session_folder_name(dt(2026, 4, 29, 14, 30), "", id);

        assert_eq!(name, format!("2026-04-29-1430-{suffix}"));
    }

    #[test]
    fn test_session_folder_name_collapses_runs_of_punctuation_to_single_dashes() {
        let id = SessionId::new();

        let name = session_folder_name(dt(2026, 4, 29, 14, 30), "  Acme!! / discovery??  ", id);

        assert!(name.contains("-acme-discovery-"));
        assert!(!name.contains("--"));
    }

    #[test]
    fn test_session_folder_name_truncates_long_titles_to_keep_path_safe() {
        let id = SessionId::new();
        let long_title: String = "x".repeat(200);

        let name = session_folder_name(dt(2026, 4, 29, 14, 30), &long_title, id);

        let slug_len = name.len() - "2026-04-29-1430--".len() - id.to_string_26().len();
        assert!(slug_len <= 60, "slug not truncated: {slug_len}");
    }

    #[test]
    fn test_acquire_session_lock_creates_pid_lock_file_with_caller_pid() {
        let dir = tempfile::tempdir().unwrap();

        let lock = acquire_session_lock(dir.path(), 4242).unwrap();

        let body = std::fs::read_to_string(&lock).unwrap();
        assert_eq!(body.trim(), "4242");
        assert_eq!(lock, dir.path().join(PID_LOCK_NAME));
    }

    #[test]
    fn test_acquire_session_lock_second_call_returns_session_locked_error() {
        let dir = tempfile::tempdir().unwrap();
        let _first = acquire_session_lock(dir.path(), 1234).unwrap();

        let err = acquire_session_lock(dir.path(), 5678).unwrap_err();

        match err {
            StorageError::SessionLocked { pid, path } => {
                assert_eq!(pid, 1234);
                assert_eq!(path, dir.path().join(PID_LOCK_NAME));
            }
            other => panic!("expected SessionLocked, got {other:?}"),
        }
    }

    #[test]
    fn test_release_session_lock_removes_lockfile_so_next_acquire_succeeds() {
        let dir = tempfile::tempdir().unwrap();
        let lock = acquire_session_lock(dir.path(), 1234).unwrap();

        release_session_lock(&lock).unwrap();
        let second = acquire_session_lock(dir.path(), 5678).unwrap();

        let body = std::fs::read_to_string(&second).unwrap();
        assert_eq!(body.trim(), "5678");
    }

    #[test]
    fn test_release_session_lock_is_idempotent_when_lock_missing() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(PID_LOCK_NAME);

        release_session_lock(&path).unwrap();
        release_session_lock(&path).unwrap();
    }

    #[test]
    fn test_write_stignore_template_creates_default_stignore_file() {
        let dir = tempfile::tempdir().unwrap();

        write_stignore_template(dir.path()).unwrap();

        let body = std::fs::read_to_string(dir.path().join(STIGNORE_NAME)).unwrap();
        assert!(body.contains("scrybe-managed"));
        assert!(body.contains('*'));
    }
}
