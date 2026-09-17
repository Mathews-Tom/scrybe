// Copyright 2026 Mathews Tom
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//     https://www.apache.org/licenses/LICENSE-2.0

//! What diagnosis reports about local transcription and local notes,
//! and how a setup screen reads it.
//!
//! The notes probe dials, so a test about it owns the thing it dials.
//! `reachable` binds a listener on an ephemeral loopback port and keeps
//! it alive for the duration; `unreachable` binds one, takes the port,
//! and drops it. Nothing here depends on what happens to be running on
//! the machine — which matters, because a local provider *is* running
//! on some development machines and is never running in CI.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::net::TcpListener;
use std::path::{Path, PathBuf};

use pretty_assertions::assert_eq;
use scrybe_application::diagnostics::{
    Capability, DiagnosticCode, DiagnosticComponent, DiagnosticReport, FacetState, Readiness,
    RecoveryAction, RepairStatus,
};
use scrybe_application::{
    ConfigService, DiagnosticsService, ErrorCode, ModelManager, SessionRepository, StorageRoot,
};

struct Install {
    dir: tempfile::TempDir,
}

impl Install {
    fn new() -> Self {
        let install = Self {
            dir: tempfile::tempdir().unwrap(),
        };
        std::fs::create_dir_all(install.dir.path().join("sessions")).unwrap();
        install
    }

    fn models_dir(&self) -> PathBuf {
        self.dir.path().join("models")
    }

    fn models(&self) -> ModelManager {
        ModelManager::new(self.models_dir())
    }

    fn write_config(&self, body: &str) -> ConfigService {
        let path = self.dir.path().join("config.toml");
        std::fs::write(&path, body).unwrap();
        ConfigService::new(path)
    }

    fn report(&self, body: &str) -> DiagnosticReport {
        let root = StorageRoot::new(self.dir.path().join("sessions"));
        DiagnosticsService::new(root.clone())
            .diagnose(
                &self.write_config(body),
                &SessionRepository::new(root),
                &self.models(),
            )
            .unwrap()
    }

    fn seed_model_partial(&self, name: &str) -> PathBuf {
        std::fs::create_dir_all(self.models_dir()).unwrap();
        let path = self.models_dir().join(name);
        std::fs::write(&path, b"half a model").unwrap();
        path
    }
}

/// Local transcription, with a remote notes endpoint so the notes probe
/// does not fire.
const LOCAL_TRANSCRIPTION: &str = "schema_version = 1\n\n\
                                   [stt]\nprovider = \"whisper-local\"\n\n\
                                   [llm]\nbase_url = \"https://notes.example/v1\"\n";

/// Neither probe fires.
const NEITHER: &str = "schema_version = 1\n\n\
                       [stt]\nprovider = \"openai-compat\"\nbase_url = \"https://stt.example/v1\"\n\n\
                       [llm]\nbase_url = \"https://notes.example/v1\"\n";

fn notes_at(port: u16) -> String {
    format!(
        "schema_version = 1\n\n\
         [stt]\nprovider = \"openai-compat\"\nbase_url = \"https://stt.example/v1\"\n\n\
         [llm]\nbase_url = \"http://127.0.0.1:{port}/v1\"\n"
    )
}

fn codes(report: &DiagnosticReport) -> Vec<DiagnosticCode> {
    report.findings.iter().map(|finding| finding.code).collect()
}

// -- the transcription model -------------------------------------------

#[test]
fn test_a_missing_local_model_blocks_transcription_and_offers_to_install_it() {
    let install = Install::new();

    let report = install.report(LOCAL_TRANSCRIPTION);

    assert!(codes(&report).contains(&DiagnosticCode::TranscriptionModelAbsent));
    let found = report
        .findings
        .iter()
        .find(|finding| finding.code == DiagnosticCode::TranscriptionModelAbsent)
        .unwrap();
    assert_eq!(found.component, DiagnosticComponent::Providers);
    assert!(matches!(
        found.recovery_action,
        Some(RecoveryAction::InstallTranscriptionModel { .. })
    ));
    assert!(found.mutation_required);

    assert_eq!(
        Readiness::from_report(&report).transcription.state,
        FacetState::Blocked
    );
}

#[test]
fn test_a_file_at_the_destination_that_the_catalog_does_not_describe_is_reported_not_replaced() {
    let install = Install::new();
    std::fs::create_dir_all(install.models_dir()).unwrap();
    let destination = install.models_dir().join("ggml-small.en.bin");
    std::fs::write(&destination, b"not a model").unwrap();

    let report = install.report(LOCAL_TRANSCRIPTION);

    assert!(codes(&report).contains(&DiagnosticCode::TranscriptionModelUnreadable));
    assert_eq!(std::fs::read(&destination).unwrap(), b"not a model");
    assert_eq!(
        Readiness::from_report(&report).transcription.state,
        FacetState::Blocked
    );
}

#[test]
fn test_transcription_from_a_hosted_provider_produces_no_model_finding() {
    let install = Install::new();

    let report = install.report(NEITHER);

    let found = codes(&report);
    assert!(!found.contains(&DiagnosticCode::TranscriptionModelAbsent));
    assert!(!found.contains(&DiagnosticCode::TranscriptionModelPresent));
    assert_eq!(
        Readiness::from_report(&report).transcription.state,
        FacetState::NotConfigured
    );
}

// -- model partials ----------------------------------------------------

#[test]
fn test_an_interrupted_download_is_reported_and_not_deleted() {
    let install = Install::new();
    let partial = install.seed_model_partial("ggml-small.en.bin.1-0-2.partial");

    let report = install.report(LOCAL_TRANSCRIPTION);

    assert!(codes(&report).contains(&DiagnosticCode::ModelDownloadPartial));
    assert!(
        partial.exists(),
        "diagnosis deleted an interrupted download"
    );
}

#[test]
fn test_a_model_partial_is_reported_separately_from_a_session_partial() {
    let install = Install::new();
    install.seed_model_partial("ggml-small.en.bin.1-0-2.partial");
    std::fs::write(
        install
            .dir
            .path()
            .join("sessions")
            .join("audio.opus.partial"),
        b"",
    )
    .unwrap();

    let report = install.report(LOCAL_TRANSCRIPTION);

    let found = codes(&report);
    assert!(found.contains(&DiagnosticCode::ModelDownloadPartial));
    assert!(found.contains(&DiagnosticCode::OrphanedPartialFile));
}

#[test]
fn test_removing_a_model_partial_is_a_separate_explicit_call() {
    let install = Install::new();
    let partial = install.seed_model_partial("ggml-small.en.bin.1-0-2.partial");
    let root = StorageRoot::new(install.dir.path().join("sessions"));

    let applied = DiagnosticsService::new(root.clone())
        .apply_repair(
            &RecoveryAction::RemoveModelPartial {
                name: "ggml-small.en.bin.1-0-2.partial".into(),
            },
            &SessionRepository::new(root),
            &install.models(),
        )
        .unwrap();

    assert_eq!(applied.status, RepairStatus::Applied);
    assert!(!partial.exists());
}

#[test]
fn test_a_partial_name_that_escapes_the_models_directory_is_refused() {
    let install = Install::new();
    let root = StorageRoot::new(install.dir.path().join("sessions"));
    // The models directory must exist for this test to exercise what
    // it claims. Unix resolves `..` through the filesystem, so against
    // a models directory that does not exist, `../outside.partial`
    // does not exist either and the pre-confinement `.exists()` check
    // that used to run first would already return early on its own —
    // hiding the escape rather than refusing it. Windows normalizes
    // `..` lexically and resolves the join regardless, which is what
    // exposed the defect there. Creating the directory makes the join
    // resolve on every platform, so the refusal below is confinement,
    // not a directory that merely does not exist yet.
    std::fs::create_dir_all(install.models_dir()).unwrap();
    let outside = install.dir.path().join("outside.partial");
    std::fs::write(&outside, b"not yours").unwrap();

    for escape in ["../outside.partial", "sub/outside.partial", "plain.bin"] {
        let outcome = DiagnosticsService::new(root.clone())
            .apply_repair(
                &RecoveryAction::RemoveModelPartial {
                    name: escape.into(),
                },
                &SessionRepository::new(root.clone()),
                &install.models(),
            )
            .unwrap();

        // A name the manager cannot address names nothing inside the
        // models directory, so the repair reports there is nothing to
        // do rather than reaching for whatever it does name.
        assert_eq!(outcome.status, RepairStatus::AlreadyResolved, "{escape}");
    }
    assert!(
        outside.exists(),
        "a repair reached outside the models directory"
    );
}

// -- installing a model is never a repair -------------------------------

#[test]
fn test_installing_a_model_is_refused_as_a_repair_because_it_carries_no_confirmation() {
    let install = Install::new();
    let root = StorageRoot::new(install.dir.path().join("sessions"));

    let refused = DiagnosticsService::new(root.clone())
        .apply_repair(
            &RecoveryAction::InstallTranscriptionModel {
                id: "whisper-small-en".into(),
            },
            &SessionRepository::new(root),
            &install.models(),
        )
        .unwrap_err();

    assert_eq!(refused.code(), ErrorCode::ModelConfirmationRequired);
    assert!(!install.models_dir().join("ggml-small.en.bin").exists());
}

#[test]
fn test_opening_a_platform_surface_is_not_something_this_layer_performs() {
    let install = Install::new();
    let root = StorageRoot::new(install.dir.path().join("sessions"));
    let service = DiagnosticsService::new(root.clone());

    for action in [
        RecoveryAction::OpenAdvancedConfiguration,
        RecoveryAction::OpenSystemSettings {
            capability: Capability::Microphone,
        },
    ] {
        let refused = service
            .apply_repair(
                &action,
                &SessionRepository::new(root.clone()),
                &install.models(),
            )
            .unwrap_err();

        assert_eq!(refused.code(), ErrorCode::NotApplicable);
    }
}

// -- the notes endpoint -------------------------------------------------

#[test]
fn test_a_local_notes_provider_that_answers_is_ready() {
    let install = Install::new();
    // Held for the duration: a bound listener is what makes this
    // deterministic rather than a guess about the machine.
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();

    let report = install.report(&notes_at(port));

    assert!(codes(&report).contains(&DiagnosticCode::NotesProviderReachable));
    assert_eq!(
        Readiness::from_report(&report).notes.state,
        FacetState::Ready
    );
    drop(listener);
}

#[test]
fn test_a_local_notes_provider_that_does_not_answer_is_a_warning_not_an_error() {
    let install = Install::new();
    // Bound to claim an unused port, then released, so nothing is
    // listening on it when the probe runs.
    let port = {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.local_addr().unwrap().port()
    };

    let report = install.report(&notes_at(port));

    let found = report
        .findings
        .iter()
        .find(|finding| finding.code == DiagnosticCode::NotesProviderUnreachable)
        .unwrap();
    assert_eq!(found.component, DiagnosticComponent::Providers);
    assert_eq!(
        found.severity,
        scrybe_application::diagnostics::Severity::Warning
    );
}

#[test]
fn test_a_remote_notes_endpoint_is_never_dialled() {
    let install = Install::new();

    let report = install.report(NEITHER);

    let found = codes(&report);
    assert!(!found.contains(&DiagnosticCode::NotesProviderReachable));
    assert!(!found.contains(&DiagnosticCode::NotesProviderUnreachable));
    assert_eq!(
        Readiness::from_report(&report).notes.state,
        FacetState::NotConfigured
    );
}

// -- readiness is five answers, not one ---------------------------------

#[test]
fn test_recording_may_be_ready_while_notes_are_visibly_unavailable() {
    let install = Install::new();
    // Nothing listening, so notes are blocked; transcription is not
    // configured from a local model, so it is not the blocker either.
    let port = {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.local_addr().unwrap().port()
    };
    let body = format!(
        "schema_version = 1\n\n\
         [stt]\nprovider = \"openai-compat\"\nbase_url = \"https://stt.example/v1\"\n\n\
         [llm]\nbase_url = \"http://127.0.0.1:{port}/v1\"\n"
    );

    let readiness = Readiness::from_report(&install.report(&body));

    assert_eq!(readiness.notes.state, FacetState::Blocked);
    assert_eq!(readiness.storage.state, FacetState::Ready);
    assert!(
        !readiness.notes.summary.is_empty(),
        "an unavailable notes provider says nothing about why"
    );
}

/// Nothing in this layer probes a capture permission, so the facet must
/// not claim one either way. It used to report `Ready` off an empty
/// evidence list, which told an installation whose microphone had been
/// refused that it was ready to record.
#[test]
fn test_capture_is_reported_as_unverified_rather_than_claimed_ready() {
    let install = Install::new();

    let readiness = Readiness::from_report(&install.report(LOCAL_TRANSCRIPTION));

    assert_eq!(readiness.capture.state, FacetState::Unverified);
    assert!(
        readiness.capture.codes.is_empty(),
        "capture cited evidence it cannot have: {:?}",
        readiness.capture.codes
    );
    assert!(
        readiness.capture.summary.contains("not checked here"),
        "the capture summary does not say what was left unchecked: {:?}",
        readiness.capture.summary
    );
}

/// Unverified is not a blocker. Refusing to record on the strength of a
/// permission nothing measured would replace one wrong claim with
/// another, and macOS raises its own dialog where the grant is needed.
#[test]
fn test_an_unverified_capture_facet_does_not_stop_a_recording() {
    let install = Install::new();
    let port = {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.local_addr().unwrap().port()
    };
    let body = format!(
        "schema_version = 1\n\n\
         [stt]\nprovider = \"openai-compat\"\nbase_url = \"https://stt.example/v1\"\n\n\
         [llm]\nbase_url = \"http://127.0.0.1:{port}/v1\"\n"
    );

    let readiness = Readiness::from_report(&install.report(&body));

    assert_eq!(readiness.capture.state, FacetState::Unverified);
    assert!(readiness.can_record());
}

#[test]
fn test_a_blocked_transcription_facet_stops_recording_but_a_blocked_notes_facet_does_not() {
    let install = Install::new();

    let blocked = Readiness::from_report(&install.report(LOCAL_TRANSCRIPTION));
    assert_eq!(blocked.transcription.state, FacetState::Blocked);
    assert!(!blocked.can_record());

    let port = {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.local_addr().unwrap().port()
    };
    let notes_only = Readiness::from_report(&install.report(&notes_at(port)));
    assert_eq!(notes_only.notes.state, FacetState::Blocked);
    assert!(
        notes_only.can_record(),
        "an unavailable notes provider blocked recording"
    );
}

#[test]
fn test_egress_is_disclosed_rather_than_treated_as_a_fault() {
    let install = Install::new();

    let readiness = Readiness::from_report(&install.report(NEITHER));

    assert_eq!(readiness.egress.state, FacetState::Ready);
    assert!(readiness.egress.summary.contains("off this device"));
}

#[test]
fn test_every_facet_names_the_findings_it_was_derived_from() {
    let install = Install::new();

    let readiness = Readiness::from_report(&install.report(LOCAL_TRANSCRIPTION));

    assert!(readiness
        .transcription
        .codes
        .contains(&DiagnosticCode::TranscriptionModelAbsent));
    assert!(readiness.egress.codes.len() >= 2);
}

// -- diagnosis still mutates nothing -------------------------------------

#[test]
fn test_diagnosing_the_new_providers_mutates_nothing() {
    let install = Install::new();
    install.seed_model_partial("ggml-small.en.bin.1-0-2.partial");
    // The configuration is fixture setup, so it is in place before the
    // fingerprint; what follows sees only what diagnosis did.
    install.write_config(LOCAL_TRANSCRIPTION);
    let before = fingerprint(install.dir.path());

    let _ = install.report(LOCAL_TRANSCRIPTION);
    let _ = install.report(LOCAL_TRANSCRIPTION);

    assert_eq!(fingerprint(install.dir.path()), before);
}

fn fingerprint(root: &Path) -> Vec<String> {
    let mut entries = Vec::new();
    walk(root, &mut entries);
    entries.sort();
    entries
}

fn walk(dir: &Path, out: &mut Vec<String>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let length = std::fs::metadata(&path).map_or(0, |meta| meta.len());
        out.push(format!("{} {length}", path.display()));
        if path.is_dir() {
            walk(&path, out);
        }
    }
}

/// `[stt].model` is a free-text field and the catalog holds one entry,
/// so readiness computed for the catalog's artifact would answer for a
/// file the runtime is never going to open.
#[test]
fn test_a_configured_model_that_is_not_the_managed_one_is_reported_on_its_own_terms() {
    let install = Install::new();
    let body = "schema_version = 1\n\n\
                [stt]\nprovider = \"whisper-local\"\nmodel = \"medium.en\"\n\n\
                [llm]\nbase_url = \"https://notes.example/v1\"\n";
    // The managed artifact is installed and valid. It is still not what
    // this configuration loads, so it must not be reported as if it were.
    install.seed_model_partial("ggml-small.en.bin");

    let report = install.report(body);

    let found = report
        .findings
        .iter()
        .find(|finding| finding.code == DiagnosticCode::TranscriptionModelAbsent)
        .expect("no finding about the model that will actually load");
    assert!(
        found.summary.contains("ggml-medium.en.bin"),
        "the finding does not name the model that was checked: {:?}",
        found.summary
    );
    assert!(
        !codes(&report).contains(&DiagnosticCode::TranscriptionModelPresent),
        "the managed artifact was reported ready for a configuration that does not load it"
    );
    assert_eq!(
        found.recovery_action,
        Some(RecoveryAction::ReviewConfiguration),
        "installing the managed model was offered, which would not change what loads"
    );
}

#[test]
fn test_a_configured_model_that_is_present_is_reported_present_and_unverified() {
    let install = Install::new();
    let body = "schema_version = 1\n\n\
                [stt]\nprovider = \"whisper-local\"\nmodel = \"medium.en\"\n\n\
                [llm]\nbase_url = \"https://notes.example/v1\"\n";
    install.seed_model_partial("ggml-medium.en.bin");

    let report = install.report(body);

    let found = report
        .findings
        .iter()
        .find(|finding| finding.code == DiagnosticCode::TranscriptionModelPresent)
        .expect("a present configured model produced no finding");
    assert!(
        found.summary.contains("ggml-medium.en.bin"),
        "the finding does not name the model that was checked: {:?}",
        found.summary
    );
    assert!(
        found.summary.contains("not checked against the catalog"),
        "a model outside the catalog was reported as if it had been verified: {:?}",
        found.summary
    );
}
