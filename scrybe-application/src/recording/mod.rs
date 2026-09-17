// Copyright 2026 Mathews Tom
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//     https://www.apache.org/licenses/LICENSE-2.0

//! Recording controller contracts.
//!
//! One process-wide state model backs every recording surface: the CLI
//! signal bridge, the macOS tray, the floating pill, the global hotkey,
//! and any future window. Each surface observes the same
//! [`RecordingSnapshot`] and requests the same idempotent stop, so no
//! surface can define its own notion of "recording".
//!
//! Transition events carry timing and state only. No audio frame, no
//! transcript text, and no notes content is ever part of a recording
//! event.

mod contract;
mod controller;

pub use contract::{
    RecordingEvent, RecordingFailure, RecordingFailureKind, RecordingSnapshot, RecordingState,
    StopAcceptance, StopSource, RECORDING_EVENT_SCHEMA_VERSION,
};
pub use controller::{
    MonotonicClock, RecordingController, RecordingEventObserver, SystemMonotonicClock,
};
