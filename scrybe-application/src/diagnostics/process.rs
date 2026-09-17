// Copyright 2026 Mathews Tom
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//     https://www.apache.org/licenses/LICENSE-2.0

//! Whether the process that wrote a session lock is still running.
//!
//! This is the difference between "a meeting is being recorded right
//! now" and "a recorder died and left its lock behind". Every surface
//! that reports on a session folder needs to make that distinction, so
//! it is decided once here rather than per command.

use std::path::Path;

/// Reads `lock_path` and reports whether its process is still alive.
///
/// `None` when the lock cannot be read or does not contain a process
/// identifier — the lock is unusable either way, and the caller decides
/// what an unusable lock means.
#[must_use]
pub fn lock_owner_alive(lock_path: &Path) -> Option<bool> {
    let pid: u32 = std::fs::read_to_string(lock_path)
        .ok()?
        .trim()
        .parse()
        .ok()?;
    Some(is_alive(pid))
}

#[cfg(unix)]
#[allow(clippy::cast_possible_wrap)]
fn is_alive(pid: u32) -> bool {
    // SAFETY: `kill(pid, 0)` sends no signal. It returns 0 when the
    // process exists and is signalable and `ESRCH` otherwise. No
    // process state is mutated and nothing is allocated.
    #[allow(unsafe_code)]
    let rc = unsafe { libc::kill(pid as libc::pid_t, 0) };
    rc == 0
}

#[cfg(windows)]
fn is_alive(pid: u32) -> bool {
    use windows_sys::Win32::Foundation::{
        CloseHandle, GetLastError, ERROR_ACCESS_DENIED, WAIT_OBJECT_0,
    };
    use windows_sys::Win32::System::Threading::{
        OpenProcess, WaitForSingleObject, PROCESS_SYNCHRONIZE,
    };

    // SAFETY: `OpenProcess` returns a null handle on failure, which is
    // checked before use; the handle is closed on every path that
    // obtains one.
    #[allow(unsafe_code)]
    unsafe {
        let handle = OpenProcess(PROCESS_SYNCHRONIZE, 0, pid);
        if handle.is_null() {
            // A process owned by another user exists but cannot be
            // opened; treat that as alive rather than claiming a stale
            // lock.
            return GetLastError() == ERROR_ACCESS_DENIED;
        }
        let alive = WaitForSingleObject(handle, 0) != WAIT_OBJECT_0;
        CloseHandle(handle);
        alive
    }
}

#[cfg(not(any(unix, windows)))]
const fn is_alive(_pid: u32) -> bool {
    // With no way to ask, refusing to call a lock stale is the safe
    // answer: a false "stale" invites deleting a live recording's lock.
    true
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    #[test]
    fn test_a_lock_naming_this_process_reports_its_owner_alive() {
        let dir = tempfile::tempdir().unwrap();
        let lock = dir.path().join("pid.lock");
        std::fs::write(&lock, std::process::id().to_string()).unwrap();

        assert_eq!(lock_owner_alive(&lock), Some(true));
    }

    #[test]
    fn test_an_unparseable_lock_yields_no_verdict_rather_than_a_guess() {
        let dir = tempfile::tempdir().unwrap();
        let lock = dir.path().join("pid.lock");
        std::fs::write(&lock, "not-a-pid").unwrap();

        assert_eq!(lock_owner_alive(&lock), None);
    }

    #[test]
    fn test_an_absent_lock_yields_no_verdict() {
        let dir = tempfile::tempdir().unwrap();

        assert_eq!(lock_owner_alive(&dir.path().join("pid.lock")), None);
    }
}
