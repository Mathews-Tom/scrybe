// Copyright 2026 Mathews Tom
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//     https://www.apache.org/licenses/LICENSE-2.0

//! Read-only diagnosis, and the separate act of repairing what it found.
//!
//! [`DiagnosticsService::diagnose`] reads. It opens files, stats
//! directories, and asks whether a process is alive; it creates,
//! deletes, and rewrites nothing. A diagnostics screen therefore cannot
//! change the system merely by being opened, and a user can run
//! diagnosis as often as they like without consequence.
//!
//! [`DiagnosticsService::apply_repair`] mutates, and only ever the one
//! [`RecoveryAction`] it is handed. The two are separate calls because
//! the decision to change something belongs to the user, not to the
//! screen that noticed the problem.

use scrybe_core::config::Config;
use scrybe_core::storage::PID_LOCK_NAME;
use url::{Host, Url};

use crate::config::ConfigService;
use crate::diagnostics::contract::{
    DiagnosticCode, DiagnosticComponent, DiagnosticFinding, DiagnosticReport, RecoveryAction,
    RepairApplication, RepairStatus, Severity,
};
use crate::error::{ApplicationError, ErrorCode};
use crate::identity::{PartialFileRef, SessionRef, StorageRoot};
use crate::paging::{PageRequest, MAX_PAGE_LIMIT};
use crate::sessions::{SessionState, SessionSummary};
use crate::{Result, SessionRepository};

/// Diagnosis of one install, and the repairs it can be asked to run.
pub struct DiagnosticsService {
    root: StorageRoot,
}

impl DiagnosticsService {
    /// A service over `root`.
    #[must_use]
    pub const fn new(root: StorageRoot) -> Self {
        Self { root }
    }

    /// Everything currently wrong, or worth knowing, about this install.
    ///
    /// Reads only. Every finding that could be acted on names the
    /// [`RecoveryAction`] a user would have to choose; none is taken
    /// here.
    ///
    /// # Errors
    ///
    /// [`ErrorCode::DiagnosticsUnavailable`] when the storage root
    /// exists but cannot be enumerated.
    pub fn diagnose(
        &self,
        config: &ConfigService,
        sessions: &SessionRepository,
    ) -> Result<DiagnosticReport> {
        let mut findings = Vec::new();

        let loaded = match config.load() {
            Ok(loaded) => {
                if !config.path().exists() {
                    findings.push(finding(
                        DiagnosticCode::ConfigFileAbsent,
                        Severity::Info,
                        DiagnosticComponent::Config,
                        format!(
                            "no configuration file at {}; built-in defaults are in force",
                            config.path().display()
                        ),
                        Some(RecoveryAction::ReviewConfiguration),
                    ));
                }
                loaded
            }
            Err(error) => {
                findings.push(finding(
                    DiagnosticCode::ConfigFileUnreadable,
                    Severity::Error,
                    DiagnosticComponent::Config,
                    format!(
                        "the configuration file at {} could not be read: {}",
                        config.path().display(),
                        error.message()
                    ),
                    Some(RecoveryAction::ReviewConfiguration),
                ));
                Config::default()
            }
        };

        self.diagnose_storage(sessions, &mut findings)?;
        diagnose_egress(&loaded, &mut findings);
        diagnose_agent_access(&loaded, &mut findings);

        Ok(DiagnosticReport { findings })
    }

    /// Performs exactly one repair, chosen by the caller.
    ///
    /// # Errors
    ///
    /// [`ErrorCode::NotApplicable`] when the action is not something
    /// this layer performs, and [`ErrorCode::RepairFailed`] when the
    /// filesystem refuses the change.
    pub fn apply_repair(
        &self,
        action: &RecoveryAction,
        sessions: &SessionRepository,
    ) -> Result<RepairApplication> {
        match action {
            RecoveryAction::CreateStorageRoot => self.create_root(action),
            RecoveryAction::RepairSession { id } => {
                let result = sessions.repair_session(id)?;
                Ok(RepairApplication {
                    action: action.clone(),
                    status: RepairStatus::Applied,
                    summary: format!("session {id} is now {:?}", result.state),
                })
            }
            RecoveryAction::RemoveStaleSessionLock { id } => self.remove_stale_lock(action, id),
            RecoveryAction::RemoveOrphanedPartial { name } => {
                self.remove_orphaned_partial(action, name)
            }
            RecoveryAction::ReviewConfiguration => Err(ApplicationError::new(
                ErrorCode::NotApplicable,
                "reviewing configuration is a decision only the user can make",
            )),
        }
    }

    fn create_root(&self, action: &RecoveryAction) -> Result<RepairApplication> {
        let path = self.root.path();
        if path.is_dir() {
            return Ok(RepairApplication {
                action: action.clone(),
                status: RepairStatus::AlreadyResolved,
                summary: format!("storage root {} already exists", path.display()),
            });
        }
        std::fs::create_dir_all(path).map_err(|source| {
            ApplicationError::new(
                ErrorCode::RepairFailed,
                format!("storage root {} could not be created", path.display()),
            )
            .with_source(source)
        })?;
        Ok(RepairApplication {
            action: action.clone(),
            status: RepairStatus::Applied,
            summary: format!("created storage root {}", path.display()),
        })
    }

    fn remove_stale_lock(
        &self,
        action: &RecoveryAction,
        id: &SessionRef,
    ) -> Result<RepairApplication> {
        let lock = self.root.resolve(id).join(PID_LOCK_NAME);
        match crate::diagnostics::process::lock_owner_alive(&lock) {
            None if !lock.exists() => Ok(RepairApplication {
                action: action.clone(),
                status: RepairStatus::AlreadyResolved,
                summary: format!("session {id} holds no lock"),
            }),
            Some(true) => Err(ApplicationError::new(
                ErrorCode::NotApplicable,
                format!("session {id} is being recorded right now; its lock is not stale"),
            )),
            // The lock exists but carries no readable process id, so
            // whether a recorder still owns it is unknown. Deleting
            // state that cannot be interpreted is the wrong default for
            // a repair: it would clear the way for a second recorder to
            // write into a session the first may still be holding.
            None => Err(ApplicationError::new(
                ErrorCode::NotApplicable,
                format!(
                    "session {id} holds a lock that carries no readable process id; \
                     whether a recorder still owns it cannot be decided here"
                ),
            )),
            Some(false) => {
                std::fs::remove_file(&lock).map_err(|source| {
                    ApplicationError::new(
                        ErrorCode::RepairFailed,
                        format!("the stale lock on session {id} could not be removed"),
                    )
                    .with_source(source)
                })?;
                Ok(RepairApplication {
                    action: action.clone(),
                    status: RepairStatus::Applied,
                    summary: format!("removed the stale lock on session {id}"),
                })
            }
        }
    }

    fn remove_orphaned_partial(
        &self,
        action: &RecoveryAction,
        name: &PartialFileRef,
    ) -> Result<RepairApplication> {
        // No name check here. `PartialFileRef` construction and its
        // `Deserialize` impl already refuse every name that could
        // address something other than a direct child of the root, so
        // the type is the only gate; a second hand-rolled check would
        // be a place for the two to disagree.
        let path = self.root.path().join(name.as_str());
        if !path.exists() {
            return Ok(RepairApplication {
                action: action.clone(),
                status: RepairStatus::AlreadyResolved,
                summary: format!("{name} is already gone"),
            });
        }
        std::fs::remove_file(&path).map_err(|source| {
            ApplicationError::new(
                ErrorCode::RepairFailed,
                format!("{name} could not be removed"),
            )
            .with_source(source)
        })?;
        Ok(RepairApplication {
            action: action.clone(),
            status: RepairStatus::Applied,
            summary: format!("removed orphaned partial download {name}"),
        })
    }

    fn diagnose_storage(
        &self,
        sessions: &SessionRepository,
        findings: &mut Vec<DiagnosticFinding>,
    ) -> Result<()> {
        let path = self.root.path();
        if !path.is_dir() {
            findings.push(finding(
                DiagnosticCode::StorageRootAbsent,
                Severity::Info,
                DiagnosticComponent::Storage,
                format!(
                    "storage root {} does not exist yet; it is created by the first recording",
                    path.display()
                ),
                Some(RecoveryAction::CreateStorageRoot),
            ));
            return Ok(());
        }

        let listed = Self::collect(sessions)?;
        findings.push(finding(
            DiagnosticCode::StorageRootPresent,
            Severity::Info,
            DiagnosticComponent::Storage,
            format!(
                "storage root {} holds {} {}",
                path.display(),
                listed.len(),
                if listed.len() == 1 {
                    "session"
                } else {
                    "sessions"
                }
            ),
            None,
        ));

        self.diagnose_locks(findings);
        for session in &listed {
            Self::diagnose_session(session, findings);
        }
        self.diagnose_partials(findings);
        Ok(())
    }

    fn collect(sessions: &SessionRepository) -> Result<Vec<SessionSummary>> {
        let mut all = Vec::new();
        let mut offset = 0;
        loop {
            let page = sessions.list_sessions(PageRequest::new(offset, MAX_PAGE_LIMIT))?;
            let more = page.has_more;
            offset += page.items.len();
            all.extend(page.items);
            if !more {
                break;
            }
        }
        Ok(all)
    }

    /// Every `pid.lock` under the root, whether or not the folder
    /// holding it classifies as a session.
    ///
    /// `scrybe-core` acquires the lock immediately after
    /// `create_dir_all` and before any journal, audio, or `meta.toml`
    /// exists, so a recorder killed at startup leaves a folder holding
    /// nothing but a lock — which `classify` reports as `NotASession`.
    /// Keying lock detection off the session listing would therefore
    /// hide exactly the lock most likely to be orphaned.
    fn diagnose_locks(&self, findings: &mut Vec<DiagnosticFinding>) {
        let Ok(entries) = std::fs::read_dir(self.root.path()) else {
            return;
        };
        let mut folders: Vec<SessionRef> = entries
            .flatten()
            .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir()))
            .filter_map(|entry| {
                SessionRef::parse(entry.file_name().to_str().unwrap_or_default()).ok()
            })
            .collect();
        folders.sort_by(|left, right| left.as_str().cmp(right.as_str()));

        for id in folders {
            let lock = self.root.resolve(&id).join(PID_LOCK_NAME);
            if !lock.exists() {
                continue;
            }
            findings.push(match crate::diagnostics::process::lock_owner_alive(&lock) {
                Some(true) => finding(
                    DiagnosticCode::SessionInProgress,
                    Severity::Info,
                    DiagnosticComponent::Session,
                    format!("session {id} is being recorded right now"),
                    None,
                ),
                Some(false) => finding(
                    DiagnosticCode::SessionLockStale,
                    Severity::Warning,
                    DiagnosticComponent::Session,
                    format!("session {id} holds a lock whose process is gone"),
                    Some(RecoveryAction::RemoveStaleSessionLock { id: id.clone() }),
                ),
                // Neither stale nor live: the lock carries no process
                // identifier to decide with. It is reported without a
                // recovery action because removing state that cannot be
                // interpreted is a decision only a human can take.
                None => finding(
                    DiagnosticCode::SessionLockUnreadable,
                    Severity::Warning,
                    DiagnosticComponent::Session,
                    format!("session {id} holds a lock that carries no readable process id"),
                    None,
                ),
            });
        }
    }

    fn diagnose_session(session: &SessionSummary, findings: &mut Vec<DiagnosticFinding>) {
        let id = &session.id;

        match session.state {
            SessionState::Complete => {}
            SessionState::Repairable => findings.push(finding(
                DiagnosticCode::SessionRepairable,
                Severity::Info,
                DiagnosticComponent::Session,
                format!("session {id} did not finish and can be recovered"),
                Some(RecoveryAction::RepairSession { id: id.clone() }),
            )),
            SessionState::Unfinished => findings.push(finding(
                DiagnosticCode::SessionUnfinished,
                Severity::Info,
                DiagnosticComponent::Session,
                format!("session {id} did not finish and has nothing durable to recover"),
                None,
            )),
            SessionState::Failed => findings.push(finding(
                DiagnosticCode::SessionFailed,
                Severity::Warning,
                DiagnosticComponent::Session,
                format!("session {id} has durable state that cannot be read"),
                None,
            )),
        }
    }

    fn diagnose_partials(&self, findings: &mut Vec<DiagnosticFinding>) {
        let Ok(entries) = std::fs::read_dir(self.root.path()) else {
            return;
        };
        // A name that cannot form a confined reference is not
        // addressable through this layer, so it is omitted rather than
        // reported with a recovery action no caller could invoke.
        let mut names: Vec<PartialFileRef> = entries
            .flatten()
            .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_file()))
            .filter_map(|entry| {
                PartialFileRef::parse(entry.file_name().to_str().unwrap_or_default()).ok()
            })
            .collect();
        names.sort_by(|left, right| left.as_str().cmp(right.as_str()));
        for name in names {
            findings.push(finding(
                DiagnosticCode::OrphanedPartialFile,
                Severity::Warning,
                DiagnosticComponent::Storage,
                format!(
                    "orphaned partial download {name} under {}",
                    self.root.path().display()
                ),
                Some(RecoveryAction::RemoveOrphanedPartial { name: name.clone() }),
            ));
        }
    }
}

/// Builds a finding, deriving `mutation_required` from the action
/// rather than letting a caller state it independently.
/// Whether acting on `action` changes the system.
///
/// Derived in one place so a finding can never claim a mutation it
/// does not need, or hide one it does.
const fn mutates(action: Option<&RecoveryAction>) -> bool {
    matches!(
        action,
        Some(
            RecoveryAction::CreateStorageRoot
                | RecoveryAction::RepairSession { .. }
                | RecoveryAction::RemoveStaleSessionLock { .. }
                | RecoveryAction::RemoveOrphanedPartial { .. }
        )
    )
}

const fn finding(
    code: DiagnosticCode,
    severity: Severity,
    component: DiagnosticComponent,
    summary: String,
    recovery_action: Option<RecoveryAction>,
) -> DiagnosticFinding {
    DiagnosticFinding {
        mutation_required: mutates(recovery_action.as_ref()),
        code,
        severity,
        component,
        summary,
        recovery_action,
    }
}

fn diagnose_egress(config: &Config, findings: &mut Vec<DiagnosticFinding>) {
    let (stt_code, stt_summary) = if config.stt.provider == "whisper-local" {
        (
            DiagnosticCode::SttEgressLocal,
            "no egress (local Whisper)".to_string(),
        )
    } else {
        config.stt.base_url.as_deref().map_or_else(
            || {
                (
                    DiagnosticCode::SttEgressRemote,
                    format!(
                        "STT provider {} configured without base_url",
                        config.stt.provider
                    ),
                )
            },
            |url| {
                (
                    DiagnosticCode::SttEgressRemote,
                    format!("egress to STT provider {} at {url}", config.stt.provider),
                )
            },
        )
    };
    findings.push(finding(
        stt_code,
        Severity::Info,
        DiagnosticComponent::Egress,
        stt_summary,
        None,
    ));

    let (llm_code, llm_summary) = if is_loopback_url(&config.llm.base_url) {
        (
            DiagnosticCode::LlmEgressLocal,
            format!("no egress (local LLM at {})", config.llm.base_url),
        )
    } else {
        (
            DiagnosticCode::LlmEgressRemote,
            format!(
                "egress to LLM provider {} at {}",
                config.llm.provider, config.llm.base_url
            ),
        )
    };
    findings.push(finding(
        llm_code,
        Severity::Info,
        DiagnosticComponent::Egress,
        llm_summary,
        None,
    ));
}

fn diagnose_agent_access(config: &Config, findings: &mut Vec<DiagnosticFinding>) {
    if !config.agent_access.enabled {
        return;
    }
    findings.push(finding(
        DiagnosticCode::AgentAccessEnabled,
        Severity::Info,
        DiagnosticComponent::AgentAccess,
        "read-only agent access over the storage root is enabled".to_string(),
        None,
    ));
}

fn is_loopback_url(value: &str) -> bool {
    Url::parse(value)
        .ok()
        .and_then(|url| {
            url.host().map(|host| match host {
                Host::Domain(host) => host.eq_ignore_ascii_case("localhost"),
                Host::Ipv4(address) => address.is_loopback(),
                Host::Ipv6(address) => address.is_loopback(),
            })
        })
        .unwrap_or(false)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;
    use std::path::Path;

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
                mutates(found.recovery_action.as_ref()),
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
            let encoded =
                format!("{{\"action\":\"remove_orphaned_partial\",\"name\":\"{escape}\"}}");

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
        let config = install
            .config("schema_version = 1\n\n[llm]\nbase_url = \"https://api.example.com/v1\"\n");

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
}
