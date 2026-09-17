// Copyright 2026 Mathews Tom
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//     https://www.apache.org/licenses/LICENSE-2.0

//! Opaque session identity and the storage root it is confined to.
//!
//! A frontend never supplies a filesystem path. It supplies a
//! [`SessionRef`], whose textual form is a session-folder name (or an
//! unambiguous fragment of one) and whose construction rejects every
//! shape that could address something other than a direct child of the
//! configured storage root: absolute paths, path separators, `.`/`..`
//! traversal, Windows drive-relative and alternate-data-stream syntax,
//! home-relative `~` prefixes, and control characters.
//!
//! The absolute folder path is derived by [`StorageRoot::resolve`] and
//! retained inside the service layer; it is never part of a serialized
//! contract, so no consumer can round-trip a path back in.

use std::fmt;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// Why a caller-supplied session identity was refused at the boundary.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum IdentityRejection {
    /// The identity was empty or entirely whitespace.
    Empty,
    /// The identity contained a control character.
    Control,
    /// The identity was an absolute filesystem path.
    Absolute,
    /// The identity contained a `/` or `\` path separator.
    Separator,
    /// The identity contained a `.` or `..` path component.
    Traversal,
    /// The identity contained `:`, which selects a Windows drive-relative
    /// path or an NTFS alternate data stream.
    DriveOrStream,
    /// The identity began with `~`, which a consumer may expand to a home
    /// directory outside the storage root.
    HomeRelative,
    /// A partial-download name did not end in `.partial`, so it names
    /// something other than an abandoned download.
    NotAPartialFile,
}

impl IdentityRejection {
    /// Stable, user-safe explanation. Never quotes the rejected text, so
    /// an identity carrying meeting content cannot be echoed back.
    #[must_use]
    pub const fn reason(self) -> &'static str {
        match self {
            Self::Empty => "session identity must not be empty",
            Self::Control => "session identity must not contain control characters",
            Self::Absolute => "session identity must not be an absolute path",
            Self::Separator => "session identity must not contain a path separator",
            Self::Traversal => "session identity must not contain a `.` or `..` component",
            Self::DriveOrStream => "session identity must not contain `:`",
            Self::HomeRelative => "session identity must not start with `~`",
            Self::NotAPartialFile => "partial-download name must end in `.partial`",
        }
    }
}

impl fmt::Display for IdentityRejection {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.reason())
    }
}

impl std::error::Error for IdentityRejection {}

/// An opaque, root-confined reference to one session.
///
/// Construction is the only validation point. Once a `SessionRef`
/// exists it names at most one direct child of a [`StorageRoot`], so
/// downstream code performs no further path checking.
///
/// The textual form is either a complete session-folder name or an
/// unambiguous fragment of one (typically a session ULID or its
/// prefix). Resolving a fragment against a root yields the canonical
/// `SessionRef` carrying the full folder name.
#[derive(Clone, Debug, Eq, PartialEq, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct SessionRef(String);

impl SessionRef {
    /// Validates `candidate` and returns a confined reference.
    ///
    /// # Errors
    ///
    /// [`IdentityRejection`] describing the first rule the candidate
    /// broke.
    pub fn parse(candidate: &str) -> Result<Self, IdentityRejection> {
        validate(candidate)?;
        Ok(Self(candidate.to_string()))
    }

    /// The identity's textual form.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for SessionRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl TryFrom<String> for SessionRef {
    type Error = IdentityRejection;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        validate(&value)?;
        Ok(Self(value))
    }
}

impl From<SessionRef> for String {
    fn from(value: SessionRef) -> Self {
        value.0
    }
}

fn validate(candidate: &str) -> Result<(), IdentityRejection> {
    if candidate.trim().is_empty() {
        return Err(IdentityRejection::Empty);
    }
    if candidate.chars().any(char::is_control) {
        return Err(IdentityRejection::Control);
    }
    // `Path::is_absolute` is platform-dependent: Windows calls
    // `/etc/passwd` root-relative rather than absolute, so relying on
    // it alone would classify the same identity differently on
    // different platforms. A leading separator names a filesystem root
    // everywhere, so it is refused as absolute everywhere.
    if candidate.starts_with('/')
        || candidate.starts_with('\\')
        || Path::new(candidate).is_absolute()
    {
        return Err(IdentityRejection::Absolute);
    }
    if candidate
        .split(['/', '\\'])
        .any(|segment| segment == "." || segment == "..")
    {
        return Err(IdentityRejection::Traversal);
    }
    if candidate.contains('/') || candidate.contains('\\') {
        return Err(IdentityRejection::Separator);
    }
    if candidate.contains(':') {
        return Err(IdentityRejection::DriveOrStream);
    }
    if candidate.starts_with('~') {
        return Err(IdentityRejection::HomeRelative);
    }
    Ok(())
}

/// The suffix every abandoned-download file carries.
pub const PARTIAL_SUFFIX: &str = ".partial";

/// An opaque, root-confined reference to one leftover `.partial` file.
///
/// Holds the same guarantee as [`SessionRef`] — the name addresses at
/// most one direct child of a [`StorageRoot`] — plus the requirement
/// that it end in [`PARTIAL_SUFFIX`], so a recovery action carrying one
/// cannot be pointed at anything but an abandoned download.
///
/// Construction is the only validation point, so downstream code
/// performs no further name checking.
#[derive(Clone, Debug, Eq, PartialEq, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct PartialFileRef(String);

impl PartialFileRef {
    /// Validates `candidate` and returns a confined reference.
    ///
    /// # Errors
    ///
    /// [`IdentityRejection`] describing the first rule the candidate
    /// broke.
    pub fn parse(candidate: &str) -> Result<Self, IdentityRejection> {
        validate(candidate)?;
        if !candidate.ends_with(PARTIAL_SUFFIX) {
            return Err(IdentityRejection::NotAPartialFile);
        }
        Ok(Self(candidate.to_string()))
    }

    /// The name's textual form.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for PartialFileRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl TryFrom<String> for PartialFileRef {
    type Error = IdentityRejection;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::parse(&value)
    }
}

impl From<PartialFileRef> for String {
    fn from(value: PartialFileRef) -> Self {
        value.0
    }
}

/// The configured storage root every session identity resolves beneath.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StorageRoot {
    path: PathBuf,
}

impl StorageRoot {
    /// Wraps an already-expanded absolute or relative storage root.
    ///
    /// Tilde expansion is a presentation concern and stays with the
    /// caller; this type only guarantees that session identities cannot
    /// escape whatever root it is given.
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    /// The root directory itself.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The absolute folder for `id`.
    ///
    /// Always a direct child of [`Self::path`]: [`SessionRef`]
    /// construction has already excluded every component that could
    /// make the join escape.
    #[must_use]
    pub fn resolve(&self, id: &SessionRef) -> PathBuf {
        self.path.join(id.as_str())
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    #[test]
    fn test_parse_accepts_a_session_folder_name() {
        let parsed = SessionRef::parse("2026-04-29-1430-acme-01HXYZ").unwrap();

        assert_eq!(parsed.as_str(), "2026-04-29-1430-acme-01HXYZ");
    }

    #[test]
    fn test_parse_rejects_absolute_path() {
        let rejection = SessionRef::parse("/etc/passwd").unwrap_err();

        assert_eq!(rejection, IdentityRejection::Absolute);
    }

    #[test]
    fn test_parse_rejects_a_rooted_identity_the_same_way_on_every_platform() {
        // Windows does not consider a leading `/` absolute; the
        // classification must not depend on where this runs.
        for rooted in ["/etc/passwd", "\\\\server\\share", "/", "\\"] {
            assert_eq!(
                SessionRef::parse(rooted).unwrap_err(),
                IdentityRejection::Absolute,
                "{rooted} must be refused as absolute"
            );
        }
    }

    #[test]
    fn test_parse_rejects_relative_path_with_separator() {
        let rejection = SessionRef::parse("nested/session").unwrap_err();

        assert_eq!(rejection, IdentityRejection::Separator);
    }

    #[test]
    fn test_parse_rejects_backslash_separator() {
        let rejection = SessionRef::parse("nested\\session").unwrap_err();

        assert_eq!(rejection, IdentityRejection::Separator);
    }

    #[test]
    fn test_parse_rejects_parent_traversal() {
        let rejection = SessionRef::parse("../../etc").unwrap_err();

        assert_eq!(rejection, IdentityRejection::Traversal);
    }

    #[test]
    fn test_parse_rejects_bare_current_directory() {
        let rejection = SessionRef::parse(".").unwrap_err();

        assert_eq!(rejection, IdentityRejection::Traversal);
    }

    #[test]
    fn test_parse_rejects_windows_drive_relative_identity() {
        let rejection = SessionRef::parse("C:sessions").unwrap_err();

        assert_eq!(rejection, IdentityRejection::DriveOrStream);
    }

    #[test]
    fn test_parse_rejects_home_relative_identity() {
        let rejection = SessionRef::parse("~scrybe").unwrap_err();

        assert_eq!(rejection, IdentityRejection::HomeRelative);
    }

    #[test]
    fn test_parse_rejects_embedded_nul() {
        let rejection = SessionRef::parse("session\0name").unwrap_err();

        assert_eq!(rejection, IdentityRejection::Control);
    }

    #[test]
    fn test_parse_rejects_whitespace_only_identity() {
        let rejection = SessionRef::parse("   ").unwrap_err();

        assert_eq!(rejection, IdentityRejection::Empty);
    }

    #[test]
    fn test_resolve_places_every_accepted_identity_directly_under_the_root() {
        let root = StorageRoot::new("/var/scrybe");

        for accepted in ["2026-04-29-1430-acme-01HXYZ", "01HXYZ", "a b", "ünïcode"] {
            let id = SessionRef::parse(accepted).unwrap();

            let resolved = root.resolve(&id);

            assert_eq!(resolved.parent(), Some(root.path()));
            assert_eq!(
                resolved.file_name().and_then(std::ffi::OsStr::to_str),
                Some(accepted)
            );
        }
    }

    #[test]
    fn test_deserializing_an_identity_applies_the_same_confinement_rules() {
        let rejected = serde_json::from_str::<SessionRef>("\"../escape\"");

        assert!(rejected.is_err());
    }

    #[test]
    fn test_identity_round_trips_through_json_as_a_plain_string() {
        let id = SessionRef::parse("2026-04-29-1430-acme-01HXYZ").unwrap();

        let encoded = serde_json::to_string(&id).unwrap();

        assert_eq!(encoded, "\"2026-04-29-1430-acme-01HXYZ\"");
        assert_eq!(serde_json::from_str::<SessionRef>(&encoded).unwrap(), id);
    }

    #[test]
    fn test_partial_reference_accepts_a_name_directly_under_the_root() {
        let parsed = PartialFileRef::parse("model-tiny.bin.partial").unwrap();

        assert_eq!(parsed.as_str(), "model-tiny.bin.partial");
    }

    #[test]
    fn test_partial_reference_refuses_every_name_that_could_address_elsewhere() {
        let cases = [
            ("/tmp/leftover.partial", IdentityRejection::Absolute),
            ("\\tmp\\leftover.partial", IdentityRejection::Absolute),
            ("sub/leftover.partial", IdentityRejection::Separator),
            ("sub\\leftover.partial", IdentityRejection::Separator),
            ("../leftover.partial", IdentityRejection::Traversal),
            ("C:leftover.partial", IdentityRejection::DriveOrStream),
            ("~/leftover.partial", IdentityRejection::Separator),
            ("~leftover.partial", IdentityRejection::HomeRelative),
            ("left\u{7}over.partial", IdentityRejection::Control),
            ("   ", IdentityRejection::Empty),
            ("leftover.bin", IdentityRejection::NotAPartialFile),
            ("leftover.partial.bin", IdentityRejection::NotAPartialFile),
        ];

        for (candidate, expected) in cases {
            assert_eq!(
                PartialFileRef::parse(candidate).unwrap_err(),
                expected,
                "{candidate} was not refused as expected"
            );
        }
    }

    #[test]
    fn test_deserializing_a_partial_reference_applies_the_same_rules() {
        assert!(serde_json::from_str::<PartialFileRef>("\"C:leftover.partial\"").is_err());
        assert!(serde_json::from_str::<PartialFileRef>("\"leftover.bin\"").is_err());
        assert_eq!(
            serde_json::from_str::<PartialFileRef>("\"leftover.partial\"").unwrap(),
            PartialFileRef::parse("leftover.partial").unwrap()
        );
    }
}
