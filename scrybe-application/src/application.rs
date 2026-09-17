// Copyright 2026 Mathews Tom
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//     https://www.apache.org/licenses/LICENSE-2.0

//! The services a frontend is handed, assembled once.
//!
//! A frontend constructs this at startup and holds it: one storage
//! root, one configuration file, one session cache, and one recording
//! state model for the whole process. Constructing services separately
//! would let two parts of the same frontend look at different roots or
//! disagree about whether a recording is running.

use std::path::PathBuf;
use std::sync::Arc;

use crate::config::ConfigService;
use crate::diagnostics::DiagnosticsService;
use crate::identity::StorageRoot;
use crate::recording::RecordingController;
use crate::sessions::{SessionReader, SessionRepository};

/// Every application service, over one storage root and one
/// configuration file.
pub struct ScrybeApplication {
    sessions: SessionRepository,
    recording: Arc<RecordingController>,
    config: ConfigService,
    diagnostics: DiagnosticsService,
}

impl ScrybeApplication {
    /// Assembles the services over `root`, reading configuration from
    /// `config_path`.
    #[must_use]
    pub fn new(root: StorageRoot, config_path: impl Into<PathBuf>) -> Self {
        Self {
            sessions: SessionRepository::new(root.clone()),
            recording: Arc::new(RecordingController::new()),
            config: ConfigService::new(config_path),
            diagnostics: DiagnosticsService::new(root),
        }
    }

    /// Listing, search, detail, reads, repair, and notes regeneration.
    #[must_use]
    pub const fn sessions(&self) -> &SessionRepository {
        &self.sessions
    }

    /// The same sessions, without any method that could mutate one.
    ///
    /// This is what a read-only surface is given.
    #[must_use]
    pub const fn session_reader(&self) -> &(dyn SessionReader + '_) {
        &self.sessions
    }

    /// The one process-wide recording state model.
    ///
    /// Also the composition root's observer route:
    /// [`RecordingController::subscribe`] takes `&self`, so a consumer
    /// holding this application can attach an observer here. The
    /// consuming `observing` constructor is unreachable from outside
    /// the crate by design, because an owned controller would be a
    /// second instance.
    #[must_use]
    pub const fn recording(&self) -> &Arc<RecordingController> {
        &self.recording
    }

    /// Configuration reads and GUI-owned updates.
    #[must_use]
    pub const fn config(&self) -> &ConfigService {
        &self.config
    }

    /// Read-only diagnosis, and explicitly requested repairs.
    #[must_use]
    pub const fn diagnostics(&self) -> &DiagnosticsService {
        &self.diagnostics
    }

    /// The configured storage root every session resolves beneath.
    #[must_use]
    pub const fn root(&self) -> &StorageRoot {
        self.sessions.root()
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use crate::paging::PageRequest;
    use crate::recording::RecordingState;
    use pretty_assertions::assert_eq;

    #[test]
    fn test_every_service_sees_the_same_storage_root() {
        let dir = tempfile::tempdir().unwrap();
        let application =
            ScrybeApplication::new(StorageRoot::new(dir.path()), dir.path().join("config.toml"));

        std::fs::create_dir_all(
            dir.path()
                .join("2026-04-29-1430-acme-01HXYZ")
                .join("journal"),
        )
        .unwrap();

        assert_eq!(application.root().path(), dir.path());
        assert_eq!(
            application
                .sessions()
                .list_sessions(PageRequest::first())
                .unwrap()
                .total,
            1
        );
        assert_eq!(
            application
                .diagnostics()
                .diagnose(application.config(), application.sessions())
                .unwrap()
                .findings
                .iter()
                .filter(|finding| finding.summary.contains("2026-04-29-1430-acme-01HXYZ"))
                .count(),
            1
        );
    }

    #[test]
    fn test_the_recording_controller_starts_idle() {
        let dir = tempfile::tempdir().unwrap();
        let application =
            ScrybeApplication::new(StorageRoot::new(dir.path()), dir.path().join("config.toml"));

        assert_eq!(
            application.recording().snapshot().state,
            RecordingState::Idle
        );
    }

    #[test]
    fn test_the_read_only_view_serves_the_same_sessions() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(
            dir.path()
                .join("2026-04-29-1430-acme-01HXYZ")
                .join("journal"),
        )
        .unwrap();
        let application =
            ScrybeApplication::new(StorageRoot::new(dir.path()), dir.path().join("config.toml"));

        assert_eq!(
            SessionReader::list_sessions(application.session_reader(), PageRequest::first())
                .unwrap()
                .total,
            application
                .sessions()
                .list_sessions(PageRequest::first())
                .unwrap()
                .total
        );
    }
}
