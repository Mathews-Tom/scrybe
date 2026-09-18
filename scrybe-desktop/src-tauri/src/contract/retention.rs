// Copyright 2026 Mathews Tom
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//     https://www.apache.org/licenses/LICENSE-2.0

//! What a frontend learns after moving a session out of the listing.
//!
//! The outcome carries the retention window so the confirmation and the
//! acknowledgement can name the same number without the frontend
//! holding a copy of a configuration value. A surface that hardcoded
//! seven days would go on saying seven after a reader changed it.

use scrybe_application::sessions::Destination;
use scrybe_application::SessionRef;
use serde::Serialize;
use ts_rs::TS;

/// Where a retained session went.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum RetentionDestination {
    /// The trash, swept once the window passes.
    Trash,
    /// The archive, never swept.
    Archive,
}

impl From<Destination> for RetentionDestination {
    fn from(destination: Destination) -> Self {
        match destination {
            Destination::Trash => Self::Trash,
            Destination::Archive => Self::Archive,
        }
    }
}

/// The result of moving one session out of the listing.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, TS)]
pub struct RetentionOutcome {
    /// The session that moved, so a list can drop exactly that row.
    #[ts(type = "string")]
    pub id: SessionRef,
    /// Where it went.
    pub destination: RetentionDestination,
    /// Days a trashed session is kept. Reported for both destinations
    /// so one acknowledgement renders either, and ignored for the
    /// archive, which is never swept.
    pub retention_days: u32,
}
