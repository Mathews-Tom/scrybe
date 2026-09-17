// Copyright 2026 Mathews Tom
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//     https://www.apache.org/licenses/LICENSE-2.0

//! Whether the things that turn audio into a transcript and notes are
//! actually there.
//!
//! Two questions, deliberately answered separately. Local transcription
//! is a file on disk, and whether it is the right file is a digest
//! comparison the model manager already performs. Local notes are a
//! process listening on a port, and the existing egress classification
//! only ever *parsed* the configured URL — it never dialled it, so a
//! configuration pointing at a loopback address nothing is serving read
//! as "no egress (local LLM)" and told the user nothing about whether
//! notes would work.
//!
//! Both probes read. The model probe opens files; the notes probe opens
//! a TCP connection to a loopback address with a short timeout and
//! writes nothing to it. Neither creates, deletes, or rewrites
//! anything, so a diagnostics screen still cannot change the system by
//! being opened.
//!
//! A non-loopback notes endpoint is not dialled at all. Reaching out to
//! a hosted provider to see whether it answers would be an egress this
//! layer has no mandate for, and the egress classification already
//! reports that the endpoint is remote.

use std::net::{SocketAddr, TcpStream, ToSocketAddrs};
use std::time::Duration;

use scrybe_core::config::Config;
use url::{Host, Url};

use crate::diagnostics::contract::{
    DiagnosticCode, DiagnosticComponent, DiagnosticFinding, RecoveryAction, Severity,
};
use crate::models::{ModelManager, ModelState};

/// How long a loopback connection is given before it is called
/// unreachable. Generous for a process on the same machine, short
/// enough that a diagnostics screen does not appear to hang.
const PROBE_TIMEOUT: Duration = Duration::from_millis(400);

/// The catalog entry the application offers for local transcription.
const MANAGED_TRANSCRIPTION_MODEL: &str = "whisper-small-en";

/// The provider value that means transcription runs from a local file.
const LOCAL_TRANSCRIPTION_PROVIDER: &str = "whisper-local";

/// Reports on the local transcription model and the local notes
/// endpoint, under [`DiagnosticComponent::Providers`].
pub fn diagnose(config: &Config, models: &ModelManager, findings: &mut Vec<DiagnosticFinding>) {
    diagnose_transcription_model(config, models, findings);
    diagnose_model_partials(models, findings);
    diagnose_notes_endpoint(config, findings);
}

fn diagnose_transcription_model(
    config: &Config,
    models: &ModelManager,
    findings: &mut Vec<DiagnosticFinding>,
) {
    if config.stt.provider != LOCAL_TRANSCRIPTION_PROVIDER {
        return;
    }
    let plan = match models.plan(MANAGED_TRANSCRIPTION_MODEL) {
        Ok(plan) => plan,
        Err(error) => {
            findings.push(super::service::finding(
                DiagnosticCode::TranscriptionModelUnreadable,
                Severity::Error,
                DiagnosticComponent::Providers,
                format!("the managed model catalog is unusable: {}", error.message()),
                None,
            ));
            return;
        }
    };
    match plan.state {
        ModelState::Ready => findings.push(super::service::finding(
            DiagnosticCode::TranscriptionModelPresent,
            Severity::Info,
            DiagnosticComponent::Providers,
            format!(
                "local transcription model {} is installed and verified at {}",
                plan.id, plan.destination_path
            ),
            None,
        )),
        // A destination holding something the catalog does not describe
        // is reported without an action that would overwrite it: the
        // manager refuses to replace it, and so does this.
        ModelState::Failed { ref reason } => findings.push(super::service::finding(
            DiagnosticCode::TranscriptionModelUnreadable,
            Severity::Error,
            DiagnosticComponent::Providers,
            format!(
                "local transcription model {} cannot be used: {}",
                plan.id,
                describe(reason)
            ),
            Some(RecoveryAction::ReviewConfiguration),
        )),
        _ => findings.push(super::service::finding(
            DiagnosticCode::TranscriptionModelAbsent,
            Severity::Error,
            DiagnosticComponent::Providers,
            format!(
                "local transcription is configured but no model is installed at {}; \
                 recording would fail when transcription started",
                plan.destination_path
            ),
            Some(RecoveryAction::InstallTranscriptionModel { id: plan.id }),
        )),
    }
}

/// Every `.partial` under the models directory.
///
/// Distinct from the storage root's orphaned-partial finding, which
/// scans a different directory entirely: one is an interrupted session
/// write, this is an interrupted download. Neither is deleted.
fn diagnose_model_partials(models: &ModelManager, findings: &mut Vec<DiagnosticFinding>) {
    let Ok(partials) = models.stale_partials() else {
        return;
    };
    for partial in partials {
        findings.push(super::service::finding(
            DiagnosticCode::ModelDownloadPartial,
            Severity::Warning,
            DiagnosticComponent::Providers,
            format!(
                "an interrupted model download left {} ({} bytes) under {}",
                partial.name,
                partial.bytes,
                models.models_dir().display()
            ),
            Some(RecoveryAction::RemoveModelPartial {
                name: partial.name.clone(),
            }),
        ));
    }
}

fn diagnose_notes_endpoint(config: &Config, findings: &mut Vec<DiagnosticFinding>) {
    let Some(address) = loopback_address(&config.llm.base_url) else {
        return;
    };
    if TcpStream::connect_timeout(&address, PROBE_TIMEOUT).is_ok() {
        findings.push(super::service::finding(
            DiagnosticCode::NotesProviderReachable,
            Severity::Info,
            DiagnosticComponent::Providers,
            format!(
                "a local notes provider is answering at {}",
                config.llm.base_url
            ),
            None,
        ));
        return;
    }
    // A warning rather than an error. Recording and transcription do
    // not depend on it, and the product deliberately lets a user record
    // with notes unavailable — the session records a `notes_missing`
    // outcome instead of failing.
    findings.push(super::service::finding(
        DiagnosticCode::NotesProviderUnreachable,
        Severity::Warning,
        DiagnosticComponent::Providers,
        format!(
            "nothing is answering at the configured local notes provider {}; \
             recordings will complete without notes",
            config.llm.base_url
        ),
        Some(RecoveryAction::ReviewConfiguration),
    ));
}

/// The socket address behind `value`, when it names a loopback host.
///
/// `None` for anything else, including a URL that does not parse: a
/// remote endpoint is not dialled, and a malformed one is already
/// reported by the configuration diagnosis.
fn loopback_address(value: &str) -> Option<SocketAddr> {
    let url = Url::parse(value).ok()?;
    let is_loopback = match url.host()? {
        Host::Domain(host) => host.eq_ignore_ascii_case("localhost"),
        Host::Ipv4(address) => address.is_loopback(),
        Host::Ipv6(address) => address.is_loopback(),
    };
    if !is_loopback {
        return None;
    }
    let port = url.port_or_known_default()?;
    (url.host_str()?, port).to_socket_addrs().ok()?.next()
}

fn describe(reason: &crate::models::ModelFailure) -> String {
    use crate::models::ModelFailure;
    match reason {
        ModelFailure::InstalledArtifactUnrecognized { summary }
        | ModelFailure::Storage { summary }
        | ModelFailure::Transport { summary } => summary.clone(),
        ModelFailure::SizeMismatch { expected, observed } => {
            format!("it is {observed} bytes where the catalog describes {expected}")
        }
        ModelFailure::DigestMismatch { observed, .. } => {
            format!("it hashes to {observed}, which the catalog does not describe")
        }
        ModelFailure::InsufficientSpace {
            required,
            available,
        } => format!("installing it needs {required} bytes and {available} are free"),
    }
}
