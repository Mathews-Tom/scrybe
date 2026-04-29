// Copyright 2026 Mathews Tom
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//     https://www.apache.org/licenses/LICENSE-2.0

//! Atomic-replace and durable-append primitives implementing the
//! recipe in `docs/system-design.md` §8.3.
//!
//! The recipe is asymmetric on purpose:
//!
//! - macOS uses `fcntl(F_FULLFSYNC)` because `fsync(2)` only commits to
//!   drive cache (not platter) on Apple SSDs.
//! - Windows uses `MoveFileExW(MOVEFILE_REPLACE_EXISTING |
//!   MOVEFILE_WRITE_THROUGH)`. `std::fs::rename` does not guarantee
//!   atomicity on rename-to-existing, and a directory `fsync` is not
//!   portable on Windows (`std::fs::File::open` cannot open directory
//!   handles without `FILE_FLAG_BACKUP_SEMANTICS`).
//! - Other Unix platforms call `sync_all` on the file and on the
//!   parent directory after the in-filesystem rename for power-loss
//!   durability.

use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::Path;

use crate::error::StorageError;

/// Durably commit a file's contents. On macOS uses `fcntl(F_FULLFSYNC)`;
/// elsewhere falls back to `sync_all` (`fsync(2)` on Unix,
/// `FlushFileBuffers` on Windows).
///
/// # Errors
///
/// Returns `std::io::Error` if the OS reports a sync failure.
#[cfg(target_os = "macos")]
pub fn full_fsync(file: &File) -> std::io::Result<()> {
    use std::os::unix::io::AsRawFd;

    #[allow(unsafe_code)]
    let rc = unsafe { libc::fcntl(file.as_raw_fd(), libc::F_FULLFSYNC) };
    if rc == -1 {
        Err(std::io::Error::last_os_error())
    } else {
        Ok(())
    }
}

/// Durably commit a file's contents.
///
/// # Errors
///
/// Returns `std::io::Error` if the OS reports a sync failure.
#[cfg(all(unix, not(target_os = "macos")))]
pub fn full_fsync(file: &File) -> std::io::Result<()> {
    file.sync_all()
}

/// Durably commit a file's contents.
///
/// # Errors
///
/// Returns `std::io::Error` if the OS reports a sync failure.
#[cfg(target_os = "windows")]
pub fn full_fsync(file: &File) -> std::io::Result<()> {
    file.sync_all()
}

/// Replace `target` with `payload` atomically. Either the previous
/// contents or the new contents are visible after a crash; never a
/// partial write.
///
/// # Errors
///
/// Returns `StorageError::InvalidTargetPath` if `target` has no parent
/// directory, `StorageError::Persist` if the temp file's atomic-rename
/// step fails on Unix, `StorageError::AtomicRename` if `MoveFileExW`
/// fails on Windows, or `StorageError::Io` for any underlying I/O
/// failure.
pub fn atomic_replace(target: &Path, payload: &[u8]) -> Result<(), StorageError> {
    let dir = target
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .ok_or_else(|| StorageError::InvalidTargetPath {
            path: target.to_owned(),
        })?;

    let mut tmp = tempfile::NamedTempFile::new_in(dir)?;
    tmp.write_all(payload)?;
    full_fsync(tmp.as_file())?;

    #[cfg(target_os = "windows")]
    {
        use std::os::windows::ffi::OsStrExt;
        use windows_sys::Win32::Storage::FileSystem::{
            MoveFileExW, MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH,
        };

        let (_persisted_file, persisted_path) = tmp.keep().map_err(|e| StorageError::Persist {
            path: target.to_owned(),
            source: e.error,
        })?;

        let src: Vec<u16> = persisted_path
            .as_os_str()
            .encode_wide()
            .chain(Some(0))
            .collect();
        let dst: Vec<u16> = target.as_os_str().encode_wide().chain(Some(0)).collect();

        #[allow(unsafe_code)]
        let ok = unsafe {
            MoveFileExW(
                src.as_ptr(),
                dst.as_ptr(),
                MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
            )
        };
        if ok == 0 {
            let err = std::io::Error::last_os_error();
            let _ = std::fs::remove_file(&persisted_path);
            return Err(StorageError::AtomicRename {
                path: target.to_owned(),
                source: err,
            });
        }
        // MOVEFILE_WRITE_THROUGH durably commits the rename; no
        // separate fsync(dir) — opening a directory handle on Windows
        // requires FILE_FLAG_BACKUP_SEMANTICS, which std::fs::File::open
        // does not set.
    }

    #[cfg(unix)]
    {
        tmp.persist(target)
            .map_err(|e| StorageError::AtomicRename {
                path: target.to_owned(),
                source: e.error,
            })?;
        let dir_handle = File::open(dir)?;
        full_fsync(&dir_handle)?;
    }

    Ok(())
}

/// Append `line` to `target`, creating the file if absent, and
/// durably commit before returning. Used by `transcript.md`'s
/// chunk-boundary append loop.
///
/// # Errors
///
/// Returns `StorageError::Io` if the underlying append or sync fails.
pub fn append_durable(target: &Path, line: &[u8]) -> Result<(), StorageError> {
    let mut f = OpenOptions::new().append(true).create(true).open(target)?;
    f.write_all(line)?;
    full_fsync(&f)?;
    Ok(())
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use std::fs;
    use std::path::PathBuf;

    use super::*;
    use pretty_assertions::assert_eq;

    fn read_to_string(path: &Path) -> String {
        fs::read_to_string(path).unwrap()
    }

    #[test]
    fn test_atomic_replace_writes_payload_to_target() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("notes.md");

        atomic_replace(&target, b"hello world\n").unwrap();

        assert_eq!(read_to_string(&target), "hello world\n");
    }

    #[test]
    fn test_atomic_replace_overwrites_existing_target() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("meta.toml");
        fs::write(&target, b"version = 0").unwrap();

        atomic_replace(&target, b"version = 1").unwrap();

        assert_eq!(read_to_string(&target), "version = 1");
    }

    #[test]
    fn test_atomic_replace_leaves_no_temp_files_in_directory_on_success() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("notes.md");

        atomic_replace(&target, b"final").unwrap();

        let entries: Vec<_> = fs::read_dir(dir.path())
            .unwrap()
            .filter_map(Result::ok)
            .map(|e| e.file_name())
            .collect();
        assert_eq!(
            entries.len(),
            1,
            "expected only the target file: {entries:?}"
        );
        assert_eq!(entries[0], "notes.md");
    }

    #[test]
    fn test_atomic_replace_rejects_target_without_parent() {
        let target = PathBuf::from("notes.md");

        let err = atomic_replace(&target, b"x").unwrap_err();

        assert!(matches!(err, StorageError::InvalidTargetPath { .. }));
    }

    #[test]
    fn test_atomic_replace_handles_empty_payload() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("empty.txt");

        atomic_replace(&target, b"").unwrap();

        assert_eq!(read_to_string(&target), "");
    }

    #[test]
    fn test_atomic_replace_handles_large_payload() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("audio.bin");
        let payload: Vec<u8> = (0..256_000)
            .map(|i| u8::try_from(i & 0xff).unwrap_or(0))
            .collect();

        atomic_replace(&target, &payload).unwrap();

        let read_back = fs::read(&target).unwrap();
        assert_eq!(read_back.len(), payload.len());
        assert_eq!(read_back, payload);
    }

    #[test]
    fn test_append_durable_creates_file_when_missing() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("transcript.md");

        append_durable(&target, b"first line\n").unwrap();

        assert_eq!(read_to_string(&target), "first line\n");
    }

    #[test]
    fn test_append_durable_appends_to_existing_file_preserving_prior_content() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("transcript.md");
        append_durable(&target, b"first\n").unwrap();

        append_durable(&target, b"second\n").unwrap();
        append_durable(&target, b"third\n").unwrap();

        assert_eq!(read_to_string(&target), "first\nsecond\nthird\n");
    }

    #[test]
    fn test_full_fsync_succeeds_on_recently_written_temp_file() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("durable.bin");
        let mut f = OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(&target)
            .unwrap();
        f.write_all(b"abc").unwrap();

        let result = full_fsync(&f);

        assert!(result.is_ok(), "fsync failed: {result:?}");
    }

    #[test]
    fn test_atomic_replace_persists_across_three_consecutive_calls() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("meta.toml");

        for i in 0..3 {
            let payload = format!("version = {i}\n");
            atomic_replace(&target, payload.as_bytes()).unwrap();
            assert_eq!(read_to_string(&target), payload);
        }
    }
}
