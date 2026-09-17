// Copyright 2026 Mathews Tom
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//     https://www.apache.org/licenses/LICENSE-2.0

//! How much room the filesystem holding a directory has left.
//!
//! Reported as `Option`, and never as a number the caller cannot tell
//! apart from a real one. A platform that will not answer returns
//! `None`, and the preflight says the figure is unknown rather than
//! claiming the disk is empty or full — the first would let a download
//! start that cannot finish, the second would block one that could.

use std::path::Path;

/// Bytes available to an unprivileged writer under `directory`.
///
/// `None` when the platform does not answer, which includes a
/// directory that does not exist yet: the caller creates it and asks
/// again.
#[must_use]
pub fn available_bytes(directory: &Path) -> Option<u64> {
    platform_available_bytes(directory)
}

// `statvfs`'s field widths differ by platform — `f_bavail` is 32-bit on
// Darwin and 64-bit on Linux, and `f_frsize` likewise — so one of these
// conversions is a widening and the other an identity depending on where
// this compiles. Writing it for one target would not build on the other.
#[cfg(unix)]
#[allow(clippy::useless_conversion, clippy::unnecessary_fallible_conversions)]
fn platform_available_bytes(directory: &Path) -> Option<u64> {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;

    let path = CString::new(directory.as_os_str().as_bytes()).ok()?;
    // SAFETY: `statvfs` reads filesystem statistics into a caller-owned
    // struct and mutates nothing else. The buffer is fully initialised
    // by the call on success, and the result is only read on success.
    #[allow(unsafe_code)]
    let stats = unsafe {
        let mut stats = std::mem::MaybeUninit::<libc::statvfs>::zeroed();
        if libc::statvfs(path.as_ptr(), stats.as_mut_ptr()) != 0 {
            return None;
        }
        stats.assume_init()
    };
    // `f_frsize` is the fragment size blocks are counted in; `f_bsize`
    // is the preferred I/O size and is not what `f_bavail` multiplies
    // by. Some platforms leave `f_frsize` zero, in which case the two
    // are documented to coincide.
    let block = if stats.f_frsize == 0 {
        stats.f_bsize
    } else {
        stats.f_frsize
    };
    u64::try_from(block)
        .ok()?
        .checked_mul(u64::try_from(stats.f_bavail).ok()?)
}

#[cfg(windows)]
fn platform_available_bytes(directory: &Path) -> Option<u64> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::GetDiskFreeSpaceExW;

    let wide: Vec<u16> = directory.as_os_str().encode_wide().chain(Some(0)).collect();
    let mut available: u64 = 0;
    // SAFETY: the name is a NUL-terminated wide string that outlives
    // the call, the out-parameter is a caller-owned `u64`, and the two
    // unused out-parameters are passed as null, which the API accepts.
    #[allow(unsafe_code)]
    let ok = unsafe {
        GetDiskFreeSpaceExW(
            wide.as_ptr(),
            std::ptr::addr_of_mut!(available),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
        )
    };
    (ok != 0).then_some(available)
}

#[cfg(not(any(unix, windows)))]
const fn platform_available_bytes(_directory: &Path) -> Option<u64> {
    None
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    #[test]
    fn test_a_real_directory_reports_some_room() {
        let dir = tempfile::tempdir().unwrap();

        let reported = available_bytes(dir.path());

        assert!(
            reported.is_some_and(|bytes| bytes > 0),
            "the temporary filesystem reported {reported:?} bytes available"
        );
    }

    #[test]
    fn test_a_directory_that_does_not_exist_reports_nothing_rather_than_zero() {
        let dir = tempfile::tempdir().unwrap();

        assert_eq!(available_bytes(&dir.path().join("absent")), None);
    }
}
