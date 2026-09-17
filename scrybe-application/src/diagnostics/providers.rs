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
//!
//! "Loopback" is decided twice, and deliberately so. The URL's host
//! must be *written* as a loopback host, which is what keeps a hosted
//! endpoint from being resolved at all; and the address it resolves to
//! must *be* loopback, which is what keeps the guarantee from reducing
//! to a claim about spelling. A name is loopback only by convention —
//! `localhost` is whatever the hosts file, the resolver, and the search
//! domain between them say it is — so a configuration that looks local
//! and answers routable is refused rather than dialled.

use std::ffi::{OsStr, OsString};
use std::net::{SocketAddr, TcpStream, ToSocketAddrs};
use std::time::Duration;

use scrybe_core::config::Config;
use url::{Host, Url};

use crate::diagnostics::contract::{
    DiagnosticCode, DiagnosticComponent, DiagnosticFinding, RecoveryAction, Severity,
};
use crate::models::{ModelManager, ModelPlan, ModelState};

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
    // `[stt].model` is a free-text field, and the catalog holds exactly
    // one entry. Reporting the managed artifact's state regardless would
    // answer for a file the runtime is not going to open.
    if !loads_the_managed_artifact(config, &plan) {
        diagnose_configured_transcription_model(config, models, &plan, findings);
        return;
    }
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

/// Whether the file the runtime will open is the catalog's artifact.
///
/// Compared by filename rather than by whole path, and that is not a
/// shortcut: `Application::models_dir_for` resolves the models
/// directory from `whisper_model_path(&stt.model)` — the same value —
/// so the directory agrees by construction and cannot be what differs.
/// What can differ is which artifact the configuration names, and that
/// is the filename.
fn loads_the_managed_artifact(config: &Config, plan: &ModelPlan) -> bool {
    configured_artifact_name(config).as_deref() == Some(OsStr::new(&plan.destination))
}

/// The filename `[stt].model` resolves to, by the one rule the runtime
/// uses to resolve it.
fn configured_artifact_name(config: &Config) -> Option<OsString> {
    scrybe_core::record_defaults::whisper_model_path(&config.stt.model)?
        .file_name()
        .map(OsStr::to_os_string)
}

/// What to report when `[stt].model` names something other than the
/// catalog's artifact.
///
/// The managed model's state is not reported at all here, because it is
/// not the file that will load and saying it is installed would be the
/// defect this branch exists to avoid. Neither is installing it offered
/// as the recovery: it would not change what loads. What is reported is
/// whether the configured file is there, named so the reader can see
/// which one was checked.
fn diagnose_configured_transcription_model(
    config: &Config,
    models: &ModelManager,
    plan: &ModelPlan,
    findings: &mut Vec<DiagnosticFinding>,
) {
    let Some(name) = configured_artifact_name(config) else {
        findings.push(super::service::finding(
            DiagnosticCode::TranscriptionModelUnreadable,
            Severity::Error,
            DiagnosticComponent::Providers,
            format!(
                "local transcription is configured to load {}, which does not resolve to a file on this Mac",
                config.stt.model
            ),
            Some(RecoveryAction::ReviewConfiguration),
        ));
        return;
    };
    let path = models.models_dir().join(&name);
    if path.is_file() {
        findings.push(super::service::finding(
            DiagnosticCode::TranscriptionModelPresent,
            Severity::Info,
            DiagnosticComponent::Providers,
            format!(
                "local transcription will load {}, which is present. It is not the managed model {}, \
                 so its contents are not checked against the catalog",
                path.display(),
                plan.id
            ),
            None,
        ));
        return;
    }
    findings.push(super::service::finding(
        DiagnosticCode::TranscriptionModelAbsent,
        Severity::Error,
        DiagnosticComponent::Providers,
        format!(
            "local transcription is configured to load {}, and no file is there. The managed model {} \
             is a different artifact, so installing it would not change what loads",
            path.display(),
            plan.id
        ),
        Some(RecoveryAction::ReviewConfiguration),
    ));
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
    // not depend on it, and setup can be finished without a local notes
    // provider; one can be configured later.
    findings.push(super::service::finding(
        DiagnosticCode::NotesProviderUnreachable,
        Severity::Warning,
        DiagnosticComponent::Providers,
        format!(
            "nothing is answering at the configured local notes provider {}; \
             notes need one running on this Mac",
            config.llm.base_url
        ),
        Some(RecoveryAction::ReviewConfiguration),
    ));
}

/// The socket address behind `value`, when both its spelling and the
/// address it resolves to are loopback.
///
/// `None` for anything else, including a URL that does not parse: a
/// remote endpoint is not dialled, and a malformed one is already
/// reported by the configuration diagnosis.
///
/// Two checks, not one, because the spelling is not the address. A
/// name is loopback only by convention — `localhost` is whatever the
/// hosts file, the resolver, and the search domain between them say it
/// is, and any of those can be made to answer with a routable address
/// on a machine this code does not control. Checking only the spelling
/// and then dialling whatever came back would turn the guarantee in
/// this module's header into a statement about how a string is written.
fn loopback_address(value: &str) -> Option<SocketAddr> {
    let (host, port) = spelled_loopback(value)?;
    resolved_loopback((host.as_str(), port).to_socket_addrs().ok()?)
}

/// The host and port of `value`, when the host is *written* as a
/// loopback host.
///
/// The cheap half of the test, and the one that decides whether a
/// resolution happens at all: a hosted endpoint is neither resolved
/// nor dialled, because reaching out to see whether it answers is an
/// egress this layer has no mandate for.
///
/// The returned host is unbracketed even for IPv6: `url.host_str()`
/// carries the bracketed form (`[::1]`) that a URL authority requires,
/// but neither `Ipv6Addr::from_str` nor the platform resolver accepts
/// brackets, so passing that form straight into `to_socket_addrs`
/// fails resolution outright and the caller never learns whether the
/// notes provider it names is reachable.
fn spelled_loopback(value: &str) -> Option<(String, u16)> {
    let url = Url::parse(value).ok()?;
    let host = match url.host()? {
        Host::Domain(host) if host.eq_ignore_ascii_case("localhost") => host.to_owned(),
        Host::Ipv4(address) if address.is_loopback() => address.to_string(),
        Host::Ipv6(address) if address.is_loopback() => address.to_string(),
        _ => return None,
    };
    Some((host, url.port_or_known_default()?))
}

/// The address `resolved` would be dialled at, but only when that
/// address is itself loopback.
///
/// The first entry and no other, because the first is the one a
/// connection would use. Refusing outright when it is not loopback,
/// rather than searching the rest for one that is, means a resolver
/// answering with a routable address is a refusal instead of a
/// connection that happened to land somewhere acceptable.
fn resolved_loopback(mut resolved: impl Iterator<Item = SocketAddr>) -> Option<SocketAddr> {
    let address = resolved.next()?;
    address.ip().is_loopback().then_some(address)
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

// Inline because both halves are private and neither is reachable
// through `diagnose`: deciding what a hostname resolves to is the
// resolver's job, and a behavioural test in `tests/` could only assert
// it by depending on what this machine's resolver happens to answer.
// Splitting the decision from the resolution is what makes the
// property assertable at all.
#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use std::net::{Ipv4Addr, Ipv6Addr};

    #[test]
    fn test_a_hostname_that_merely_looks_local_is_not_treated_as_loopback() {
        // Spelled to be mistaken for the real thing at a glance. It is
        // an ordinary domain, and its owner chooses what it resolves to.
        assert!(spelled_loopback("http://localhost.example.com:11434/v1").is_none());
        assert!(spelled_loopback("http://notlocalhost:11434/v1").is_none());
        assert!(spelled_loopback("http://localhost.attacker.test:11434/v1").is_none());
    }

    #[test]
    fn test_a_loopback_spelling_is_accepted_for_resolution() {
        assert_eq!(
            spelled_loopback("http://localhost:11434/v1"),
            Some(("localhost".to_owned(), 11434))
        );
        assert_eq!(
            spelled_loopback("http://127.0.0.1:11434/v1"),
            Some(("127.0.0.1".to_owned(), 11434))
        );
        // Unbracketed: the bracketed form a URL authority requires is
        // not one `to_socket_addrs` accepts, so carrying it through
        // would resolve nothing.
        assert_eq!(
            spelled_loopback("http://[::1]:11434/v1"),
            Some(("::1".to_owned(), 11434))
        );
    }

    #[test]
    fn test_an_ipv6_loopback_endpoint_is_resolved_and_probed() {
        // The end-to-end proof: a bracketed IPv6 host must still reach
        // `to_socket_addrs` successfully rather than merely spelling
        // correctly. Before the fix this resolved to `None` and the
        // notes probe silently never ran.
        assert_eq!(
            loopback_address("http://[::1]:11434/v1"),
            Some(SocketAddr::from((Ipv6Addr::LOCALHOST, 11434)))
        );
    }

    #[test]
    fn test_a_loopback_spelling_that_resolves_outward_is_refused_rather_than_dialled() {
        // What a hosts-file entry, a search domain, or a resolver
        // answering for `localhost` produces. The spelling passed; the
        // address must not.
        let outward = SocketAddr::from((Ipv4Addr::new(93, 184, 216, 34), 11434));
        assert_eq!(resolved_loopback(std::iter::once(outward)), None);
    }

    #[test]
    fn test_a_resolution_whose_first_answer_is_routable_is_refused_outright() {
        // Not searched past. A resolver that puts a routable address
        // first is a resolver this code refuses to trust for the rest.
        let outward = SocketAddr::from((Ipv4Addr::new(93, 184, 216, 34), 11434));
        let inward = SocketAddr::from((Ipv4Addr::LOCALHOST, 11434));
        assert_eq!(resolved_loopback([outward, inward].into_iter()), None);
    }

    #[test]
    fn test_an_address_that_is_loopback_is_returned_for_dialling() {
        let v4 = SocketAddr::from((Ipv4Addr::LOCALHOST, 11434));
        assert_eq!(resolved_loopback(std::iter::once(v4)), Some(v4));
        let v6 = SocketAddr::from((Ipv6Addr::LOCALHOST, 11434));
        assert_eq!(resolved_loopback(std::iter::once(v6)), Some(v6));
    }
}
