// Copyright 2026 Mathews Tom
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//     https://www.apache.org/licenses/LICENSE-2.0

//! `LiveQueries`, exercised from outside the crate.
//!
//! Moved here from an inline `#[cfg(test)]` module in `src/queries.rs`
//! so this coverage does not count against the crate's `LoC` budget:
//! the gate measures `src/` only, and `LiveQueries` is already `pub`,
//! so nothing here needs access this crate does not already expose.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use scrybe_desktop::queries::LiveQueries;

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
