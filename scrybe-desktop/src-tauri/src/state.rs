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

use std::path::PathBuf;

use scrybe_application::config::ConfigService;
use scrybe_application::{ApplicationError, ScrybeApplication, StorageRoot};

/// Everything Tauri-managed state holds.
pub struct Desktop {
    application: ScrybeApplication,
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
            StorageRoot::new(&snapshot.form.storage_root),
            config.path().to_path_buf(),
        ))
    }

    /// The services over an explicit root and configuration file.
    #[must_use]
    pub fn new(root: StorageRoot, config_path: PathBuf) -> Self {
        Self {
            application: ScrybeApplication::new(root, config_path),
        }
    }

    /// The services every command reads through.
    #[must_use]
    pub const fn application(&self) -> &ScrybeApplication {
        &self.application
    }
}
