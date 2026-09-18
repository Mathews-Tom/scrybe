// Copyright 2026 Mathews Tom
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//     https://www.apache.org/licenses/LICENSE-2.0

//! The one path from "a surface asked to record" to "capture may begin".
//!
//! Resolution, preflight, and the controller's entry into the
//! preparing state happen here, once, for every surface.
//! Before this existed the command-line recorder ran six of the seven
//! checks inline in its own `run_with_stop`, where no other frontend
//! could reach them, and [`ErrorCode::PreflightFailed`] was an unused
//! stub because nothing ever produced it.
//!
//! What this module deliberately does **not** own is capture
//! construction and the call into `scrybe_core::session`. Those name
//! the per-platform capture adapters, which live in crates this one
//! does not depend on; a frontend that linked them opens the source
//! this module's plan names. The division is: everything that decides
//! *what* and *whether* is here, and only the act of opening a device
//! is at the platform edge.

use std::path::Path;

use crate::config::ConfigService;
use crate::error::ErrorCode;
use crate::recording::controller::RecordingController;
use crate::recording::plan::{RecordingOverrides, RecordingPlan};
use crate::recording::preflight::{self, CaptureSupport, PreflightReport};
use crate::Result;

/// The summary a refused recording's transition event carries.
///
/// Fixed by construction, exactly as the command-line recorder's
/// failure summary is. A `RecordingFailure` is a `Serialize` field of
/// both `RecordingEvent` and `RecordingSnapshot`, and every surface
/// that renders a transition gets one; the refusal's own reasons name
/// model paths and device identities, so they stay in the
/// [`PreflightReport`] the caller is handed, where a reader who asked
/// sees them and a log line does not.
pub const PREFLIGHT_FAILURE_SUMMARY: &str = "recording could not start";

/// A refused recording, and why.
///
/// Carries the report as well as the error so a surface can render
/// every check rather than only the summary line: a reader whose model
/// is missing wants to see that storage and the device were fine.
#[derive(Debug)]
pub struct Refusal {
    pub report: PreflightReport,
    pub error: crate::error::ApplicationError,
    /// What was asked for, when resolution got that far.
    ///
    /// `None` only when the configuration could not be resolved to a
    /// plan at all, or when the controller refused the start before
    /// resolution ran. A surface renders the refusal against the plan —
    /// "`mic+system` needs an adapter this build does not carry" reads
    /// very differently from the same sentence with no source named.
    pub plan: Option<RecordingPlan>,
}

/// Resolves and checks a recording without starting one.
///
/// Writes nothing, and leaves the controller where it found it. This is
/// what a surface calls to render readiness before a reader has asked
/// for anything.
///
/// # Errors
///
/// Whatever [`ConfigService::load`] reports, or
/// [`ErrorCode::ConfigInvalid`] when the file names a source, adapter,
/// or notes backend this release does not define.
pub fn check(
    config: &ConfigService,
    home: Option<&Path>,
    support: CaptureSupport,
    devices: Option<&[String]>,
    overrides: &RecordingOverrides,
) -> Result<(RecordingPlan, PreflightReport)> {
    let plan = RecordingPlan::resolve(&config.load()?, home, overrides)?;
    let report = preflight::run(&plan, support, devices);
    Ok((plan, report))
}

/// Resolves, checks, and moves the controller into
/// `RecordingState::Preparing`.
///
/// The returned plan is what the caller opens capture from. Until it
/// does, the controller sits in `Preparing` and the single monotonic
/// origin has not started — which is the point: permission prompts and
/// a model load are not recorded time.
///
/// A refusal settles the controller back to `RecordingState::Idle`
/// through `RecordingFailureKind::Preflight`, so a surface that was
/// watching sees the attempt and its outcome rather than nothing at
/// all. Nothing is written at any point on this path: preflight reads,
/// and the session folder is created by the session run the caller has
/// not reached.
///
/// # Errors
///
/// The [`Refusal`] — boxed, because it carries the whole report and a
/// plan, and every caller's success path would otherwise pay for its
/// size — carries [`ErrorCode::PreflightFailed`] when a check
/// blocked, or a configuration error when the file could not be
/// resolved to a plan at all. A controller that is not idle refuses
/// with [`ErrorCode::RecordingStateConflict`] and does not run
/// preflight, because a second recording is not a preflight failure.
pub fn begin(
    controller: &RecordingController,
    config: &ConfigService,
    home: Option<&Path>,
    support: CaptureSupport,
    devices: Option<&[String]>,
    overrides: &RecordingOverrides,
) -> std::result::Result<RecordingPlan, Box<Refusal>> {
    // Before the plan is resolved, so a reader who presses record twice
    // is told a recording is already running rather than being handed
    // the second attempt's preflight.
    if !controller.snapshot().state.accepts_start() {
        return Err(Box::new(Refusal {
            report: empty_report(),
            error: crate::error::ApplicationError::new(
                ErrorCode::RecordingStateConflict,
                "a recording is already in flight",
            ),
            plan: None,
        }));
    }

    let (plan, report) = match check(config, home, support, devices, overrides) {
        Ok(resolved) => resolved,
        Err(error) => {
            return Err(Box::new(Refusal {
                report: empty_report(),
                error,
                plan: None,
            }))
        }
    };

    if let Err(error) = report.clone().into_result() {
        settle_refused(controller);
        return Err(Box::new(Refusal {
            report,
            error,
            plan: Some(plan),
        }));
    }

    match controller.begin_preparing() {
        Ok(_) => Ok(plan),
        Err(error) => Err(Box::new(Refusal {
            report,
            error,
            plan: Some(plan),
        })),
    }
}

/// Walks the controller through a visible refusal and back to idle.
///
/// The summary is the preflight error's own message, which is built
/// from check labels and the reader's own configuration — no path this
/// module did not already put in a `PreflightFinding`, and nothing from
/// a recording, because none happened.
fn settle_refused(controller: &RecordingController) {
    if controller.begin_preparing().is_err() {
        return;
    }
    // `fail` labels the failure from the state it is called in, and
    // `Preparing` is exactly where a preflight refusal belongs. That
    // label is not restated here: one place decides it.
    if controller.fail(PREFLIGHT_FAILURE_SUMMARY).is_err() {
        return;
    }
    let _ = controller.acknowledge();
}

/// The report a refusal that never ran preflight carries.
const fn empty_report() -> PreflightReport {
    PreflightReport {
        schema_version: preflight::PREFLIGHT_SCHEMA_VERSION,
        findings: Vec::new(),
    }
}
