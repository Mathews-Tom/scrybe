// Copyright 2026 Mathews Tom
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//     https://www.apache.org/licenses/LICENSE-2.0

//! The one `ScrybeApplication` this process owns.
//!
//! The CLI builds a throwaway application per command, which is right
//! for a one-shot process and wrong for a persistent one: two of them
//! would mean two session caches over the same root and two recording
//! state models that can disagree about whether a recording is running.
//! This host builds exactly one, at startup, and hands out borrows.

use std::path::{Path, PathBuf};

use scrybe_application::config::ConfigService;
use scrybe_application::{ApplicationError, ScrybeApplication, StorageRoot};

use crate::queries::LiveQueries;

/// Everything Tauri-managed state holds.
pub struct Desktop {
    application: ScrybeApplication,
    queries: LiveQueries,
}

impl Desktop {
    /// Resolves the configuration file and storage root, then assembles
    /// the services over them.
    ///
    /// Resolution is the service layer's, not the host's: `discover`
    /// honors `SCRYBE_CONFIG` and the platform convention, and the
    /// storage root comes from that file. The host invents no path of
    /// its own, so a disposable run needs no special host support —
    /// pointing `SCRYBE_CONFIG` at a configuration whose storage root
    /// is disposable is enough.
    ///
    /// # Errors
    ///
    /// Whatever `ConfigService::discover` or `snapshot` reports. A host
    /// that cannot resolve its own configuration has no sensible
    /// default to fall back to; the caller reports and exits.
    pub fn discover() -> Result<Self, ApplicationError> {
        let config = ConfigService::discover()?;
        let snapshot = config.snapshot()?;
        Ok(Self::new(
            StorageRoot::new(expand_home(Path::new(&snapshot.form.storage_root))),
            config.path().to_path_buf(),
        ))
    }

    /// The services over an explicit root and configuration file.
    #[must_use]
    pub fn new(root: StorageRoot, config_path: PathBuf) -> Self {
        Self {
            application: ScrybeApplication::new(root, config_path),
            queries: LiveQueries::default(),
        }
    }

    /// The services every command reads through.
    #[must_use]
    pub const fn application(&self) -> &ScrybeApplication {
        &self.application
    }

    /// The cancellation tokens of the reads currently in flight.
    ///
    /// Held beside the services rather than inside them: which query a
    /// frontend has abandoned is a fact about this host's IPC boundary,
    /// and the service layer is shared with a command-line tool that
    /// has no such boundary.
    #[must_use]
    pub const fn queries(&self) -> &LiveQueries {
        &self.queries
    }
}

/// Resolves a leading `~` against the home directory.
///
/// `StorageRoot` documents that expansion stays with the caller, and
/// until now this host was not doing it — it handed the configuration's
/// value straight through, while the configuration the installer writes
/// holds the literal `~/scrybe`. The installed application therefore
/// resolved a relative path named `~` beneath whatever directory it
/// happened to be launched from, and presented an empty session list
/// over a storage root full of recordings. The command-line tool has
/// had this expansion all along; the two now agree.
///
/// Only a leading `~` or `~/` is expanded. A `~user` form is not: it
/// needs a password-database lookup, no configuration this application
/// writes produces one, and silently treating it as the current user's
/// home would resolve to the wrong person's files.
fn expand_home(root: &Path) -> PathBuf {
    let text = root.to_string_lossy();
    let Some(home) = home_directory() else {
        return root.to_path_buf();
    };
    if text == "~" {
        return home;
    }
    text.strip_prefix("~/")
        .map_or_else(|| root.to_path_buf(), |rest| home.join(rest))
}

fn home_directory() -> Option<PathBuf> {
    directories::BaseDirs::new().map(|dirs| dirs.home_dir().to_path_buf())
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    #[test]
    fn test_a_home_relative_root_resolves_beneath_the_home_directory() {
        let home = home_directory().unwrap();

        let expanded = expand_home(Path::new("~/scrybe"));

        assert_eq!(expanded, home.join("scrybe"));
        assert!(expanded.is_absolute());
    }

    #[test]
    fn test_a_bare_tilde_resolves_to_the_home_directory() {
        assert_eq!(expand_home(Path::new("~")), home_directory().unwrap());
    }

    #[test]
    fn test_an_absolute_root_is_left_alone() {
        let absolute = Path::new("/var/folders/scrybe-q-abc/sessions");

        assert_eq!(expand_home(absolute), absolute);
    }

    #[test]
    fn test_a_relative_root_that_is_not_home_relative_is_left_alone() {
        assert_eq!(expand_home(Path::new("sessions")), Path::new("sessions"));
    }

    /// A `~user` form names someone else's home, and resolving it to
    /// the current user's would point the application at the wrong
    /// person's recordings.
    #[test]
    fn test_another_users_home_is_not_resolved_to_this_one() {
        let other = Path::new("~someone/scrybe");

        assert_eq!(expand_home(other), other);
    }
}
