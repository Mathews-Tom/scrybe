// Copyright 2026 Mathews Tom
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//     https://www.apache.org/licenses/LICENSE-2.0

//! Resolution, preflight, and what a refused recording leaves behind.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::path::{Path, PathBuf};

use scrybe_application::config::ConfigService;
use scrybe_application::error::ErrorCode;
use scrybe_application::recording::{
    CaptureCapability, CaptureSource, CaptureSupport, CheckOutcome, NotesBackend, PreflightCheck,
    RecordingOverrides, RecordingPlan, RecordingState, SystemBackend, TranscriptionModel,
};
use scrybe_application::{ScrybeApplication, StorageRoot};
use scrybe_core::config::Config;

/// A build that carries everything, so a check that fails does so
/// because of the plan rather than because of the feature set.
const fn everything() -> CaptureSupport {
    CaptureSupport {
        capture: CaptureCapability::MicrophoneAndSystemAudio,
        transcription_model: true,
        notes_provider: true,
    }
}

fn config_with(body: &str) -> Config {
    toml::from_str(body).unwrap()
}

fn plan_over(root: &Path) -> RecordingPlan {
    RecordingPlan::resolve(
        &Config::default(),
        None,
        &RecordingOverrides {
            root: Some(root.to_path_buf()),
            ..RecordingOverrides::default()
        },
    )
    .unwrap()
}

#[test]
fn test_an_empty_configuration_resolves_to_the_source_that_needs_no_hardware() {
    let plan =
        RecordingPlan::resolve(&Config::default(), None, &RecordingOverrides::default()).unwrap();

    assert_eq!(plan.source, CaptureSource::Synthetic);
    assert_eq!(plan.notes, NotesBackend::Stub);
    assert_eq!(plan.transcription, TranscriptionModel::Stub);
    assert_eq!(plan.system_backend, SystemBackend::Sck);
}

#[test]
fn test_a_surface_override_wins_over_the_configured_source() {
    let config = config_with("[record]\nsource = \"mic\"\n");

    let plan = RecordingPlan::resolve(
        &config,
        None,
        &RecordingOverrides {
            source: Some(CaptureSource::Synthetic),
            ..RecordingOverrides::default()
        },
    )
    .unwrap();

    assert_eq!(plan.source, CaptureSource::Synthetic);
}

/// A misspelled source is refused rather than replaced with the
/// default. Falling back would record the in-process tone and hand the
/// reader a session they believe is their meeting.
#[test]
fn test_a_source_the_release_does_not_define_is_refused_rather_than_defaulted() {
    let config = config_with("[record]\nsource = \"microphone\"\n");

    let error = RecordingPlan::resolve(&config, None, &RecordingOverrides::default()).unwrap_err();

    assert_eq!(error.payload().code, ErrorCode::ConfigInvalid);
    assert!(error.payload().message.contains("microphone"));
}

#[test]
fn test_a_notes_backend_the_release_does_not_define_is_refused() {
    let config = config_with("[record]\nllm = \"anthropic\"\n");

    let error = RecordingPlan::resolve(&config, None, &RecordingOverrides::default()).unwrap_err();

    assert_eq!(error.payload().code, ErrorCode::ConfigInvalid);
}

#[test]
fn test_a_home_relative_storage_root_resolves_beneath_the_supplied_home() {
    let config = config_with("[storage]\nroot = \"~/scrybe\"\n");

    let plan = RecordingPlan::resolve(
        &config,
        Some(Path::new("/tmp/home")),
        &RecordingOverrides::default(),
    )
    .unwrap();

    assert_eq!(plan.root, PathBuf::from("/tmp/home/scrybe"));
}

/// `~someone` names another person's home. Rewriting it to this user's
/// would point the recorder at the wrong reader's directory.
#[test]
fn test_another_users_home_is_left_alone_in_a_storage_root() {
    let config = config_with("[storage]\nroot = \"~someone/scrybe\"\n");

    let plan = RecordingPlan::resolve(
        &config,
        Some(Path::new("/tmp/home")),
        &RecordingOverrides::default(),
    )
    .unwrap();

    assert_eq!(plan.root, PathBuf::from("~someone/scrybe"));
}

/// The synthetic source stays on the stub even with a model configured:
/// it generates a fixed tone, so a model load would cost seconds and
/// produce nothing a reader wants.
#[test]
fn test_the_synthetic_source_stays_on_the_stub_with_a_model_configured() {
    let config =
        config_with("[stt]\nprovider = \"whisper-local\"\nmodel = \"/models/small.bin\"\n");

    let plan = RecordingPlan::resolve(&config, None, &RecordingOverrides::default()).unwrap();

    assert_eq!(plan.transcription, TranscriptionModel::Stub);
}

/// The configured model has to be an absolute path for the resolver to
/// take it verbatim, and what counts as absolute differs by platform: a
/// leading `/` is absolute on Unix and is not on Windows, where the
/// resolver would fall through to the platform data directory instead.
/// A temporary directory is absolute on both.
#[test]
fn test_a_real_source_takes_the_configured_whisper_model() {
    let directory = tempfile::tempdir().unwrap();
    let model = directory.path().join("small.bin");
    let config = config_with(&format!(
        "[record]\nsource = \"mic\"\n[stt]\nprovider = \"whisper-local\"\nmodel = {}\n",
        toml::Value::from(model.to_str().unwrap())
    ));

    let plan = RecordingPlan::resolve(&config, None, &RecordingOverrides::default()).unwrap();

    assert_eq!(plan.transcription, TranscriptionModel::Whisper(model));
}

#[test]
fn test_every_check_is_answered_once_and_in_a_stable_order() {
    let directory = tempfile::tempdir().unwrap();

    let report =
        scrybe_application::recording::preflight(&plan_over(directory.path()), everything(), None);

    let checks: Vec<PreflightCheck> = report
        .findings
        .iter()
        .map(|finding| finding.check)
        .collect();
    assert_eq!(
        checks,
        vec![
            PreflightCheck::Configuration,
            PreflightCheck::Permissions,
            PreflightCheck::Device,
            PreflightCheck::Provider,
            PreflightCheck::Model,
            PreflightCheck::Storage,
            PreflightCheck::Capture,
        ]
    );
}

/// The one check this release cannot answer. It must never claim to
/// have measured a grant nothing here measures, and it must never
/// block — a refusal on the strength of an unmeasured permission would
/// be as wrong as an assurance.
#[test]
fn test_the_permission_check_says_it_did_not_check_and_never_blocks() {
    let directory = tempfile::tempdir().unwrap();
    let mut plan = plan_over(directory.path());
    plan.source = CaptureSource::Mic;

    let report = scrybe_application::recording::preflight(&plan, everything(), Some(&[]));

    let permissions = report
        .findings
        .iter()
        .find(|finding| finding.check == PreflightCheck::Permissions)
        .unwrap();
    assert_eq!(permissions.outcome, CheckOutcome::Unverified);
    assert!(permissions.summary.starts_with("not checked"));
    assert!(!permissions.summary.to_lowercase().contains("granted"));
}

#[test]
fn test_a_build_without_the_microphone_adapter_refuses_the_microphone_source() {
    let directory = tempfile::tempdir().unwrap();
    let mut plan = plan_over(directory.path());
    plan.source = CaptureSource::Mic;

    let report = scrybe_application::recording::preflight(
        &plan,
        CaptureSupport {
            capture: CaptureCapability::SyntheticOnly,
            ..everything()
        },
        None,
    );

    assert!(!report.can_record());
    let blocking: Vec<PreflightCheck> = report
        .blocking()
        .iter()
        .map(|finding| finding.check)
        .collect();
    assert_eq!(blocking, vec![PreflightCheck::Capture]);
}

/// Linking the microphone adapter is not linking the system-audio one.
#[test]
fn test_a_microphone_only_build_refuses_the_mic_plus_system_source() {
    let directory = tempfile::tempdir().unwrap();
    let mut plan = plan_over(directory.path());
    plan.source = CaptureSource::MicSystem;

    let report = scrybe_application::recording::preflight(
        &plan,
        CaptureSupport {
            capture: CaptureCapability::Microphone,
            ..everything()
        },
        Some(&["default".to_string()]),
    );

    let capture = report
        .blocking()
        .into_iter()
        .find(|finding| finding.check == PreflightCheck::Capture)
        .unwrap();
    assert!(capture.summary.contains("system-audio capture"));
}

#[test]
fn test_a_configured_input_device_that_is_absent_blocks() {
    let directory = tempfile::tempdir().unwrap();
    let mut plan = plan_over(directory.path());
    plan.source = CaptureSource::Mic;
    plan.input_device = Some("BuiltInMicrophoneDevice".to_string());

    let report = scrybe_application::recording::preflight(
        &plan,
        everything(),
        Some(&["ExternalInterface".to_string()]),
    );

    let device = report
        .blocking()
        .into_iter()
        .find(|finding| finding.check == PreflightCheck::Device)
        .unwrap();
    assert!(device.summary.contains("BuiltInMicrophoneDevice"));
}

/// A build that cannot enumerate devices reports that, rather than
/// reporting the configured device as missing.
#[test]
fn test_a_build_that_cannot_enumerate_devices_reports_unverified_not_missing() {
    let directory = tempfile::tempdir().unwrap();
    let mut plan = plan_over(directory.path());
    plan.source = CaptureSource::Mic;
    plan.input_device = Some("BuiltInMicrophoneDevice".to_string());

    let report = scrybe_application::recording::preflight(&plan, everything(), None);

    let device = report
        .findings
        .iter()
        .find(|finding| finding.check == PreflightCheck::Device)
        .unwrap();
    assert_eq!(device.outcome, CheckOutcome::Unverified);
    assert!(report.can_record());
}

#[test]
fn test_a_missing_transcription_model_blocks_and_names_the_path() {
    let directory = tempfile::tempdir().unwrap();
    let mut plan = plan_over(directory.path());
    plan.transcription = TranscriptionModel::Whisper(directory.path().join("absent.bin"));

    let report = scrybe_application::recording::preflight(&plan, everything(), None);

    let model = report
        .blocking()
        .into_iter()
        .find(|finding| finding.check == PreflightCheck::Model)
        .unwrap();
    assert!(model.summary.contains("absent.bin"));
}

#[test]
fn test_a_present_transcription_model_passes() {
    let directory = tempfile::tempdir().unwrap();
    let model = directory.path().join("small.bin");
    std::fs::write(&model, b"ggml").unwrap();
    let mut plan = plan_over(directory.path());
    plan.transcription = TranscriptionModel::Whisper(model);

    let report = scrybe_application::recording::preflight(&plan, everything(), None);

    assert!(report.can_record());
}

/// A build with no model runtime cannot load a model however present
/// the file is, and saying "the file is there" would be true and
/// useless.
#[test]
fn test_a_present_model_still_blocks_when_the_build_carries_no_runtime() {
    let directory = tempfile::tempdir().unwrap();
    let model = directory.path().join("small.bin");
    std::fs::write(&model, b"ggml").unwrap();
    let mut plan = plan_over(directory.path());
    plan.transcription = TranscriptionModel::Whisper(model);

    let report = scrybe_application::recording::preflight(
        &plan,
        CaptureSupport {
            transcription_model: false,
            ..everything()
        },
        None,
    );

    let model = report
        .blocking()
        .into_iter()
        .find(|finding| finding.check == PreflightCheck::Model)
        .unwrap();
    assert!(model.summary.contains("no model runtime"));
}

#[test]
fn test_a_notes_provider_the_build_does_not_carry_blocks() {
    let directory = tempfile::tempdir().unwrap();
    let mut plan = plan_over(directory.path());
    plan.notes = NotesBackend::OpenAiCompat;

    let report = scrybe_application::recording::preflight(
        &plan,
        CaptureSupport {
            notes_provider: false,
            ..everything()
        },
        None,
    );

    let provider = report
        .blocking()
        .into_iter()
        .find(|finding| finding.check == PreflightCheck::Provider)
        .unwrap();
    assert!(provider.summary.contains("openai-compat"));
}

/// A root that does not exist yet is fine — the session run creates it.
/// A root whose nearest existing ancestor is a regular file is not, and
/// discovering that halfway through a recording would cost the
/// recording.
#[test]
fn test_a_storage_root_underneath_a_regular_file_blocks() {
    let directory = tempfile::tempdir().unwrap();
    let file = directory.path().join("not-a-directory");
    std::fs::write(&file, b"x").unwrap();
    let plan = plan_over(&file.join("sessions"));

    let report = scrybe_application::recording::preflight(&plan, everything(), None);

    let storage = report
        .blocking()
        .into_iter()
        .find(|finding| finding.check == PreflightCheck::Storage)
        .unwrap();
    assert!(storage.summary.contains("not a directory"));
}

#[test]
fn test_a_storage_root_that_does_not_exist_yet_passes_on_its_writable_parent() {
    let directory = tempfile::tempdir().unwrap();
    let plan = plan_over(&directory.path().join("nested").join("sessions"));

    let report = scrybe_application::recording::preflight(&plan, everything(), None);

    assert!(report.can_record());
}

/// Preflight reads. A reader who is told their configuration is wrong
/// should not find a directory tree that was created to tell them.
#[test]
fn test_preflight_creates_nothing_under_a_storage_root_that_does_not_exist() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("sessions");
    let plan = plan_over(&root);

    let _ = scrybe_application::recording::preflight(&plan, everything(), None);

    assert!(!root.exists());
}

/// Every blocking reason in one attempt, so a reader fixes both rather
/// than fixing one and discovering the other.
#[test]
fn test_a_refusal_names_every_blocking_check_not_only_the_first() {
    let directory = tempfile::tempdir().unwrap();
    let mut plan = plan_over(directory.path());
    plan.transcription = TranscriptionModel::Whisper(directory.path().join("absent.bin"));
    plan.notes = NotesBackend::OpenAiCompat;

    let error = scrybe_application::recording::preflight(
        &plan,
        CaptureSupport {
            notes_provider: false,
            ..everything()
        },
        None,
    )
    .into_result()
    .unwrap_err();

    let message = error.payload().message;
    assert_eq!(error.payload().code, ErrorCode::PreflightFailed);
    assert!(message.contains("provider:"), "{message}");
    assert!(message.contains("model:"), "{message}");
}

/// The guarantee a surface relies on, asserted on the filesystem rather
/// than on the returned error: a refused recording leaves no session
/// folder, and because preflight writes nothing, no storage root
/// either.
#[test]
fn test_a_refused_recording_creates_no_session_and_no_storage_root() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("sessions");
    let config_path = directory.path().join("config.toml");
    std::fs::write(
        &config_path,
        format!(
            "[storage]\nroot = \"{}\"\n[record]\nsource = \"mic\"\n",
            root.display()
        ),
    )
    .unwrap();
    let application = ScrybeApplication::new(StorageRoot::new(&root), &config_path);

    let refusal = scrybe_application::recording::begin(
        application.recording(),
        &ConfigService::new(&config_path),
        None,
        CaptureSupport {
            capture: CaptureCapability::SyntheticOnly,
            ..everything()
        },
        None,
        &RecordingOverrides::default(),
    )
    .unwrap_err();

    assert_eq!(refusal.error.payload().code, ErrorCode::PreflightFailed);
    assert!(
        !root.exists(),
        "a refused recording created {}",
        root.display()
    );
    assert_eq!(
        std::fs::read_dir(directory.path())
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect::<Vec<_>>(),
        vec![std::ffi::OsString::from("config.toml")]
    );
}

/// A refusal is visible: the controller passes through the attempt and
/// settles back to idle, so a surface that was watching sees it
/// happened rather than nothing at all.
#[test]
fn test_a_refused_recording_settles_the_controller_back_to_idle() {
    let directory = tempfile::tempdir().unwrap();
    let config_path = directory.path().join("config.toml");
    std::fs::write(&config_path, "[record]\nsource = \"mic\"\n").unwrap();
    let application = ScrybeApplication::new(StorageRoot::new(directory.path()), &config_path);

    let _ = scrybe_application::recording::begin(
        application.recording(),
        &ConfigService::new(&config_path),
        None,
        CaptureSupport {
            capture: CaptureCapability::SyntheticOnly,
            ..everything()
        },
        None,
        &RecordingOverrides::default(),
    )
    .unwrap_err();

    let snapshot = application.recording().snapshot();
    assert_eq!(snapshot.state, RecordingState::Idle);
    assert!(snapshot.failure.is_none());
}

/// A passing preflight leaves the controller preparing, not recording:
/// the single monotonic origin starts where capture does, so a
/// permission prompt and a model load are not counted as recorded time.
#[test]
fn test_a_passing_preflight_leaves_the_controller_preparing() {
    let directory = tempfile::tempdir().unwrap();
    let config_path = directory.path().join("config.toml");
    let application = ScrybeApplication::new(StorageRoot::new(directory.path()), &config_path);

    let plan = scrybe_application::recording::begin(
        application.recording(),
        &ConfigService::new(&config_path),
        None,
        everything(),
        None,
        &RecordingOverrides {
            root: Some(directory.path().to_path_buf()),
            ..RecordingOverrides::default()
        },
    )
    .unwrap();

    assert_eq!(plan.source, CaptureSource::Synthetic);
    let snapshot = application.recording().snapshot();
    assert_eq!(snapshot.state, RecordingState::Preparing);
    assert_eq!(snapshot.elapsed_ms, 0);
}

/// A second start is a state conflict, not a preflight failure — and it
/// must not run preflight at all, because the answer a reader needs is
/// "one is already running".
#[test]
fn test_a_second_start_is_refused_as_a_conflict_without_running_preflight() {
    let directory = tempfile::tempdir().unwrap();
    let config_path = directory.path().join("config.toml");
    let application = ScrybeApplication::new(StorageRoot::new(directory.path()), &config_path);
    let overrides = RecordingOverrides {
        root: Some(directory.path().to_path_buf()),
        ..RecordingOverrides::default()
    };
    let service = ConfigService::new(&config_path);
    scrybe_application::recording::begin(
        application.recording(),
        &service,
        None,
        everything(),
        None,
        &overrides,
    )
    .unwrap();

    let refusal = scrybe_application::recording::begin(
        application.recording(),
        &service,
        None,
        everything(),
        None,
        &overrides,
    )
    .unwrap_err();

    assert_eq!(
        refusal.error.payload().code,
        ErrorCode::RecordingStateConflict
    );
    assert!(refusal.report.findings.is_empty());
    assert_eq!(
        application.recording().snapshot().state,
        RecordingState::Preparing
    );
}
