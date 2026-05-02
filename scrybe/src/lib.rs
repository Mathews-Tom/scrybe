// Copyright 2026 Mathews Tom
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//     https://www.apache.org/licenses/LICENSE-2.0

//! # scrybe
//!
//! Open-source local-first meeting transcription and notes.
//!
//! This crate is a **pre-release namespace placeholder**. The functional
//! library, CLI, and platform capture adapters are tracked in the
//! repository at <https://github.com/Mathews-Tom/scrybe>; the architecture
//! is documented in `docs/system-overview.md` and `docs/system-design.md`,
//! and the delivery plan in `.docs/development-plan.md`.
//!
//! No public API is exposed at this version. Depend on a later release
//! once the `scrybe-core` crate ships.

/// Project name as exposed in CLI banners and crate metadata.
pub const NAME: &str = "scrybe";

/// Crate version reported by the placeholder; matches `Cargo.toml`.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Repository URL for users who reach this crate from `cargo search`.
pub const REPOSITORY: &str = "https://github.com/Mathews-Tom/scrybe";

#[cfg(test)]
mod tests {
    use super::{NAME, REPOSITORY, VERSION};

    #[test]
    fn test_name_constant_matches_crate_identity() {
        assert_eq!(NAME, "scrybe");
    }

    #[test]
    fn test_version_constant_matches_cargo_metadata() {
        assert_eq!(VERSION, env!("CARGO_PKG_VERSION"));
        // Lock to the v0.5.x line. Loosen when bumping to the next
        // minor. The assertion guards against accidental 0.x-to-1.0
        // jumps that would silently break the SemVer contract
        // documented in `docs/system-design.md` §12.
        assert!(VERSION.starts_with("0.5."));
    }

    #[test]
    fn test_repository_constant_points_at_canonical_github_url() {
        assert_eq!(REPOSITORY, "https://github.com/Mathews-Tom/scrybe");
    }
}
