// Copyright 2026 Mathews Tom
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//     https://www.apache.org/licenses/LICENSE-2.0

//! Serializable diagnostics types.

use serde::{Deserialize, Serialize};

use crate::identity::SessionRef;

/// How much a finding matters.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    /// Worth showing; nothing is wrong.
    Info,
    /// Something will degrade or surprise the user if left alone.
    Warning,
    /// Something is broken and a documented workflow will not work.
    Error,
}

/// Which part of the system a finding is about.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DiagnosticComponent {
    Config,
    Storage,
    Session,
    Providers,
    Egress,
    AgentAccess,
}

/// Stable identifier for a class of finding.
///
/// Consumers branch on this; the human-readable summary is free to be
/// reworded without breaking them.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DiagnosticCode {
    /// No configuration file exists; built-in defaults are in force.
    ConfigFileAbsent,
    /// The configuration file exists but could not be read or parsed.
    ConfigFileUnreadable,
    /// The configured storage root does not exist yet.
    StorageRootAbsent,
    /// The storage root exists and was scanned.
    StorageRootPresent,
    /// A session is being recorded right now by a live process.
    SessionInProgress,
    /// A `pid.lock` survives with no live process behind it.
    SessionLockStale,
    /// A `.partial` file was left under the storage root.
    OrphanedPartialFile,
    /// A session has durable state that `repair_session` can recover.
    SessionRepairable,
    /// A session left a journal with nothing durable to merge.
    SessionUnfinished,
    /// A session's durable state is present but corrupt.
    SessionFailed,
    /// The configured speech-to-text provider runs locally.
    SttEgressLocal,
    /// The configured speech-to-text provider sends audio off-device.
    SttEgressRemote,
    /// The configured notes provider runs locally.
    LlmEgressLocal,
    /// The configured notes provider sends transcripts off-device.
    LlmEgressRemote,
    /// Read-only agent access over the storage root is enabled.
    AgentAccessEnabled,
}

/// A mutation a user can explicitly choose in response to a finding.
///
/// Every variant names an operation the user must invoke; a diagnostics
/// run only ever reports them.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "action")]
pub enum RecoveryAction {
    /// Create the configured storage root.
    CreateStorageRoot,
    /// Run `repair_session` against one session.
    RepairSession { id: SessionRef },
    /// Delete a `pid.lock` whose owning process is gone.
    RemoveStaleSessionLock { id: SessionRef },
    /// Delete a leftover `.partial` file directly under the root.
    RemoveOrphanedPartial { name: String },
    /// Edit the configuration; not something the service can do for the
    /// user, because only they know the intended value.
    ReviewConfiguration,
}

/// One thing diagnosis found.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct DiagnosticFinding {
    pub code: DiagnosticCode,
    pub severity: Severity,
    pub component: DiagnosticComponent,
    /// One line describing the finding. Never contains transcript,
    /// notes, or audio content.
    pub summary: String,
    /// What would fix it, when anything can.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recovery_action: Option<RecoveryAction>,
    /// Whether acting on this finding changes the system. Always
    /// `false` for a finding whose recovery action is absent or is
    /// [`RecoveryAction::ReviewConfiguration`].
    pub mutation_required: bool,
}

/// Everything one read-only diagnosis run found.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct DiagnosticReport {
    pub findings: Vec<DiagnosticFinding>,
}

impl DiagnosticReport {
    /// Findings at or above `severity`.
    #[must_use]
    pub fn at_least(&self, severity: Severity) -> Vec<&DiagnosticFinding> {
        self.findings
            .iter()
            .filter(|finding| finding.severity >= severity)
            .collect()
    }

    /// How many findings are warnings or errors.
    #[must_use]
    pub fn warning_count(&self) -> usize {
        self.at_least(Severity::Warning).len()
    }

    /// The distinct recovery actions offered, in report order.
    #[must_use]
    pub fn recovery_actions(&self) -> Vec<&RecoveryAction> {
        self.findings
            .iter()
            .filter_map(|finding| finding.recovery_action.as_ref())
            .collect()
    }
}

/// Whether an explicitly requested repair changed anything.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RepairStatus {
    /// The system was changed.
    Applied,
    /// The condition had already been resolved; nothing was changed.
    AlreadyResolved,
}

/// Result of applying one [`RecoveryAction`].
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct RepairApplication {
    pub action: RecoveryAction,
    pub status: RepairStatus,
    pub summary: String,
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    fn finding(severity: Severity, code: DiagnosticCode) -> DiagnosticFinding {
        DiagnosticFinding {
            code,
            severity,
            component: DiagnosticComponent::Storage,
            summary: "example".into(),
            recovery_action: None,
            mutation_required: false,
        }
    }

    #[test]
    fn test_warning_count_excludes_informational_findings() {
        let report = DiagnosticReport {
            findings: vec![
                finding(Severity::Info, DiagnosticCode::StorageRootPresent),
                finding(Severity::Warning, DiagnosticCode::SessionLockStale),
                finding(Severity::Error, DiagnosticCode::SessionFailed),
            ],
        };

        assert_eq!(report.warning_count(), 2);
    }

    #[test]
    fn test_a_recovery_action_serializes_with_its_tagged_discriminant() {
        let action = RecoveryAction::RepairSession {
            id: SessionRef::parse("2026-04-29-1430-acme-01HXYZ").unwrap(),
        };

        let encoded = serde_json::to_value(&action).unwrap();

        assert_eq!(encoded["action"], "repair_session");
        assert_eq!(encoded["id"], "2026-04-29-1430-acme-01HXYZ");
    }
}
