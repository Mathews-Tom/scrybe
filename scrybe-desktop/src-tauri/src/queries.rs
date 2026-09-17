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
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

use scrybe_application::CancellationToken;

/// One query's cancellation token together with the generation its
/// registration was issued under.
struct Entry {
    token: CancellationToken,
    generation: u64,
}

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
/// result of a newer one: [`Self::finish`] only removes the entry it
/// was issued for, identified by generation rather than by the
/// identifier alone, so a query that finishes late can never evict the
/// entry of the query that displaced it.
#[derive(Default)]
pub struct LiveQueries {
    live: Mutex<HashMap<String, Entry>>,
    next_generation: AtomicU64,
}

impl LiveQueries {
    /// Registers a fresh token for `request_id` and returns it together
    /// with the generation this registration was issued under.
    ///
    /// An identifier already in the map belongs to a query that has not
    /// reported itself finished. Its token is cancelled as it is
    /// replaced: two live queries under one identifier would leave
    /// [`Self::cancel`] unable to say which of them it reached. The
    /// generation returned here must be passed back to [`Self::finish`]
    /// so a query that finishes after losing this race cannot remove
    /// the entry of the query that replaced it.
    #[must_use]
    pub fn begin(&self, request_id: &str) -> (CancellationToken, u64) {
        let generation = self.next_generation.fetch_add(1, Ordering::Relaxed);
        let token = CancellationToken::new();
        if let Ok(mut live) = self.live.lock() {
            let entry = Entry {
                token: token.clone(),
                generation,
            };
            if let Some(replaced) = live.insert(request_id.to_owned(), entry) {
                replaced.token.cancel();
            }
        }
        (token, generation)
    }

    /// Drops the entry for `request_id`, but only if it is still the
    /// one registered under `generation`.
    ///
    /// Every query calls this on its way out, cancelled or not, so the
    /// map holds exactly the queries that are still running rather than
    /// growing by one per keystroke. A query that lost the
    /// [`Self::begin`] race and finishes after a newer query has
    /// registered under the same identifier must not evict that newer
    /// query's still-live entry — doing so would leave a later
    /// [`Self::cancel`] for the newer query with nothing to find.
    pub fn finish(&self, request_id: &str, generation: u64) {
        if let Ok(mut live) = self.live.lock() {
            if live
                .get(request_id)
                .is_some_and(|entry| entry.generation == generation)
            {
                live.remove(request_id);
            }
        }
    }

    /// Cancels the query registered under `request_id`, and says
    /// whether one was registered.
    ///
    /// Cancels whichever query is currently registered under the
    /// identifier, regardless of generation: that is the query the
    /// caller means to reach.
    #[must_use]
    pub fn cancel(&self, request_id: &str) -> bool {
        let Ok(live) = self.live.lock() else {
            return false;
        };
        live.get(request_id).is_some_and(|entry| {
            entry.token.cancel();
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
        let (token, _generation) = queries.begin("q-1");

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
        let (_token, generation) = queries.begin("q-1");

        queries.finish("q-1", generation);

        assert!(!queries.cancel("q-1"));
    }

    #[test]
    fn test_cancelling_one_query_leaves_every_other_query_running() {
        let queries = LiveQueries::default();
        let (first, _first_generation) = queries.begin("q-1");
        let (second, _second_generation) = queries.begin("q-2");

        assert!(queries.cancel("q-1"));

        assert!(first.is_cancelled());
        assert!(!second.is_cancelled());
    }

    /// Two queries under one identifier would leave `cancel` unable to
    /// say which it reached, so the one being displaced is cancelled.
    #[test]
    fn test_reusing_a_live_identifier_cancels_the_query_it_displaces() {
        let queries = LiveQueries::default();
        let (displaced, _displaced_generation) = queries.begin("q-1");

        let (replacement, _replacement_generation) = queries.begin("q-1");

        assert!(displaced.is_cancelled());
        assert!(!replacement.is_cancelled());
    }

    /// A query that lost the `begin` race and calls `finish` late must
    /// not evict the entry of the query that displaced it: that would
    /// leave a later `cancel` for the still-running newer query unable
    /// to find anything, contradicting this module's guarantee that an
    /// abandoned query can never replace the result of a newer one.
    #[test]
    fn test_a_displaced_registration_finishing_late_cannot_evict_the_query_that_replaced_it() {
        let queries = LiveQueries::default();
        let (_older_token, older_generation) = queries.begin("q-1");
        let (_newer_token, _newer_generation) = queries.begin("q-1");

        queries.finish("q-1", older_generation);

        assert!(queries.cancel("q-1"));
    }
}
