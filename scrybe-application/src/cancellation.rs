// Copyright 2026 Mathews Tom
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//     https://www.apache.org/licenses/LICENSE-2.0

//! Cooperative cancellation for long filesystem work.
//!
//! A search over a large storage root reads every session's notes and
//! transcript. A frontend that navigates away, or a user who keeps
//! typing, must be able to abandon that work without waiting for it.
//! The token is checked before the scan starts, between entries while
//! the root is fingerprinted, between folders while it is classified,
//! and between sessions while they are matched. Matching one session
//! may read both its notes and its transcript behind a single check,
//! so an abandoned search stops within one session's worth of I/O.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

/// A cancellation flag shared between a caller and the work it started.
///
/// Cloning shares the flag; cancelling any clone cancels all of them.
/// Cancellation is one-way: a cancelled token stays cancelled.
#[derive(Clone, Debug, Default)]
pub struct CancellationToken {
    flag: Arc<AtomicBool>,
}

impl CancellationToken {
    /// A token that has not been cancelled.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Requests cancellation. Idempotent.
    pub fn cancel(&self) {
        self.flag.store(true, Ordering::SeqCst);
    }

    /// Whether cancellation has been requested.
    #[must_use]
    pub fn is_cancelled(&self) -> bool {
        self.flag.load(Ordering::SeqCst)
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    #[test]
    fn test_cancelling_one_handle_cancels_every_clone() {
        let token = CancellationToken::new();
        let clone = token.clone();

        clone.cancel();

        assert!(token.is_cancelled());
    }

    #[test]
    fn test_a_fresh_token_is_not_cancelled() {
        assert!(!CancellationToken::new().is_cancelled());
    }
}
