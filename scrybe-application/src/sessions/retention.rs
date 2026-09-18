// Copyright 2026 Mathews Tom
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//     https://www.apache.org/licenses/LICENSE-2.0

//! Moving a session out of the listing, and removing it later.
//!
//! Two destinations, both direct children of the configured storage
//! root: `trash/`, which is swept once a session has been there longer
//! than the configured window, and `archive/`, which is never swept.
//! Both are ordinary directory moves, so the files stay readable by
//! hand, by the command-line tool, and by a backup.
//!
//! The sweep is the only irreversible path in the application, and it
//! acts on a directory a reader can also write to themselves. Two
//! independent conditions therefore gate every removal:
//!
//! 1. The retention index at `trash/retention.toml` records that *this
//!    application* moved that folder, and when. A directory a reader
//!    dropped into `trash/` by hand has no entry, so the sweep has no
//!    reading of its age and cannot remove it.
//! 2. The directory still classifies as a session. A folder whose
//!    contents no longer look like a session is left alone rather than
//!    removed on the strength of a stale index entry.
//!
//! Neither condition is sufficient alone, and the index is what makes
//! "only what the application moved" structural instead of a rule
//! somebody has to remember.
//!
//! The sweep takes the current instant as an argument. Nothing here
//! reads the clock, because a retention window measured in days is
//! otherwise testable only by waiting days.

use std::collections::BTreeMap;
use std::error::Error as StdError;
use std::fmt;
#[cfg(unix)]
use std::fs::File;
use std::fs::OpenOptions;
use std::io::{ErrorKind, Write};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, PoisonError};

use chrono::{DateTime, Utc};
use scrybe_core::storage::{atomic_replace, full_fsync};
use serde::{Deserialize, Serialize};
use ulid::Ulid;

use crate::error::{ApplicationError, ErrorCode};
use crate::identity::{SessionRef, StorageRoot};
use crate::Result;

use super::repository::DirectoryView;
use super::scan::{classify, Classified};

/// The directory a deleted session is moved into.
pub const TRASH_DIR: &str = "trash";

/// The directory an archived session is moved into.
pub const ARCHIVE_DIR: &str = "archive";

/// The index recording which folders this application trashed, and when.
const INDEX_FILE: &str = "retention.toml";

/// Prefix for the random marker binding an index entry to one moved folder.
const ENTRY_MARKER_PREFIX: &str = ".scrybe-trash-entry-";

/// Prefix for an app-owned directory whose permanent removal has begun.
const PURGE_PREFIX: &str = ".scrybe-purge-";

/// Prefix for the durable authorization beside a purge staging directory.
const PURGE_AUTH_PREFIX: &str = ".scrybe-purge-auth-";

/// Whether a direct child of the storage root is a retention directory
/// rather than a session.
///
/// The scan would otherwise examine these like any other directory.
/// `SessionRef::parse` accepts both names — they contain no separator,
/// no traversal and no control character — so they reach classification
/// and are omitted only because they happen to hold no session
/// artifacts at their own top level. A `notes.md` placed in `trash/` by
/// hand would make the trash directory itself appear as an unfinished
/// session.
#[must_use]
pub fn is_reserved_directory(name: &str) -> bool {
    name == TRASH_DIR || name == ARCHIVE_DIR
}

/// Whether `candidate` resolves to either retention directory.
///
/// Exact names cover every filesystem. Canonical identity additionally
/// covers case-insensitive macOS and Windows volumes without hiding a
/// distinct `Trash` session on case-sensitive filesystems.
pub(super) fn is_reserved_path(root: &Path, candidate: &Path, name: &str) -> bool {
    if is_reserved_directory(name) {
        return true;
    }
    let Ok(candidate) = std::fs::canonicalize(candidate) else {
        return false;
    };
    [TRASH_DIR, ARCHIVE_DIR].into_iter().any(|reserved| {
        std::fs::canonicalize(root.join(reserved)).is_ok_and(|path| path == candidate)
    })
}

/// Where a session went, for the message a reader is shown.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Destination {
    /// Moved to `trash/`, and swept once the window passes.
    Trash,
    /// Moved to `archive/`, and never swept.
    Archive,
}

impl Destination {
    /// The directory name this destination writes into.
    #[must_use]
    pub const fn directory(self) -> &'static str {
        match self {
            Self::Trash => TRASH_DIR,
            Self::Archive => ARCHIVE_DIR,
        }
    }
}

/// What one sweep did, so a caller can report it without re-reading the
/// filesystem.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct SweepOutcome {
    /// Folder names removed permanently.
    pub removed: Vec<String>,
    /// Folder names left in place because their window has not passed.
    pub retained: Vec<String>,
    /// Index entries whose folder no longer exists, now forgotten.
    pub forgotten: Vec<String>,
}

impl SweepOutcome {
    /// Whether the sweep changed anything on disk.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.removed.is_empty() && self.forgotten.is_empty()
    }
}

/// One indexed move into the trash.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct TrashEntry {
    trashed_at: DateTime<Utc>,
    retention_days: u32,
    marker: Ulid,
    purging: bool,
}

/// The retention index, mapping a trashed folder name to the move that
/// authorized its later removal.
#[derive(Debug, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Index {
    #[serde(default)]
    trashed: BTreeMap<String, TrashEntry>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum IndexChange {
    Unchanged,
    Dirty,
    Persisted,
}

impl Index {
    fn read(path: &Path) -> Result<Self> {
        let body = match std::fs::read_to_string(path) {
            Ok(body) => body,
            Err(source) if source.kind() == ErrorKind::NotFound => return Ok(Self::default()),
            Err(source) => {
                return Err(ApplicationError::new(
                    ErrorCode::StorageUnavailable,
                    format!(
                        "the retention index at {} could not be read",
                        path.display()
                    ),
                )
                .with_source(source));
            }
        };
        toml::from_str(&body).map_err(|source| {
            ApplicationError::new(
                ErrorCode::StorageUnavailable,
                format!("the retention index at {} is not valid", path.display()),
            )
            .with_source(source)
        })
    }

    fn write(&self, path: &Path) -> Result<()> {
        let body = toml::to_string_pretty(self).map_err(|source| {
            ApplicationError::new(
                ErrorCode::StorageUnavailable,
                "the retention index could not be encoded",
            )
            .with_source(source)
        })?;
        atomic_replace(path, body.as_bytes()).map_err(|source| {
            ApplicationError::new(
                ErrorCode::StorageUnavailable,
                format!(
                    "the retention index at {} could not be written",
                    path.display()
                ),
            )
            .with_source(source)
        })
    }
}

#[cfg(unix)]
fn sync_parent_directory(path: &Path) -> Result<()> {
    let parent = path.parent().ok_or_else(|| {
        ApplicationError::new(
            ErrorCode::StorageUnavailable,
            "a retention marker has no parent directory",
        )
    })?;
    let directory = File::open(parent).map_err(|source| {
        ApplicationError::new(
            ErrorCode::StorageUnavailable,
            format!(
                "{} could not be opened for synchronization",
                parent.display()
            ),
        )
        .with_source(source)
    })?;
    full_fsync(&directory).map_err(|source| {
        ApplicationError::new(
            ErrorCode::StorageUnavailable,
            format!("{} could not be synchronized", parent.display()),
        )
        .with_source(source)
    })
}

#[cfg(not(unix))]
#[allow(clippy::unnecessary_wraps)]
const fn sync_parent_directory(_path: &Path) -> Result<()> {
    Ok(())
}

#[derive(Debug)]
enum DurableRenameError {
    NotMoved(std::io::Error),
    #[cfg(unix)]
    MovedNotDurable(std::io::Error),
}

impl DurableRenameError {
    const fn moved(&self) -> bool {
        #[cfg(unix)]
        {
            matches!(self, Self::MovedNotDurable(_))
        }
        #[cfg(not(unix))]
        {
            match self {
                Self::NotMoved(_) => false,
            }
        }
    }
}

impl fmt::Display for DurableRenameError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotMoved(source) => write!(formatter, "the directory was not moved: {source}"),
            #[cfg(unix)]
            Self::MovedNotDurable(source) => {
                write!(
                    formatter,
                    "the directory moved but could not be synchronized: {source}"
                )
            }
        }
    }
}

#[cfg(target_os = "macos")]
fn rename_noreplace(source: &Path, target: &Path) -> std::io::Result<()> {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt as _;

    let source = CString::new(source.as_os_str().as_bytes())
        .map_err(|error| std::io::Error::new(ErrorKind::InvalidInput, error))?;
    let target = CString::new(target.as_os_str().as_bytes())
        .map_err(|error| std::io::Error::new(ErrorKind::InvalidInput, error))?;
    #[allow(unsafe_code)]
    let renamed = unsafe {
        libc::renameatx_np(
            libc::AT_FDCWD,
            source.as_ptr(),
            libc::AT_FDCWD,
            target.as_ptr(),
            libc::RENAME_EXCL,
        )
    };
    if renamed == 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
    }
}

#[cfg(target_os = "linux")]
fn rename_noreplace(source: &Path, target: &Path) -> std::io::Result<()> {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt as _;

    let source = CString::new(source.as_os_str().as_bytes())
        .map_err(|error| std::io::Error::new(ErrorKind::InvalidInput, error))?;
    let target = CString::new(target.as_os_str().as_bytes())
        .map_err(|error| std::io::Error::new(ErrorKind::InvalidInput, error))?;
    #[allow(unsafe_code)]
    let renamed = unsafe {
        libc::renameat2(
            libc::AT_FDCWD,
            source.as_ptr(),
            libc::AT_FDCWD,
            target.as_ptr(),
            libc::RENAME_NOREPLACE,
        )
    };
    if renamed == 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
    }
}

#[cfg(all(unix, not(any(target_os = "macos", target_os = "linux"))))]
fn rename_noreplace(_source: &Path, _target: &Path) -> std::io::Result<()> {
    Err(std::io::Error::new(
        ErrorKind::Unsupported,
        "atomic no-replace directory moves are unavailable on this platform",
    ))
}

impl StdError for DurableRenameError {
    fn source(&self) -> Option<&(dyn StdError + 'static)> {
        match self {
            Self::NotMoved(source) => Some(source),
            #[cfg(unix)]
            Self::MovedNotDurable(source) => Some(source),
        }
    }
}

#[cfg(unix)]
fn durable_rename(source: &Path, target: &Path) -> std::result::Result<(), DurableRenameError> {
    rename_noreplace(source, target).map_err(DurableRenameError::NotMoved)?;
    let source_parent = source.parent().ok_or_else(|| {
        DurableRenameError::MovedNotDurable(std::io::Error::other("retention source has no parent"))
    })?;
    let target_parent = target.parent().ok_or_else(|| {
        DurableRenameError::MovedNotDurable(std::io::Error::other("retention target has no parent"))
    })?;
    full_fsync(&File::open(source_parent).map_err(DurableRenameError::MovedNotDurable)?)
        .map_err(DurableRenameError::MovedNotDurable)?;
    if source_parent != target_parent {
        full_fsync(&File::open(target_parent).map_err(DurableRenameError::MovedNotDurable)?)
            .map_err(DurableRenameError::MovedNotDurable)?;
    }
    Ok(())
}
#[cfg(windows)]
fn durable_rename(source: &Path, target: &Path) -> std::result::Result<(), DurableRenameError> {
    use std::os::windows::ffi::OsStrExt as _;
    use windows_sys::Win32::Storage::FileSystem::MoveFileExW;

    let source: Vec<u16> = source.as_os_str().encode_wide().chain(Some(0)).collect();
    let target: Vec<u16> = target.as_os_str().encode_wide().chain(Some(0)).collect();
    // Omitting `MOVEFILE_REPLACE_EXISTING` makes the move atomic and
    // no-clobber. `MOVEFILE_WRITE_THROUGH` rejects directory moves with
    // `ERROR_ACCESS_DENIED`, including two directories under one root.
    #[allow(unsafe_code)]
    let moved = unsafe { MoveFileExW(source.as_ptr(), target.as_ptr(), 0) };
    if moved == 0 {
        Err(DurableRenameError::NotMoved(std::io::Error::last_os_error()))
    } else {
        Ok(())
    }
}

#[cfg(not(any(unix, windows)))]
fn durable_rename(source: &Path, target: &Path) -> std::result::Result<(), DurableRenameError> {
    std::fs::rename(source, target).map_err(DurableRenameError::NotMoved)
}

/// Moves sessions out of the listing and sweeps the trash.
#[derive(Debug)]
pub struct RetentionService {
    root: StorageRoot,
    operations: Mutex<()>,
}

impl RetentionService {
    /// Builds the service over a storage root.
    #[must_use]
    pub const fn new(root: StorageRoot) -> Self {
        Self {
            root,
            operations: Mutex::new(()),
        }
    }

    fn directory(&self, destination: Destination) -> PathBuf {
        self.root.path().join(destination.directory())
    }

    fn index_path(&self) -> PathBuf {
        self.directory(Destination::Trash).join(INDEX_FILE)
    }

    fn ensure_directory(&self, destination: Destination) -> Result<PathBuf> {
        let directory = self.directory(destination);
        match std::fs::symlink_metadata(&directory) {
            Ok(metadata) if metadata.file_type().is_dir() => return Ok(directory),
            Ok(_) => {
                return Err(ApplicationError::new(
                    ErrorCode::StorageUnavailable,
                    format!("{} is not an ordinary directory", destination.directory()),
                ));
            }
            Err(source) if source.kind() == ErrorKind::NotFound => {}
            Err(source) => {
                return Err(ApplicationError::new(
                    ErrorCode::StorageUnavailable,
                    format!("{} could not be inspected", directory.display()),
                )
                .with_source(source));
            }
        }
        std::fs::create_dir_all(&directory).map_err(|source| {
            ApplicationError::new(
                ErrorCode::StorageUnavailable,
                format!("{} could not be created", directory.display()),
            )
            .with_source(source)
        })?;
        let metadata = std::fs::symlink_metadata(&directory).map_err(|source| {
            ApplicationError::new(
                ErrorCode::StorageUnavailable,
                format!("{} could not be inspected", directory.display()),
            )
            .with_source(source)
        })?;
        if !metadata.file_type().is_dir() {
            return Err(ApplicationError::new(
                ErrorCode::StorageUnavailable,
                format!("{} is not an ordinary directory", destination.directory()),
            ));
        }
        Ok(directory)
    }

    fn owned_candidate(
        folder: &str,
        candidate: &Path,
        marker: &Ulid,
    ) -> Result<Option<SessionRef>> {
        let metadata = match std::fs::symlink_metadata(candidate) {
            Ok(metadata) => metadata,
            Err(source) if source.kind() == ErrorKind::NotFound => return Ok(None),
            Err(source) => {
                return Err(ApplicationError::new(
                    ErrorCode::StorageUnavailable,
                    format!("{} could not be inspected", candidate.display()),
                )
                .with_source(source));
            }
        };
        if !metadata.file_type().is_dir() {
            return Ok(None);
        }
        let marker = marker.to_string();
        let marker_path = candidate.join(format!("{ENTRY_MARKER_PREFIX}{marker}"));
        match std::fs::read_to_string(marker_path) {
            Ok(found) if found == marker => {}
            Ok(_) => return Ok(None),
            Err(source) if source.kind() == ErrorKind::NotFound => return Ok(None),
            Err(source) => {
                return Err(ApplicationError::new(
                    ErrorCode::StorageUnavailable,
                    format!("the trash marker for {folder} could not be read"),
                )
                .with_source(source));
            }
        }
        let Ok(id) = SessionRef::parse(folder) else {
            return Ok(None);
        };
        let view = DirectoryView {
            folder: candidate.to_path_buf(),
        };
        Ok(matches!(classify(id.clone(), &view), Classified::Session(_)).then_some(id))
    }

    fn purge_authorized(staging: &Path, authorization: &Path, marker: &Ulid) -> Result<bool> {
        let marker = marker.to_string();
        match std::fs::read_to_string(authorization) {
            Ok(found) if found == marker => {
                sync_parent_directory(authorization)?;
                return Ok(true);
            }
            Ok(_) => return Ok(false),
            Err(source) if source.kind() == ErrorKind::NotFound => {}
            Err(source) => {
                return Err(ApplicationError::new(
                    ErrorCode::StorageUnavailable,
                    "the purge authorization could not be read",
                )
                .with_source(source));
            }
        }
        let marker_path = staging.join(format!("{ENTRY_MARKER_PREFIX}{marker}"));
        match std::fs::read_to_string(marker_path) {
            Ok(found) if found == marker => {}
            Ok(_) => return Ok(false),
            Err(source) if source.kind() == ErrorKind::NotFound => return Ok(false),
            Err(source) => {
                return Err(ApplicationError::new(
                    ErrorCode::StorageUnavailable,
                    "the staged trash marker could not be read",
                )
                .with_source(source));
            }
        }
        let mut file = match OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(authorization)
        {
            Ok(file) => file,
            Err(source) if source.kind() == ErrorKind::AlreadyExists => {
                let found = std::fs::read_to_string(authorization).map_err(|source| {
                    ApplicationError::new(
                        ErrorCode::StorageUnavailable,
                        "the purge authorization could not be read",
                    )
                    .with_source(source)
                })?;
                if found != marker {
                    return Ok(false);
                }
                sync_parent_directory(authorization)?;
                return Ok(true);
            }
            Err(source) => {
                return Err(ApplicationError::new(
                    ErrorCode::StorageUnavailable,
                    "the purge authorization could not be created",
                )
                .with_source(source));
            }
        };
        file.write_all(marker.as_bytes())
            .and_then(|()| full_fsync(&file))
            .map_err(|source| {
                ApplicationError::new(
                    ErrorCode::StorageUnavailable,
                    "the purge authorization could not be committed",
                )
                .with_source(source)
            })?;
        sync_parent_directory(authorization)?;
        Ok(true)
    }

    /// Moves one session into a retention directory.
    ///
    /// The folder name is preserved, so the move is reversible by hand
    /// and the session remains identifiable. A name already taken in
    /// the destination is refused rather than overwritten: two sessions
    /// cannot share a folder, and silently replacing one with the other
    /// would destroy the first.
    ///
    /// # Errors
    ///
    /// Returns an error when no session of that identity is present,
    /// when the destination directory cannot be created, when the
    /// destination already holds a folder of that name, when the move
    /// itself fails, or when the retention index cannot be written.
    pub fn retain(
        &self,
        id: &SessionRef,
        destination: Destination,
        now: DateTime<Utc>,
        retention_days: u32,
    ) -> Result<PathBuf> {
        let _operation = self
            .operations
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let source = self.root.resolve(id);
        let source_metadata = std::fs::symlink_metadata(&source).map_err(|source| {
            ApplicationError::new(
                ErrorCode::SessionNotFound,
                format!("no session named {id} is present under the storage root"),
            )
            .with_source(source)
        })?;
        if !source_metadata.file_type().is_dir() {
            return Err(ApplicationError::new(
                ErrorCode::SessionNotFound,
                format!("no session named {id} is present under the storage root"),
            ));
        }

        let folder = source
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or_else(|| {
                ApplicationError::new(
                    ErrorCode::StorageUnavailable,
                    "the session folder has no readable name",
                )
            })?
            .to_string();
        let target = self.ensure_directory(destination)?.join(&folder);
        match std::fs::symlink_metadata(&target) {
            Ok(_) => {
                return Err(ApplicationError::new(
                    ErrorCode::StorageUnavailable,
                    format!(
                        "{} already holds an entry named {folder}",
                        destination.directory()
                    ),
                ));
            }
            Err(source) if source.kind() == ErrorKind::NotFound => {}
            Err(source) => {
                return Err(ApplicationError::new(
                    ErrorCode::StorageUnavailable,
                    format!(
                        "{} could not be checked for an entry named {folder}",
                        destination.directory()
                    ),
                )
                .with_source(source));
            }
        }

        match destination {
            Destination::Archive => Self::move_to_archive(&source, target, &folder),
            Destination::Trash => self.move_to_trash(&source, target, &folder, now, retention_days),
        }
    }

    fn move_to_archive(source: &Path, target: PathBuf, folder: &str) -> Result<PathBuf> {
        if let Err(source) = durable_rename(source, &target) {
            let message = if source.moved() {
                format!("{folder} was moved to archive, but the move could not be synchronized")
            } else {
                format!("{folder} could not be moved to archive")
            };
            return Err(
                ApplicationError::new(ErrorCode::StorageUnavailable, message).with_source(source),
            );
        }
        Ok(target)
    }

    fn move_to_trash(
        &self,
        source: &Path,
        target: PathBuf,
        folder: &str,
        now: DateTime<Utc>,
        retention_days: u32,
    ) -> Result<PathBuf> {
        // Commit the authorization before the move. A crash before the
        // rename leaves an entry whose target is absent, which the next
        // sweep safely forgets. A crash after the rename leaves the
        // complete marker + index pair needed to sweep the folder.
        let path = self.index_path();
        let mut index = Index::read(&path)?;
        let marker = Ulid::new();
        let marker_path = source.join(format!("{ENTRY_MARKER_PREFIX}{marker}"));
        let mut marker_file = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&marker_path)
            .map_err(|source| {
                ApplicationError::new(
                    ErrorCode::StorageUnavailable,
                    format!("{folder} could not be marked for the trash"),
                )
                .with_source(source)
            })?;
        let marker_result = marker_file
            .write_all(marker.to_string().as_bytes())
            .and_then(|()| full_fsync(&marker_file));
        // Windows refuses both cleanup and the directory move while
        // this handle remains open.
        drop(marker_file);
        if let Err(source) = marker_result {
            let _ = std::fs::remove_file(&marker_path);
            return Err(ApplicationError::new(
                ErrorCode::StorageUnavailable,
                format!("{folder} could not be marked for the trash"),
            )
            .with_source(source));
        }
        if let Err(failure) = sync_parent_directory(&marker_path) {
            let _ = std::fs::remove_file(&marker_path);
            return Err(failure);
        }
        index.trashed.insert(
            folder.to_string(),
            TrashEntry {
                trashed_at: now,
                retention_days,
                marker,
                purging: false,
            },
        );
        if let Err(failure) = index.write(&path) {
            let _ = std::fs::remove_file(marker_path);
            return Err(failure);
        }

        if let Err(source_error) = durable_rename(source, &target) {
            if source_error.moved() {
                return Err(ApplicationError::new(
                    ErrorCode::StorageUnavailable,
                    format!("{folder} was moved to trash, but the move could not be synchronized"),
                )
                .with_source(source_error));
            }
            index.trashed.remove(folder);
            let cleanup = index.write(&path);
            let _ = std::fs::remove_file(marker_path);
            if let Err(cleanup_error) = cleanup {
                return Err(ApplicationError::new(
                    ErrorCode::StorageUnavailable,
                    format!(
                        "{folder} could not be moved to trash, and its retention entry \
                         could not be rolled back: {}",
                        cleanup_error.message()
                    ),
                )
                .with_source(source_error));
            }
            return Err(ApplicationError::new(
                ErrorCode::StorageUnavailable,
                format!("{folder} could not be moved to trash"),
            )
            .with_source(source_error));
        }

        Ok(target)
    }

    /// Removes every trashed session whose own window has passed.
    ///
    /// The window is evaluated per session against the instant that
    /// session was moved, not as one cutoff over the directory: a
    /// reader who deletes something today and something else next week
    /// expects each to survive its own seven days.
    ///
    /// # Errors
    ///
    /// Returns an error when the retention directory or index cannot be
    /// read safely, or when an index transition cannot be durably
    /// committed. A directory that cannot be removed is retained in an
    /// app-owned purge stage for the next launch.
    pub fn sweep(&self, now: DateTime<Utc>) -> Result<SweepOutcome> {
        let _operation = self
            .operations
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let directory = self.directory(Destination::Trash);
        match std::fs::symlink_metadata(&directory) {
            Err(source) if source.kind() == ErrorKind::NotFound => {
                return Ok(SweepOutcome::default());
            }
            Err(source) => {
                return Err(ApplicationError::new(
                    ErrorCode::StorageUnavailable,
                    format!("{} could not be inspected", directory.display()),
                )
                .with_source(source));
            }
            Ok(metadata) if !metadata.file_type().is_dir() => {
                return Err(ApplicationError::new(
                    ErrorCode::StorageUnavailable,
                    "trash is not an ordinary directory",
                ));
            }
            Ok(_) => {}
        }

        let path = self.index_path();
        let mut index = Index::read(&path)?;
        if index.trashed.is_empty() {
            return Ok(SweepOutcome::default());
        }

        let mut outcome = SweepOutcome::default();
        let mut changed = false;
        let folders: Vec<String> = index.trashed.keys().cloned().collect();
        for folder in folders {
            match Self::sweep_folder(&directory, &path, folder, now, &mut index, &mut outcome)? {
                IndexChange::Unchanged => {}
                IndexChange::Dirty => changed = true,
                IndexChange::Persisted => changed = false,
            }
        }
        if changed {
            index.write(&path)?;
        }
        Ok(outcome)
    }

    fn sweep_folder(
        directory: &Path,
        path: &Path,
        folder: String,
        now: DateTime<Utc>,
        index: &mut Index,
        outcome: &mut SweepOutcome,
    ) -> Result<IndexChange> {
        let Some(entry) = index.trashed.get(&folder).cloned() else {
            return Ok(IndexChange::Unchanged);
        };
        let candidate = directory.join(&folder);
        let staging = directory.join(format!("{PURGE_PREFIX}{}", entry.marker));
        let authorization = directory.join(format!("{PURGE_AUTH_PREFIX}{}", entry.marker));
        let mut persisted = false;

        if entry.purging {
            match std::fs::symlink_metadata(&staging) {
                Ok(metadata) if metadata.file_type().is_dir() => {}
                Err(source) if source.kind() == ErrorKind::NotFound => {
                    match Self::owned_candidate(&folder, &candidate, &entry.marker) {
                        Ok(Some(_)) => {
                            if durable_rename(&candidate, &staging).is_err() {
                                outcome.retained.push(folder);
                                return Ok(IndexChange::Unchanged);
                            }
                        }
                        Ok(None) => {
                            let _ = std::fs::remove_file(&authorization);
                            index.trashed.remove(&folder);
                            outcome.forgotten.push(folder);
                            return Ok(IndexChange::Dirty);
                        }
                        Err(_) => {
                            outcome.retained.push(folder);
                            return Ok(IndexChange::Unchanged);
                        }
                    }
                }
                Ok(_) | Err(_) => {
                    outcome.retained.push(folder);
                    return Ok(IndexChange::Unchanged);
                }
            }
        } else {
            match Self::owned_candidate(&folder, &candidate, &entry.marker) {
                Ok(Some(_)) => {}
                Ok(None) => {
                    index.trashed.remove(&folder);
                    outcome.forgotten.push(folder);
                    return Ok(IndexChange::Dirty);
                }
                Err(_) => {
                    outcome.retained.push(folder);
                    return Ok(IndexChange::Unchanged);
                }
            }
            let retained_for = now.signed_duration_since(entry.trashed_at).num_seconds();
            let required = i64::from(entry.retention_days) * 86_400;
            if retained_for < required {
                outcome.retained.push(folder);
                return Ok(IndexChange::Unchanged);
            }

            // Persist the purge transition before renaming. A crash
            // before the rename can resume from the owned candidate;
            // a crash after it can resume from the unique staging
            // directory even if recursive removal was partial.
            if let Some(stored) = index.trashed.get_mut(&folder) {
                stored.purging = true;
            }
            index.write(path)?;
            persisted = true;
            if durable_rename(&candidate, &staging).is_err() {
                outcome.retained.push(folder);
                return Ok(IndexChange::Persisted);
            }
        }

        if !matches!(
            Self::purge_authorized(&staging, &authorization, &entry.marker),
            Ok(true)
        ) {
            outcome.retained.push(folder);
            return Ok(if persisted {
                IndexChange::Persisted
            } else {
                IndexChange::Unchanged
            });
        }

        if std::fs::remove_dir_all(&staging).is_ok() {
            let _ = std::fs::remove_file(&authorization);
            index.trashed.remove(&folder);
            outcome.removed.push(folder);
            Ok(IndexChange::Dirty)
        } else {
            outcome.retained.push(folder);
            Ok(if persisted {
                IndexChange::Persisted
            } else {
                IndexChange::Unchanged
            })
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::super::SessionRepository;
    use super::*;
    use crate::PageRequest;
    use pretty_assertions::assert_eq;

    use std::sync::Arc;
    struct Tree {
        dir: tempfile::TempDir,
    }

    impl Tree {
        fn new() -> Self {
            Self {
                dir: tempfile::tempdir().unwrap(),
            }
        }

        fn root(&self) -> StorageRoot {
            StorageRoot::new(self.dir.path())
        }

        fn service(&self) -> RetentionService {
            RetentionService::new(self.root())
        }

        /// A folder that classifies as a complete session.
        fn session(&self, folder: &str) -> &Self {
            self.session_under(self.dir.path(), folder)
        }

        fn session_under(&self, parent: &Path, folder: &str) -> &Self {
            let path = parent.join(folder);
            std::fs::create_dir_all(&path).unwrap();
            std::fs::write(
                path.join("meta.toml"),
                "session_id = \"01AAA\"\n\
                 title = \"a meeting\"\n\
                 started_at = \"2026-04-29T14:30:00Z\"\n\
                 ended_at = \"2026-04-29T15:00:00Z\"\n\
                 duration_secs = 1800\n",
            )
            .unwrap();
            std::fs::write(path.join("audio.opus"), b"").unwrap();
            self
        }

        fn listed(&self) -> Vec<String> {
            SessionRepository::new(self.root())
                .list_sessions(PageRequest::default())
                .unwrap()
                .items
                .into_iter()
                .map(|session| session.id.to_string())
                .collect()
        }
    }

    fn id(text: &str) -> SessionRef {
        SessionRef::parse(text).unwrap()
    }

    fn at(day: u32) -> DateTime<Utc> {
        format!("2026-05-{day:02}T12:00:00Z").parse().unwrap()
    }

    #[test]
    fn test_a_deleted_session_leaves_the_listing_and_lands_in_the_trash() {
        let tree = Tree::new();
        tree.session("2026-04-29-1430-standup");
        assert_eq!(tree.listed().len(), 1);

        let target = tree
            .service()
            .retain(&id("2026-04-29-1430-standup"), Destination::Trash, at(1), 7)
            .unwrap();

        assert!(target.is_dir(), "the session should exist at its new home");
        assert_eq!(
            target.parent().unwrap().file_name().unwrap(),
            TRASH_DIR,
            "a deleted session belongs in the trash directory"
        );
        assert!(
            tree.listed().is_empty(),
            "a deleted session must leave the listing"
        );
    }

    #[test]
    fn test_an_archived_session_leaves_the_listing_and_is_never_swept() {
        let tree = Tree::new();
        tree.session("2026-04-29-1430-standup");

        tree.service()
            .retain(
                &id("2026-04-29-1430-standup"),
                Destination::Archive,
                at(1),
                7,
            )
            .unwrap();
        assert!(tree.listed().is_empty());

        // A year later, with a one-day window.
        let outcome = tree.service().sweep(at(28)).unwrap();
        assert!(
            outcome.removed.is_empty(),
            "the archive is never swept: {outcome:?}"
        );
        assert!(
            tree.dir
                .path()
                .join(ARCHIVE_DIR)
                .join("2026-04-29-1430-standup")
                .is_dir(),
            "the archived session must still be there"
        );
    }

    /// The guard this proves: the window is read per session, from that
    /// session's own move time. A single cutoff over the directory would
    /// take both or neither.
    #[test]
    fn test_the_sweep_takes_the_session_past_its_window_and_leaves_the_newer_one() {
        let tree = Tree::new();
        tree.session("2026-04-29-1430-old");
        tree.session("2026-04-29-1430-new");
        let service = tree.service();

        service
            .retain(&id("2026-04-29-1430-old"), Destination::Trash, at(1), 7)
            .unwrap();
        service
            .retain(&id("2026-04-29-1430-new"), Destination::Trash, at(10), 7)
            .unwrap();

        // Day 12: the first is eleven days in, the second is two.
        let outcome = service.sweep(at(12)).unwrap();

        assert_eq!(outcome.removed, vec!["2026-04-29-1430-old".to_string()]);
        assert_eq!(outcome.retained, vec!["2026-04-29-1430-new".to_string()]);
        let trash = tree.dir.path().join(TRASH_DIR);
        assert!(!trash.join("2026-04-29-1430-old").exists());
        assert!(trash.join("2026-04-29-1430-new").is_dir());
    }

    #[test]
    fn test_a_session_inside_its_window_survives_the_sweep() {
        let tree = Tree::new();
        tree.session("2026-04-29-1430-standup");
        let service = tree.service();
        service
            .retain(&id("2026-04-29-1430-standup"), Destination::Trash, at(1), 7)
            .unwrap();

        // Six days is inside a seven-day window.
        let outcome = service.sweep(at(7)).unwrap();
        assert!(outcome.removed.is_empty(), "{outcome:?}");
        assert!(tree
            .dir
            .path()
            .join(TRASH_DIR)
            .join("2026-04-29-1430-standup")
            .is_dir());
    }

    /// The guard this proves: the sweep removes only what this
    /// application moved. A reader's own directory in `trash/` has no
    /// index entry, so it has no age and cannot be swept.
    #[test]
    fn test_the_sweep_leaves_a_directory_the_application_did_not_move() {
        let tree = Tree::new();
        let trash = tree.dir.path().join(TRASH_DIR);
        std::fs::create_dir_all(&trash).unwrap();
        // A real session, by content — but placed there by hand.
        tree.session_under(&trash, "2026-04-29-1430-by-hand");
        std::fs::write(trash.join("a-note-to-self.txt"), b"keep this").unwrap();

        let outcome = tree.service().sweep(at(28)).unwrap();

        assert!(outcome.removed.is_empty(), "{outcome:?}");
        assert!(
            trash.join("2026-04-29-1430-by-hand").is_dir(),
            "a hand-placed session must survive: the application never moved it"
        );
        assert!(trash.join("a-note-to-self.txt").is_file());
    }

    /// The guard this proves: an index entry alone does not authorize a
    /// recursive removal. If the folder stopped being a session, the
    /// sweep leaves it rather than acting on a stale reading.
    #[test]
    fn test_the_sweep_leaves_a_trashed_folder_that_no_longer_looks_like_a_session() {
        let tree = Tree::new();
        tree.session("2026-04-29-1430-standup");
        let service = tree.service();
        service
            .retain(&id("2026-04-29-1430-standup"), Destination::Trash, at(1), 7)
            .unwrap();

        // Emptied out after it was trashed: no artifact left to classify.
        let folder = tree
            .dir
            .path()
            .join(TRASH_DIR)
            .join("2026-04-29-1430-standup");
        std::fs::remove_file(folder.join("meta.toml")).unwrap();
        std::fs::remove_file(folder.join("audio.opus")).unwrap();
        std::fs::write(folder.join("something-else.bin"), b"?").unwrap();

        let outcome = service.sweep(at(28)).unwrap();

        assert!(outcome.removed.is_empty(), "{outcome:?}");
        assert!(folder.is_dir(), "it is not ours to remove any more");
    }

    /// The guard this proves: `trash/` and `archive/` are absent from
    /// the listing by name, not because they happen to hold no
    /// artifacts. Both names pass `SessionRef::parse`, so without the
    /// skip a `notes.md` inside either makes the directory itself
    /// classify as an unfinished session.
    #[test]
    fn test_a_retention_directory_is_never_listed_even_holding_session_artifacts() {
        let tree = Tree::new();
        tree.session("2026-04-29-1430-standup");
        for reserved in [TRASH_DIR, ARCHIVE_DIR] {
            let path = tree.dir.path().join(reserved);
            std::fs::create_dir_all(&path).unwrap();
            std::fs::write(path.join("notes.md"), "## TL;DR\n- not a session\n").unwrap();
        }

        assert_eq!(
            tree.listed(),
            vec!["2026-04-29-1430-standup".to_string()],
            "only the real session may be listed"
        );
    }

    /// `rename` refuses to replace a *non-empty* directory, so the OS
    /// would have caught that case without help. It replaces an empty
    /// one silently, and that is where this guard is load-bearing: a
    /// half-created folder of the same name in the destination would
    /// otherwise absorb the move and the session would be gone.
    #[test]
    fn test_a_name_already_taken_in_the_destination_is_refused_not_overwritten() {
        let tree = Tree::new();
        tree.session("2026-04-29-1430-standup");
        let trash = tree.dir.path().join(TRASH_DIR);
        let occupied = trash.join("2026-04-29-1430-standup");
        std::fs::create_dir_all(&occupied).unwrap();

        let refusal =
            tree.service()
                .retain(&id("2026-04-29-1430-standup"), Destination::Trash, at(1), 7);

        assert!(refusal.is_err(), "two sessions cannot share one folder");
        assert!(
            tree.dir.path().join("2026-04-29-1430-standup").is_dir(),
            "the session must stay where it was rather than be moved onto the empty folder"
        );
        assert!(
            tree.dir
                .path()
                .join("2026-04-29-1430-standup")
                .join("meta.toml")
                .is_file(),
            "with its artifacts intact"
        );
    }

    #[test]
    fn test_the_move_primitive_never_replaces_an_existing_directory() {
        let tree = Tree::new();
        let source = tree.dir.path().join("source");
        let target = tree.dir.path().join("target");
        std::fs::create_dir(&source).unwrap();
        std::fs::write(source.join("owned"), b"source").unwrap();
        std::fs::create_dir(&target).unwrap();

        let refusal = durable_rename(&source, &target).unwrap_err();

        assert!(!refusal.moved());
        assert!(source.join("owned").is_file());
        assert!(target.read_dir().unwrap().next().is_none());
    }

    #[cfg(unix)]
    #[test]
    fn test_a_dangling_symlink_at_the_destination_is_refused_not_overwritten() {
        use std::os::unix::fs::symlink;

        let tree = Tree::new();
        tree.session("2026-04-29-1430-standup");
        let trash = tree.dir.path().join(TRASH_DIR);
        std::fs::create_dir_all(&trash).unwrap();
        let occupied = trash.join("2026-04-29-1430-standup");
        symlink("missing-session", &occupied).unwrap();

        let refusal =
            tree.service()
                .retain(&id("2026-04-29-1430-standup"), Destination::Trash, at(1), 7);

        assert!(refusal.is_err());
        assert!(std::fs::symlink_metadata(occupied)
            .unwrap()
            .file_type()
            .is_symlink());
        assert!(tree.dir.path().join("2026-04-29-1430-standup").is_dir());
    }

    #[test]
    fn test_an_installation_with_no_trash_sweeps_nothing_and_fails_nothing() {
        let tree = Tree::new();
        tree.session("2026-04-29-1430-standup");

        let outcome = tree.service().sweep(at(28)).unwrap();

        assert_eq!(outcome, SweepOutcome::default());
        assert_eq!(tree.listed().len(), 1, "and the listing is untouched");
    }

    #[test]
    fn test_a_restored_folder_is_forgotten_rather_than_remembered_forever() {
        let tree = Tree::new();
        tree.session("2026-04-29-1430-standup");
        let service = tree.service();
        service
            .retain(&id("2026-04-29-1430-standup"), Destination::Trash, at(1), 7)
            .unwrap();

        // Put back by hand, the way the design says a reader restores.
        std::fs::rename(
            tree.dir
                .path()
                .join(TRASH_DIR)
                .join("2026-04-29-1430-standup"),
            tree.dir.path().join("2026-04-29-1430-standup"),
        )
        .unwrap();

        let outcome = service.sweep(at(28)).unwrap();

        assert_eq!(
            outcome.forgotten,
            vec!["2026-04-29-1430-standup".to_string()]
        );
        assert!(outcome.removed.is_empty());
        assert_eq!(
            tree.listed(),
            vec!["2026-04-29-1430-standup".to_string()],
            "a restored session is listed again"
        );
        // And the entry is gone, so a later folder of the same name is
        // not swept on the strength of the old reading.
        let outcome = service.sweep(at(29)).unwrap();
        assert_eq!(outcome, SweepOutcome::default());
    }

    #[test]
    fn test_moving_a_session_that_is_not_there_is_refused() {
        let tree = Tree::new();
        let refusal =
            tree.service()
                .retain(&id("2026-04-29-1430-absent"), Destination::Trash, at(1), 7);
        assert!(refusal.is_err());
    }

    #[test]
    fn test_a_corrupt_index_blocks_a_move_instead_of_forgetting_older_entries() {
        let tree = Tree::new();
        tree.session("2026-04-29-1430-standup");
        let trash = tree.dir.path().join(TRASH_DIR);
        std::fs::create_dir_all(&trash).unwrap();
        std::fs::write(trash.join(INDEX_FILE), "[trashed\n").unwrap();

        let refusal =
            tree.service()
                .retain(&id("2026-04-29-1430-standup"), Destination::Trash, at(1), 7);

        assert!(refusal.is_err());
        assert!(tree.dir.path().join("2026-04-29-1430-standup").is_dir());
    }

    #[test]
    fn test_a_replacement_at_an_indexed_name_is_never_removed() {
        let tree = Tree::new();
        tree.session("2026-04-29-1430-standup");
        let service = tree.service();
        service
            .retain(&id("2026-04-29-1430-standup"), Destination::Trash, at(1), 7)
            .unwrap();
        let trash = tree.dir.path().join(TRASH_DIR);
        let candidate = trash.join("2026-04-29-1430-standup");
        std::fs::remove_dir_all(&candidate).unwrap();
        tree.session_under(&trash, "2026-04-29-1430-standup");

        let outcome = service.sweep(at(28)).unwrap();

        assert!(outcome.removed.is_empty());
        assert_eq!(
            outcome.forgotten,
            vec!["2026-04-29-1430-standup".to_string()]
        );
        assert!(candidate.is_dir(), "the foreign replacement must survive");
    }

    #[test]
    fn test_an_index_marker_must_be_a_canonical_ulid_before_paths_are_built() {
        let tree = Tree::new();
        let trash = tree.dir.path().join(TRASH_DIR);
        std::fs::create_dir_all(&trash).unwrap();
        std::fs::write(
            trash.join(INDEX_FILE),
            "[trashed.\"2026-04-29-1430-standup\"]\n\
             trashed_at = \"2026-05-01T12:00:00Z\"\n\
             retention_days = 7\n\
             marker = \"x/../../../outside\"\n\
             purging = true\n",
        )
        .unwrap();

        assert!(tree.service().sweep(at(28)).is_err());
    }

    #[test]
    fn test_a_partial_purge_resumes_from_its_durable_authorization() {
        let tree = Tree::new();
        tree.session("2026-04-29-1430-standup");
        let service = tree.service();
        service
            .retain(&id("2026-04-29-1430-standup"), Destination::Trash, at(1), 7)
            .unwrap();
        let trash = tree.dir.path().join(TRASH_DIR);
        let path = trash.join(INDEX_FILE);
        let mut index = Index::read(&path).unwrap();
        let entry = index.trashed.get_mut("2026-04-29-1430-standup").unwrap();
        entry.purging = true;
        let marker = entry.marker;
        index.write(&path).unwrap();
        let candidate = trash.join("2026-04-29-1430-standup");
        let staging = trash.join(format!("{PURGE_PREFIX}{marker}"));
        durable_rename(&candidate, &staging).unwrap();
        let authorization = trash.join(format!("{PURGE_AUTH_PREFIX}{marker}"));
        assert!(RetentionService::purge_authorized(&staging, &authorization, &marker).unwrap());
        std::fs::remove_file(staging.join(format!("{ENTRY_MARKER_PREFIX}{marker}"))).unwrap();

        let outcome = service.sweep(at(28)).unwrap();

        assert_eq!(outcome.removed, vec!["2026-04-29-1430-standup".to_string()]);
        assert!(!staging.exists());
    }

    #[test]
    fn test_retain_serializes_the_index_read_modify_write() {
        let tree = Tree::new();
        tree.session("2026-04-29-1430-standup");
        let service = Arc::new(tree.service());
        let held = service
            .operations
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let worker = Arc::clone(&service);
        let (sent, received) = std::sync::mpsc::channel();
        let thread = std::thread::spawn(move || {
            sent.send(worker.retain(&id("2026-04-29-1430-standup"), Destination::Trash, at(1), 7))
                .unwrap();
        });

        assert!(
            received
                .recv_timeout(std::time::Duration::from_millis(30))
                .is_err(),
            "retain must wait for the service's index transaction lock"
        );
        drop(held);
        assert!(received
            .recv_timeout(std::time::Duration::from_secs(1))
            .unwrap()
            .is_ok());
        thread.join().unwrap();
    }
}
