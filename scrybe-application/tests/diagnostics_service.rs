// Copyright 2026 Mathews Tom
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//     https://www.apache.org/licenses/LICENSE-2.0

//! Diagnosis and repair, exercised through the crate's public surface.
//!
//! These live outside `src/` because `diagnostics/service.rs` had grown
//! past the file-size policy with its inline test module, and every one
//! of these tests reaches the service the way a frontend does — through
//! `DiagnosticsService`, `ConfigService`, and `SessionRepository` — so
//! none of them needed to be inside the module.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::path::Path;

use pretty_assertions::assert_eq;
use scrybe_application::diagnostics::{
    DiagnosticCode, DiagnosticReport, RecoveryAction, RepairStatus,
};
use scrybe_application::{
    ConfigService, DiagnosticsService, ErrorCode, PageRequest, SessionRef, SessionRepository,
    StorageRoot,
};
use scrybe_core::storage::PID_LOCK_NAME;

struct Install {
    dir: tempfile::TempDir,
}

impl Install {
    fn new() -> Self {
        Self {
            dir: tempfile::tempdir().unwrap(),
        }
    }

    fn root(&self) -> StorageRoot {
        StorageRoot::new(self.dir.path().join("sessions"))
    }

    fn ensure_root(&self) -> &Self {
        std::fs::create_dir_all(self.dir.path().join("sessions")).unwrap();
        self
    }

    fn service(&self) -> DiagnosticsService {
        DiagnosticsService::new(self.root())
    }

    fn sessions(&self) -> SessionRepository {
        SessionRepository::new(self.root())
    }

    fn config(&self, body: &str) -> ConfigService {
        let path = self.dir.path().join("config.toml");
        std::fs::write(&path, body).unwrap();
        ConfigService::new(path)
    }

    fn absent_config(&self) -> ConfigService {
        ConfigService::new(self.dir.path().join("absent.toml"))
    }

    fn journal_session(&self, folder: &str) -> &Self {
        self.ensure_root();
        let path = self
            .dir
            .path()
            .join("sessions")
            .join(folder)
            .join("journal");
        std::fs::create_dir_all(&path).unwrap();
        std::fs::write(path.join("manifest.toml"), b"").unwrap();
        self
    }

    /// A folder holding nothing at all, as `scrybe-core` leaves it
    /// between `create_dir_all` and the first journal write.
    fn bare_folder(&self, folder: &str) -> &Self {
        self.ensure_root();
        std::fs::create_dir_all(self.dir.path().join("sessions").join(folder)).unwrap();
        self
    }

    fn lock(&self, folder: &str, pid: &str) -> &Self {
        std::fs::write(
            self.dir
                .path()
                .join("sessions")
                .join(folder)
                .join(PID_LOCK_NAME),
            pid,
        )
        .unwrap();
        self
    }

    fn partial(&self, name: &str) -> &Self {
        self.ensure_root();
        std::fs::write(self.dir.path().join("sessions").join(name), b"").unwrap();
        self
    }

    fn report(&self) -> DiagnosticReport {
        self.service()
            .diagnose(&self.absent_config(), &self.sessions())
            .unwrap()
    }
}

fn codes(report: &DiagnosticReport) -> Vec<DiagnosticCode> {
    report.findings.iter().map(|finding| finding.code).collect()
}

#[test]
fn test_diagnosis_creates_nothing_even_when_it_finds_a_missing_root() {
    let install = Install::new();

    let report = install.report();

    assert!(codes(&report).contains(&DiagnosticCode::StorageRootAbsent));
    assert!(!install.dir.path().join("sessions").exists());
}

#[test]
fn test_diagnosis_leaves_a_stale_lock_in_place_for_the_user_to_decide_on() {
    let install = Install::new();
    install
        .journal_session("2026-04-29-1430-acme-01HXYZ")
        .lock("2026-04-29-1430-acme-01HXYZ", "4294967294");

    let report = install.report();
    let lock = install
        .dir
        .path()
        .join("sessions")
        .join("2026-04-29-1430-acme-01HXYZ")
        .join(PID_LOCK_NAME);

    assert!(codes(&report).contains(&DiagnosticCode::SessionLockStale));
    assert!(lock.exists());
}

#[test]
fn test_a_live_recording_is_reported_in_progress_rather_than_stale() {
    let install = Install::new();
    install.journal_session("2026-04-29-1430-acme-01HXYZ").lock(
        "2026-04-29-1430-acme-01HXYZ",
        &std::process::id().to_string(),
    );

    let report = install.report();

    assert!(codes(&report).contains(&DiagnosticCode::SessionInProgress));
    assert!(!codes(&report).contains(&DiagnosticCode::SessionLockStale));
}

#[test]
fn test_an_orphaned_partial_download_is_reported_and_not_deleted() {
    let install = Install::new();
    install.partial("model.bin.partial");

    let report = install.report();

    assert!(codes(&report).contains(&DiagnosticCode::OrphanedPartialFile));
    assert!(install
        .dir
        .path()
        .join("sessions")
        .join("model.bin.partial")
        .exists());
}

#[test]
fn test_every_finding_that_requires_a_mutation_names_the_action_it_needs() {
    let install = Install::new();
    install
        .journal_session("2026-04-29-1430-acme-01HXYZ")
        .lock("2026-04-29-1430-acme-01HXYZ", "4294967294")
        .partial("model.bin.partial");

    let report = install.report();

    for found in &report.findings {
        assert_eq!(
            found.mutation_required,
            matches!(
                found.recovery_action,
                Some(
                    RecoveryAction::CreateStorageRoot
                        | RecoveryAction::RepairSession { .. }
                        | RecoveryAction::RemoveStaleSessionLock { .. }
                        | RecoveryAction::RemoveOrphanedPartial { .. }
                )
            ),
            "{:?} disagrees with its recovery action",
            found.code
        );
    }
}

#[test]
fn test_no_finding_that_requires_a_mutation_is_acted_on_by_diagnosis() {
    let install = Install::new();
    install
        .journal_session("2026-04-29-1430-acme-01HXYZ")
        .lock("2026-04-29-1430-acme-01HXYZ", "4294967294")
        .partial("model.bin.partial");
    let before = fingerprint(install.dir.path());

    let _ = install.report();
    let _ = install.report();

    assert_eq!(fingerprint(install.dir.path()), before);
}

fn fingerprint(root: &Path) -> Vec<String> {
    let mut entries = Vec::new();
    walk(root, &mut entries);
    entries.sort();
    entries
}

fn walk(dir: &Path, out: &mut Vec<String>) {
    let Ok(read) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in read.flatten() {
        let path = entry.path();
        let len = entry.metadata().map(|meta| meta.len()).unwrap_or_default();
        out.push(format!("{} {len}", path.display()));
        if path.is_dir() {
            walk(&path, out);
        }
    }
}

#[test]
fn test_repairing_a_stale_lock_removes_it_only_when_explicitly_asked() {
    let install = Install::new();
    install
        .journal_session("2026-04-29-1430-acme-01HXYZ")
        .lock("2026-04-29-1430-acme-01HXYZ", "4294967294");
    let id = SessionRef::parse("2026-04-29-1430-acme-01HXYZ").unwrap();
    let lock = install.root().resolve(&id).join(PID_LOCK_NAME);
    assert!(lock.exists());

    let applied = install
        .service()
        .apply_repair(
            &RecoveryAction::RemoveStaleSessionLock { id },
            &install.sessions(),
        )
        .unwrap();

    assert_eq!(applied.status, RepairStatus::Applied);
    assert!(!lock.exists());
}

#[test]
fn test_a_live_recordings_lock_is_refused_rather_than_removed() {
    let install = Install::new();
    install.journal_session("2026-04-29-1430-acme-01HXYZ").lock(
        "2026-04-29-1430-acme-01HXYZ",
        &std::process::id().to_string(),
    );
    let id = SessionRef::parse("2026-04-29-1430-acme-01HXYZ").unwrap();

    let error = install
        .service()
        .apply_repair(
            &RecoveryAction::RemoveStaleSessionLock { id },
            &install.sessions(),
        )
        .unwrap_err();

    assert_eq!(error.code(), ErrorCode::NotApplicable);
}

#[test]
fn test_a_lock_that_cannot_be_interpreted_is_refused_rather_than_removed() {
    let install = Install::new();
    install
        .journal_session("2026-04-29-1430-acme-01HXYZ")
        .lock("2026-04-29-1430-acme-01HXYZ", "not-a-pid");
    let id = SessionRef::parse("2026-04-29-1430-acme-01HXYZ").unwrap();
    let lock = install.root().resolve(&id).join(PID_LOCK_NAME);

    let error = install
        .service()
        .apply_repair(
            &RecoveryAction::RemoveStaleSessionLock { id },
            &install.sessions(),
        )
        .unwrap_err();

    assert_eq!(error.code(), ErrorCode::NotApplicable);
    assert!(lock.exists(), "an uninterpretable lock must survive repair");
}

#[test]
fn test_a_lock_that_cannot_be_interpreted_is_reported_without_a_recovery_action() {
    let install = Install::new();
    install
        .journal_session("2026-04-29-1430-acme-01HXYZ")
        .lock("2026-04-29-1430-acme-01HXYZ", "not-a-pid");

    let report = install.report();
    let codes = codes(&report);

    assert!(codes.contains(&DiagnosticCode::SessionLockUnreadable));
    // Reporting it as stale would offer a repair that is refused,
    // and would claim the owning process is known to be gone.
    assert!(!codes.contains(&DiagnosticCode::SessionLockStale));
    assert!(report
        .findings
        .iter()
        .find(|found| found.code == DiagnosticCode::SessionLockUnreadable)
        .is_some_and(|found| found.recovery_action.is_none()));
}

#[test]
fn test_a_lock_in_a_folder_that_is_not_yet_a_session_is_still_reported() {
    let install = Install::new();
    // `scrybe-core` acquires the lock immediately after creating the
    // folder, before any journal, audio, or `meta.toml` exists. A
    // recorder killed at startup leaves exactly this, and the
    // repository classifies it as `NotASession`.
    install
        .bare_folder("2026-04-29-1430-acme-01HXYZ")
        .lock("2026-04-29-1430-acme-01HXYZ", "4294967294");

    let report = install.report();

    assert!(install
        .sessions()
        .list_sessions(PageRequest::first())
        .unwrap()
        .items
        .is_empty());
    assert!(codes(&report).contains(&DiagnosticCode::SessionLockStale));
}

#[test]
fn test_repairing_an_already_resolved_condition_reports_no_change() {
    let install = Install::new();
    install.ensure_root();

    let applied = install
        .service()
        .apply_repair(&RecoveryAction::CreateStorageRoot, &install.sessions())
        .unwrap();

    assert_eq!(applied.status, RepairStatus::AlreadyResolved);
}

#[test]
fn test_removing_a_partial_refuses_a_name_that_is_not_directly_under_the_root() {
    // The refusal is `PartialFileRef`'s, so the only way a name
    // that escapes the root can reach `apply_repair` is by being
    // deserialized into the action. That boundary is what this
    // asserts; no such `RecoveryAction` value can be constructed in
    // Rust at all.
    for escape in [
        "../outside.partial",
        "sub/outside.partial",
        "/tmp/outside.partial",
        "C:outside.partial",
        "outside.bin",
    ] {
        let encoded = format!("{{\"action\":\"remove_orphaned_partial\",\"name\":\"{escape}\"}}");

        assert!(
            serde_json::from_str::<RecoveryAction>(&encoded).is_err(),
            "{escape} must not deserialize into a recovery action"
        );
    }
}

#[test]
fn test_reviewing_configuration_is_not_a_repair_this_layer_performs() {
    let install = Install::new();

    let error = install
        .service()
        .apply_repair(&RecoveryAction::ReviewConfiguration, &install.sessions())
        .unwrap_err();

    assert_eq!(error.code(), ErrorCode::NotApplicable);
}

#[test]
fn test_a_default_install_reports_both_providers_as_local() {
    let install = Install::new();
    install.ensure_root();
    let config = install.config("schema_version = 1\n");

    let report = install
        .service()
        .diagnose(&config, &install.sessions())
        .unwrap();

    assert!(codes(&report).contains(&DiagnosticCode::SttEgressLocal));
    assert!(codes(&report).contains(&DiagnosticCode::LlmEgressLocal));
}

#[test]
fn test_a_hosted_notes_provider_is_reported_as_egress() {
    let install = Install::new();
    install.ensure_root();
    let config =
        install.config("schema_version = 1\n\n[llm]\nbase_url = \"https://api.example.com/v1\"\n");

    let report = install
        .service()
        .diagnose(&config, &install.sessions())
        .unwrap();

    assert!(codes(&report).contains(&DiagnosticCode::LlmEgressRemote));
}

#[test]
fn test_an_unreadable_configuration_is_a_finding_rather_than_a_failed_diagnosis() {
    let install = Install::new();
    install.ensure_root();
    let config = install.config("[storage\n");

    let report = install
        .service()
        .diagnose(&config, &install.sessions())
        .unwrap();

    assert!(codes(&report).contains(&DiagnosticCode::ConfigFileUnreadable));
    assert_eq!(report.warning_count(), 1);
}
