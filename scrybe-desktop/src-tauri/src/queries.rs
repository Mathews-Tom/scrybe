// Copyright 2026 Mathews Tom
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//     https://www.apache.org/licenses/LICENSE-2.0

//! Cancellation for the reads a view has in flight.
//!
//! `CancellationToken` does not serialize and Tauri's `invoke` has no
//! abort primitive, so the frontend can neither hold a token nor drop a
//! call it has already made. What it can do is name the call: it mints
//! an identifier per query, passes that identifier with the query, and
//! sends the same identifier to a cancel command when it abandons the
//! query. This registry is where those two commands meet.
//!
//! Deliberately not a generalization of `setup::ModelAcquisition`. That
//! type holds one slot because one acquisition runs at a time and a
//! cancel that names nothing still has to reach it, and it carries a
//! `running` flag whose only purpose is to answer whether anything was
//! cancelled. A library view has several reads outstanding and cancels
//! one of them by name, which a single slot cannot express; widening
//! that type to carry both shapes would change the behaviour of an
//! already-shipped surface in order to serve a new one.

use std::collections::HashMap;
use std::sync::Mutex;

use scrybe_application::CancellationToken;

/// The cancellation token of every query currently running, keyed by
/// the identifier the frontend minted for it.
///
/// Cancellation here stops work; it is not what decides which answer a
/// view renders. A cancel and the query it names arrive as two separate
/// invokes and the runtime may run them in either order, so a cancel
/// can lose the race and leave its query to run to completion. The
/// frontend therefore also discards any answer whose identifier is no
/// longer the one it is waiting for. Between the two, an abandoned
/// query wastes at most one page of I/O and can never replace the
/// result of a newer one.
#[derive(Default)]
pub struct LiveQueries {
    live: Mutex<HashMap<String, CancellationToken>>,
}

impl LiveQueries {
    /// Registers a fresh token for `request_id` and returns it.
    ///
    /// An identifier already in the map belongs to a query that has not
    /// reported itself finished. Its token is cancelled as it is
    /// replaced: two live queries under one identifier would leave
    /// [`Self::cancel`] unable to say which of them it reached.
    #[must_use]
    pub fn begin(&self, request_id: &str) -> CancellationToken {
        let token = CancellationToken::new();
        if let Ok(mut live) = self.live.lock() {
            if let Some(replaced) = live.insert(request_id.to_owned(), token.clone()) {
                replaced.cancel();
            }
        }
        token
    }

    /// Drops the entry for `request_id`.
    ///
    /// Every query calls this on its way out, cancelled or not, so the
    /// map holds exactly the queries that are still running rather than
    /// growing by one per keystroke.
    pub fn finish(&self, request_id: &str) {
        if let Ok(mut live) = self.live.lock() {
            live.remove(request_id);
        }
    }

    /// Cancels the query registered under `request_id`, and says
    /// whether one was registered.
    #[must_use]
    pub fn cancel(&self, request_id: &str) -> bool {
        let Ok(live) = self.live.lock() else {
            return false;
        };
        live.get(request_id).is_some_and(|token| {
            token.cancel();
            true
        })
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    #[test]
    fn test_cancelling_a_live_query_cancels_the_token_it_is_running_with() {
        let queries = LiveQueries::default();
        let token = queries.begin("q-1");

        assert!(queries.cancel("q-1"));
        assert!(token.is_cancelled());
    }

    #[test]
    fn test_cancelling_an_identifier_no_query_is_running_under_reports_nothing_to_cancel() {
        let queries = LiveQueries::default();

        assert!(!queries.cancel("q-1"));
    }

    #[test]
    fn test_a_finished_query_leaves_nothing_behind_for_a_later_cancel_to_find() {
        let queries = LiveQueries::default();
        let _token = queries.begin("q-1");

        queries.finish("q-1");

        assert!(!queries.cancel("q-1"));
    }

    #[test]
    fn test_cancelling_one_query_leaves_every_other_query_running() {
        let queries = LiveQueries::default();
        let first = queries.begin("q-1");
        let second = queries.begin("q-2");

        assert!(queries.cancel("q-1"));

        assert!(first.is_cancelled());
        assert!(!second.is_cancelled());
    }

    /// Two queries under one identifier would leave `cancel` unable to
    /// say which it reached, so the one being displaced is cancelled.
    #[test]
    fn test_reusing_a_live_identifier_cancels_the_query_it_displaces() {
        let queries = LiveQueries::default();
        let displaced = queries.begin("q-1");

        let replacement = queries.begin("q-1");

        assert!(displaced.is_cancelled());
        assert!(!replacement.is_cancelled());
    }
}
