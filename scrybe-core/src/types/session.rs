// Copyright 2026 Mathews Tom
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//     https://www.apache.org/licenses/LICENSE-2.0

//! Session identifier — ULID, lexicographically sortable, grep-friendly.
//!
//! Uniqueness within a minute is required by `docs/system-design.md` §8.1
//! to prevent folder collisions when two `scrybe record` invocations land
//! in the same minute-stamped directory.

use std::str::FromStr;

use serde::{Deserialize, Serialize};
use ulid::Ulid;

/// A 128-bit ULID rendered as a 26-character Crockford-base32 string.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SessionId(Ulid);

impl SessionId {
    /// Generate a fresh ULID at the current wall-clock time.
    #[must_use]
    pub fn new() -> Self {
        Self(Ulid::new())
    }

    /// 26-character Crockford-base32 representation suitable for filenames
    /// and folder suffixes.
    #[must_use]
    pub fn to_string_26(self) -> String {
        self.0.to_string()
    }

    /// The underlying ULID. Exposed so callers can extract the embedded
    /// timestamp without round-tripping through string form.
    #[must_use]
    pub const fn as_ulid(self) -> Ulid {
        self.0
    }
}

impl Default for SessionId {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Display for SessionId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl FromStr for SessionId {
    type Err = ulid::DecodeError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        s.parse::<Ulid>().map(Self)
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    #[test]
    fn test_session_id_to_string_26_returns_canonical_length() {
        let id = SessionId::new();

        assert_eq!(id.to_string_26().len(), 26);
    }

    #[test]
    fn test_session_id_two_consecutive_ids_are_distinct() {
        let a = SessionId::new();
        let b = SessionId::new();

        assert_ne!(a, b);
    }

    #[test]
    fn test_session_id_round_trips_through_string_via_from_str() {
        let original = SessionId::new();

        let parsed: SessionId = original.to_string().parse().unwrap();

        assert_eq!(parsed, original);
    }

    #[test]
    fn test_session_id_round_trips_through_json_as_transparent_string() {
        let id = SessionId::new();

        let encoded = serde_json::to_string(&id).unwrap();
        let decoded: SessionId = serde_json::from_str(&encoded).unwrap();

        assert_eq!(decoded, id);
        assert!(encoded.starts_with('"'));
        assert!(encoded.ends_with('"'));
        assert_eq!(encoded.len(), 28);
    }

    #[test]
    fn test_session_id_default_produces_distinct_id() {
        let a = SessionId::default();
        let b = SessionId::default();

        assert_ne!(a, b);
    }

    #[test]
    fn test_session_id_lexicographic_order_matches_creation_order() {
        let earlier = SessionId(Ulid::from_parts(1_700_000_000_000, 0));
        let later = SessionId(Ulid::from_parts(1_800_000_000_000, 0));

        assert!(earlier < later);
        assert!(earlier.to_string_26() < later.to_string_26());
    }
}
