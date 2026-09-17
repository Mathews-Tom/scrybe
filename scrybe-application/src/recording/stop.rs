// Copyright 2026 Mathews Tom
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//     https://www.apache.org/licenses/LICENSE-2.0

//! The one route from "some surface asked to stop" to "capture is torn
//! down".
//!
//! Every surface a reader can stop a recording from — a window control,
//! a tray item, a floating pill, a global hotkey, a signal, the act of
//! quitting — calls [`Stop::request`]. It asks the controller, and it
//! flips the capture signal **only if the controller accepted**. That
//! ordering is the whole point: [`RecordingController::request_stop`]
//! decides under one mutex whether this request is the one that counts,
//! so a surface cannot tear capture down for a stop the controller
//! refused, and two surfaces racing cannot produce two teardowns.
//!
//! The primitive is the controller's `Mutex<ControllerState>`. The
//! invariant it holds is that `stop_requested` moves from `false` to
//! `true` exactly once per recording, inside the same lock acquisition
//! that reads it, so exactly one caller ever observes
//! [`StopAcceptance::Accepted`]. Everything here is downstream of that
//! decision; it adds no second coordinator.

use tokio::sync::watch;

use crate::recording::contract::{StopAcceptance, StopSource};
use crate::recording::controller::RecordingController;

/// The sending half: what a surface asks to stop through.
///
/// Cloneable, because six surfaces hold one recording's stop and a
/// `watch::Sender` is designed to be shared. Cloning it does not clone
/// the decision — that lives in the controller, and there is one.
#[derive(Clone, Debug)]
pub struct Stop {
    signal: watch::Sender<bool>,
}

/// The receiving half: what holds capture open until a stop is
/// accepted.
#[derive(Clone, Debug)]
pub struct StopWatch {
    signal: watch::Receiver<bool>,
}

impl Stop {
    /// A fresh stop for one recording.
    #[must_use]
    pub fn new() -> (Self, StopWatch) {
        let (signal, receiver) = watch::channel(false);
        (Self { signal }, StopWatch { signal: receiver })
    }

    /// Asks the controller to stop, and tears capture down if it
    /// agreed.
    ///
    /// Returns what the controller decided, so a surface can render
    /// "already stopping" or "nothing is recording" rather than
    /// guessing. A surface that flipped the signal without checking
    /// this would end capture for a request the controller had already
    /// superseded.
    pub fn request(&self, controller: &RecordingController, source: StopSource) -> StopAcceptance {
        let acceptance = controller.request_stop(source);
        if acceptance == StopAcceptance::Accepted {
            // A send with no receivers left means capture has already
            // finished; the recording is over either way, which is why
            // this is not an error.
            let _ = self.signal.send(true);
            tracing::debug!(source = source.label(), "recording stop accepted");
        }
        acceptance
    }

    /// Whether a stop has already been accepted for this recording.
    #[must_use]
    pub fn accepted(&self) -> bool {
        *self.signal.borrow()
    }
}

impl StopWatch {
    /// Completes the first time a stop is accepted, or when every
    /// [`Stop`] has been dropped.
    ///
    /// The second case matters: a recording whose surfaces have all
    /// gone away must not hold capture open forever.
    pub async fn wait(mut self) {
        let _ = self.signal.wait_for(|stopped| *stopped).await;
    }
}
