// Copyright 2026 Mathews Tom
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//     https://www.apache.org/licenses/LICENSE-2.0

//! The commands guided setup and settings reach Rust through.
//!
//! Every one forwards to a service method and converts the result into
//! a narrowed transport type. None interprets a path, opens a file,
//! parses configuration, decides what is ready, or verifies a model:
//! those are the service layer's, and duplicating any of them here
//! would be a second implementation free to disagree with the one the
//! command-line tool uses.
//!
//! Three properties of this surface are worth stating, because each is
//! a thing a plausible design would get wrong.
//!
//! Reading and mutating are different commands. `diagnostics_report`
//! reads; `apply_recovery` mutates, and only the one action it is
//! handed. Opening a diagnostics screen calls the first and never the
//! second, so it cannot change the system by being opened.
//!
//! The model plan and the model install are different commands too, in
//! that order. `model_offer` reads the catalog and the filesystem and
//! returns everything a confirmation prompt renders; `install_model`
//! refuses without a confirmation naming the digest that prompt showed.
//! A frontend that skipped the prompt could not construct an acceptable
//! confirmation, because it would not know the digest.
//!
//! `open_system_settings` and `open_advanced_configuration` hand the
//! user to the platform. They take no argument that names a path or a
//! URL: the destinations are derived in Rust from a capability
//! enumeration and from the configuration service's own path, so the
//! frontend cannot ask this host to open something it chose.

// Tauri resolves a command's managed state by value, so every command
// below takes `State` that way whether or not it consumes it.
#![allow(clippy::needless_pass_by_value)]

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use scrybe_application::cancellation::CancellationToken;
use scrybe_application::config::{ConfigUpdate, ConfigValue};
use scrybe_application::diagnostics::{Capability, Readiness, RecoveryAction};
use scrybe_application::models::{DownloadProgress, ModelConfirmation};
use scrybe_application::{ApplicationError, ErrorCode, SessionRef};
use tauri::{Emitter, Manager, State};

use crate::contract::setup::{
    DiagnosticRows, ModelOffer, ModelOutcome, ModelProgress, RecoveryActionView, RepairOutcome,
    SettingsChange, SettingsForm, MODEL_PROGRESS_EVENT,
};
use crate::contract::CommandFailure;
use crate::state::Desktop;

/// The setup surface's commands, appended to the host's list.
pub const COMMANDS: &[&str] = &[
    "settings_form",
    "apply_settings",
    "diagnostics_report",
    "apply_recovery",
    "readiness_report",
    "model_offer",
    "install_model",
    "cancel_model_install",
    "open_system_settings",
    "open_advanced_configuration",
];

/// Whichever model acquisition is in flight, so a cancel command can
/// reach it.
///
/// One at a time by construction: `install_model` replaces the token
/// before it starts, and a second install of the same model while one
/// is running would be a second `.partial`, which the manager already
/// makes safe. The flag alongside it is what makes a cancel arriving
/// before the install a cancel of that install rather than of nothing.
#[derive(Default)]
pub struct ModelAcquisition {
    token: std::sync::Mutex<CancellationToken>,
    running: AtomicBool,
}

impl ModelAcquisition {
    /// A fresh token, registered as the one a cancel reaches.
    ///
    /// `pub` because the debug-only qualification probe drives the same
    /// manager through the same one-at-a-time discipline; a second
    /// token registry for the harness would be a second thing that
    /// could disagree with this one about what is running.
    pub fn begin(&self) -> CancellationToken {
        let token = CancellationToken::new();
        if let Ok(mut held) = self.token.lock() {
            *held = token.clone();
        }
        self.running.store(true, Ordering::SeqCst);
        token
    }

    /// Marks the acquisition finished. See [`Self::begin`].
    pub fn finish(&self) {
        self.running.store(false, Ordering::SeqCst);
    }

    /// Cancels whichever acquisition is running, and says whether one
    /// was. See [`Self::begin`].
    pub fn cancel(&self) -> bool {
        if let Ok(held) = self.token.lock() {
            held.cancel();
        }
        self.running.load(Ordering::SeqCst)
    }
}

/// The configuration a settings form renders, and what it may write.
///
/// # Errors
///
/// An unreadable configuration file, or one that is not well-formed.
#[tauri::command]
pub fn settings_form(desktop: State<'_, Desktop>) -> Result<SettingsForm, CommandFailure> {
    desktop
        .application()
        .config()
        .snapshot()
        .map(Into::into)
        .map_err(Into::into)
}

/// Applies a set of field changes as one unit.
///
/// The service layer edits the document in place, validates the
/// complete candidate, and only then replaces the file atomically, so a
/// rejected change leaves the previous file exactly as it was and the
/// comments and advanced blocks no form models survive an accepted one.
///
/// # Errors
///
/// An unreadable or malformed existing file, a candidate the strict
/// schema rejects, or a durable replacement that fails.
#[tauri::command]
pub fn apply_settings(
    desktop: State<'_, Desktop>,
    changes: Vec<SettingsChange>,
) -> Result<SettingsForm, CommandFailure> {
    let mut update = ConfigUpdate::new();
    for change in changes {
        let value: ConfigValue = change.value.into();
        update = update.set(change.field.into(), value);
    }
    desktop.application().config().apply(&update)?;
    settings_form(desktop)
}

/// Everything one read-only diagnosis found.
///
/// Reads. Nothing here creates, deletes, or rewrites anything, which is
/// what lets a diagnostics screen call it on mount.
///
/// # Errors
///
/// A storage root that exists but cannot be enumerated.
#[tauri::command]
pub fn diagnostics_report(desktop: State<'_, Desktop>) -> Result<DiagnosticRows, CommandFailure> {
    let application = desktop.application();
    application
        .diagnostics()
        .diagnose(
            application.config(),
            application.sessions(),
            application.models(),
        )
        .map(|report| (&report).into())
        .map_err(Into::into)
}

/// Performs exactly one repair, chosen by the user.
///
/// Separate from [`diagnostics_report`] on purpose: the decision to
/// change something belongs to the user, not to the screen that noticed
/// the problem.
///
/// # Errors
///
/// An action this layer does not perform — installing a model, which
/// needs a confirmation, or opening a platform surface, which is a
/// different command — or a filesystem that refuses the change.
#[tauri::command]
pub fn apply_recovery(
    desktop: State<'_, Desktop>,
    action: RecoveryActionView,
) -> Result<RepairOutcome, CommandFailure> {
    let application = desktop.application();
    let action = rebuild(&action)?;
    application
        .diagnostics()
        .apply_repair(&action, application.sessions(), application.models())
        .map(Into::into)
        .map_err(Into::into)
}

/// Capture, transcription, notes, storage, and egress, separately.
///
/// # Errors
///
/// As [`diagnostics_report`], which this is a projection of.
#[tauri::command]
pub fn readiness_report(
    desktop: State<'_, Desktop>,
) -> Result<crate::contract::setup::ReadinessReport, CommandFailure> {
    let application = desktop.application();
    let report = application.diagnostics().diagnose(
        application.config(),
        application.sessions(),
        application.models(),
    )?;
    Ok((&Readiness::from_report(&report)).into())
}

/// What acquiring `id` would involve, and what is already installed.
///
/// Reads the catalog, the destination, and the filesystem. Opens no
/// connection, so this is safe to call before the user has agreed to
/// anything — which is the point, because its result is what they
/// agree to.
///
/// # Errors
///
/// An identity the catalog does not carry, or a catalog that is not
/// usable.
#[tauri::command]
pub fn model_offer(desktop: State<'_, Desktop>, id: String) -> Result<ModelOffer, CommandFailure> {
    desktop
        .application()
        .models()
        .plan(&id)
        .map(Into::into)
        .map_err(Into::into)
}

/// Acquires `id`, having been handed the digest the user was shown.
///
/// Progress is published as [`MODEL_PROGRESS_EVENT`] while bytes
/// arrive. A cancellation, a size or digest mismatch, a transport
/// failure, or a full disk comes back as an outcome rather than an
/// error, because each is a state the user is shown and can act on.
///
/// # Errors
///
/// A confirmation that does not name this model and the digest the
/// catalog offers — raised before anything is requested — or a catalog
/// or models directory that cannot be used.
#[tauri::command]
pub async fn install_model(
    app: tauri::AppHandle,
    id: String,
    acknowledged_sha256: String,
) -> Result<ModelOutcome, CommandFailure> {
    let acquisition = app.state::<Arc<ModelAcquisition>>().inner().clone();
    let token = acquisition.begin();
    let confirmation = ModelConfirmation::new(id.clone(), acknowledged_sha256);
    let emitter = app.clone();
    let published = id.clone();
    let progress = move |progress: DownloadProgress| {
        let event = ModelProgress {
            id: published.clone(),
            received_bytes: progress.received_bytes.to_string(),
            total_bytes: progress.total_bytes.to_string(),
        };
        // An observer cannot propagate, and a progress bar silently
        // frozen is worse than a line on stderr.
        if let Err(error) = emitter.emit(MODEL_PROGRESS_EVENT, event) {
            eprintln!("scrybe-desktop: could not emit model progress: {error}");
        }
    };
    let outcome = app
        .state::<Desktop>()
        .application()
        .models()
        .install(&id, &confirmation, &token, &progress)
        .await;
    acquisition.finish();
    outcome.map(Into::into).map_err(Into::into)
}

/// Asks whichever acquisition is running to stop.
///
/// Returns whether one was running. The manager checks the token
/// between chunks and leaves its `.partial` where it is, so nothing
/// half-written is ever promoted and nothing is deleted behind the
/// user.
#[must_use]
#[tauri::command]
pub fn cancel_model_install(acquisition: State<'_, Arc<ModelAcquisition>>) -> bool {
    acquisition.cancel()
}

/// Opens the System Settings pane where `capability` is granted.
///
/// The frontend names a capability, never a URL: the destination is
/// derived in Rust from a closed enumeration, so this command cannot be
/// asked to open something the frontend chose.
///
/// # Errors
///
/// A capability name the enumeration does not carry, or a platform that
/// refuses to open it.
#[tauri::command]
pub fn open_system_settings(
    app: tauri::AppHandle,
    capability: String,
) -> Result<(), CommandFailure> {
    let capability = match capability.as_str() {
        "microphone" => Capability::Microphone,
        "system_audio_recording" => Capability::SystemAudioRecording,
        other => {
            return Err(ApplicationError::new(
                ErrorCode::NotApplicable,
                format!("{other:?} is not a capability this application can recover"),
            )
            .into())
        }
    };
    open(&app, capability.settings_url())
}

/// Opens the configuration file for the advanced settings no form
/// models.
///
/// The path comes from the configuration service, not from the caller.
///
/// # Errors
///
/// A platform that refuses to open the file.
#[tauri::command]
pub fn open_advanced_configuration(app: tauri::AppHandle) -> Result<(), CommandFailure> {
    let path = app
        .state::<Desktop>()
        .application()
        .config()
        .path()
        .display()
        .to_string();
    open(&app, &path)
}

/// Hands `target` to the platform's own opener.
///
/// `open(1)` rather than a Tauri plugin: the plugin's permission is
/// scoped by a URL allowlist the frontend's capability file would have
/// to carry, and this host deliberately grants the frontend nothing
/// that names a path or a URL. Every target reaching here was built in
/// Rust from a closed enumeration, from the configuration service's own
/// path, or from a session folder the repository resolved beneath the
/// configured root.
///
/// Visible to the crate because the session surface reveals a folder
/// through it. A second opener would be a second place the rule above
/// has to hold.
#[cfg(target_os = "macos")]
pub(crate) fn open(_app: &tauri::AppHandle, target: &str) -> Result<(), CommandFailure> {
    std::process::Command::new("/usr/bin/open")
        .arg(target)
        .status()
        .map_err(|source| {
            CommandFailure::from(
                ApplicationError::new(ErrorCode::NotApplicable, "the platform could not open that")
                    .with_source(source),
            )
        })
        .and_then(|status| {
            if status.success() {
                Ok(())
            } else {
                Err(ApplicationError::new(
                    ErrorCode::NotApplicable,
                    "the platform declined to open that",
                )
                .into())
            }
        })
}

#[cfg(not(target_os = "macos"))]
pub(crate) fn open(_app: &tauri::AppHandle, _target: &str) -> Result<(), CommandFailure> {
    Err(ApplicationError::new(
        ErrorCode::NotApplicable,
        "this platform has no recovery surface to open yet",
    )
    .into())
}

/// Rebuilds the service layer's action from what the frontend was
/// handed.
///
/// The frontend returns the discriminant and fields it was given rather
/// than composing one, and this refuses anything it does not recognise.
/// A session identity is re-parsed through `SessionRef`, so a name that
/// could escape the storage root is rejected here rather than reaching
/// the repair.
fn rebuild(view: &RecoveryActionView) -> Result<RecoveryAction, CommandFailure> {
    let missing = |what: &str| -> CommandFailure {
        ApplicationError::new(
            ErrorCode::NotApplicable,
            format!("a {} recovery needs {what}", view.action),
        )
        .into()
    };
    Ok(match view.action.as_str() {
        "create_storage_root" => RecoveryAction::CreateStorageRoot,
        "review_configuration" => RecoveryAction::ReviewConfiguration,
        "open_advanced_configuration" => RecoveryAction::OpenAdvancedConfiguration,
        "repair_session" => RecoveryAction::RepairSession {
            id: session(view.id.as_deref().ok_or_else(|| missing("a session"))?)?,
        },
        "remove_stale_session_lock" => RecoveryAction::RemoveStaleSessionLock {
            id: session(view.id.as_deref().ok_or_else(|| missing("a session"))?)?,
        },
        "remove_orphaned_partial" => RecoveryAction::RemoveOrphanedPartial {
            name: scrybe_application::PartialFileRef::parse(
                view.name.as_deref().ok_or_else(|| missing("a file name"))?,
            )
            .map_err(CommandFailure::from)?,
        },
        "remove_model_partial" => RecoveryAction::RemoveModelPartial {
            name: scrybe_application::PartialFileRef::parse(
                view.name.as_deref().ok_or_else(|| missing("a file name"))?,
            )
            .map_err(CommandFailure::from)?
            .into(),
        },
        "install_transcription_model" => RecoveryAction::InstallTranscriptionModel {
            id: view.id.clone().ok_or_else(|| missing("a model"))?,
        },
        "open_system_settings" => RecoveryAction::OpenSystemSettings {
            capability: match view.capability.as_deref() {
                Some("microphone") => Capability::Microphone,
                Some("system_audio_recording") => Capability::SystemAudioRecording,
                _ => return Err(missing("a capability")),
            },
        },
        other => {
            return Err(ApplicationError::new(
                ErrorCode::NotApplicable,
                format!("{other:?} is not a recovery this application offers"),
            )
            .into())
        }
    })
}

fn session(candidate: &str) -> Result<SessionRef, CommandFailure> {
    SessionRef::parse(candidate).map_err(CommandFailure::from)
}
