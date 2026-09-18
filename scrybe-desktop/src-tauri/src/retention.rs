// Copyright 2026 Mathews Tom
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//     https://www.apache.org/licenses/LICENSE-2.0

//! The launch sweep.
//!
//! The service layer decides what a sweep removes; this decides when
//! one happens. Exactly once, during setup, before a window exists and
//! therefore before any frontend can ask for a listing.
//!
//! No command exposes the sweep. A frontend able to request one could
//! remove a reader's data while they were looking at it, and a timer
//! could do it without anybody asking at all. Tying removal to launch
//! has a visible consequence — an application left open for a
//! fortnight has not swept — and that is the honest trade rather than
//! a defect.
//!
//! [`sweep`] is generic over the runtime so a test can drive it against
//! `tauri::test::MockRuntime`; [`sweep_at_launch`] is the concrete
//! setup-time wrapper that reports what it did. The split exists
//! because the debug `note!` expansion resolves the concrete handle.

use chrono::{DateTime, Utc};
use scrybe_application::sessions::SweepOutcome;
use scrybe_application::ApplicationError;
use tauri::{AppHandle, Manager as _, Runtime};

use crate::state::Desktop;

/// Sweeps the trash, if this handle has services to sweep with.
///
/// `None` means no services were registered, which is not a failure:
/// there is no storage root to sweep.
pub fn sweep<R: Runtime>(
    handle: &AppHandle<R>,
    now: DateTime<Utc>,
) -> Option<Result<SweepOutcome, ApplicationError>> {
    let desktop = handle.try_state::<Desktop>()?;
    Some(desktop.application().retention().sweep(now))
}

/// Sweeps once at launch, reporting what happened.
///
/// A failure is reported and launch continues. Retention is not a
/// precondition for recording or reviewing anything, and refusing to
/// start over an unsweepable trash would deny a reader their sessions
/// to enforce a cleanup they never asked to watch.
pub fn sweep_at_launch(handle: &AppHandle) {
    let Some(result) = sweep(handle, Utc::now()) else {
        return;
    };
    match result {
        Ok(outcome) => {
            if !outcome.is_empty() {
                eprintln!(
                    "scrybe-desktop: retention removed {} session(s) past the window, \
                     and forgot {} already gone",
                    outcome.removed.len(),
                    outcome.forgotten.len()
                );
            }
            crate::note!(handle, "retention-swept");
        }
        Err(error) => {
            eprintln!("scrybe-desktop: the trash could not be swept: {error}");
            crate::note!(handle, "retention-unavailable");
        }
    }
}
