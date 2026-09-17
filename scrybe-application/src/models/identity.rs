// Copyright 2026 Mathews Tom
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//     https://www.apache.org/licenses/LICENSE-2.0

//! Enough of a file's metadata to say it is still the same file,
//! unchanged.
//!
//! Used to decide whether a digest already taken still describes what
//! is on disk. Length and modification time — what a build system keys
//! on — are not enough for that here. A build system keys inputs it
//! produced itself; this keys an artifact any other process may
//! rewrite, and preserving a modification time across a rewrite is
//! ordinary rather than adversarial: `rsync -t`, `cp -p`, `tar -x`,
//! `unzip` and a bare `utimes` all do it.
//!
//! So the identity of the file is carried too. On Unix that is the
//! device and inode numbers, which change when a destination is
//! replaced by a rename, together with the change time, which moves
//! whenever the inode is written at all and which no userspace
//! interface can set — only altering the clock or writing the
//! filesystem raw does.
//!
//! [`FileIdentity::of`] answers `None` rather than a partial identity
//! when the platform will not supply one, and the caller then hashes
//! every time instead of memoising. A partial key would be worse than
//! no cache at all: two files it could not separate would compare
//! equal and answer for each other, which is the defect this type
//! exists to close.
//!
//! Windows is such a platform today. The three fields it would need —
//! `volume_serial_number`, `file_index`, and `change_time` — are all
//! still unstable in the standard library on the pinned toolchain, and
//! the fields that are stable (`creation_time`, `last_write_time`) are
//! settable through `SetFileTime` and so carry exactly the weakness
//! described above. Reaching for `GetFileInformationByHandle` directly
//! would buy the memo back at the price of an `unsafe` block on a
//! platform that does not ship the desktop host the memo exists for,
//! so this answers `None` there until one of those is no longer true.

/// What a file looked like at the moment it was read.
///
/// Compared for equality and nothing else; the individual fields are
/// deliberately not exposed, because no caller has a use for one of
/// them on its own.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FileIdentity {
    len: u64,
    device: u64,
    file: u64,
    /// When the contents were last written, in whatever resolution the
    /// platform keeps.
    modified: (i64, i64),
    /// When the inode itself was last written.
    changed: (i64, i64),
}

impl FileIdentity {
    /// What `metadata` says the file is, or `None` when the platform
    /// will not say.
    #[must_use]
    pub fn of(metadata: &std::fs::Metadata) -> Option<Self> {
        platform_identity(metadata)
    }
}

// Infallible here and infallible-looking to clippy, but the signature
// is one platforms share: the arm below cannot answer at all, and
// `FileIdentity::of` is the caller's single entry point either way.
#[cfg(unix)]
#[allow(clippy::unnecessary_wraps)]
fn platform_identity(metadata: &std::fs::Metadata) -> Option<FileIdentity> {
    use std::os::unix::fs::MetadataExt as _;

    Some(FileIdentity {
        len: metadata.len(),
        device: metadata.dev(),
        file: metadata.ino(),
        modified: (metadata.mtime(), metadata.mtime_nsec()),
        changed: (metadata.ctime(), metadata.ctime_nsec()),
    })
}

// See the module documentation: every field that would make this an
// identity rather than a guess is unstable on the pinned toolchain.
#[cfg(not(unix))]
const fn platform_identity(_metadata: &std::fs::Metadata) -> Option<FileIdentity> {
    None
}
