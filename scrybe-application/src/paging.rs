// Copyright 2026 Mathews Tom
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//     https://www.apache.org/licenses/LICENSE-2.0

//! Paging contract shared by every listing and search response.
//!
//! [`PageRequest`] clamps its own limit, including when it arrives over
//! the wire, so no consumer can ask a service to materialize an
//! unbounded result set.

use serde::{Deserialize, Serialize};

/// Rows returned when a caller does not choose a limit.
pub const DEFAULT_PAGE_LIMIT: usize = 20;

/// Hard upper bound on rows per page, regardless of what is requested.
pub const MAX_PAGE_LIMIT: usize = 200;

/// A caller's request for one page of results.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(from = "RawPageRequest")]
pub struct PageRequest {
    offset: usize,
    limit: usize,
}

impl PageRequest {
    /// The first page at the default limit.
    #[must_use]
    pub const fn first() -> Self {
        Self {
            offset: 0,
            limit: DEFAULT_PAGE_LIMIT,
        }
    }

    /// A page starting at `offset`, with `limit` clamped into
    /// `1..=MAX_PAGE_LIMIT`.
    #[must_use]
    pub const fn new(offset: usize, limit: usize) -> Self {
        Self {
            offset,
            limit: if limit < 1 {
                1
            } else if limit > MAX_PAGE_LIMIT {
                MAX_PAGE_LIMIT
            } else {
                limit
            },
        }
    }

    /// Index of the first requested row.
    #[must_use]
    pub const fn offset(self) -> usize {
        self.offset
    }

    /// Maximum rows in the response.
    #[must_use]
    pub const fn limit(self) -> usize {
        self.limit
    }
}

impl Default for PageRequest {
    fn default() -> Self {
        Self::first()
    }
}

#[derive(Deserialize)]
struct RawPageRequest {
    #[serde(default)]
    offset: usize,
    #[serde(default = "default_limit")]
    limit: usize,
}

const fn default_limit() -> usize {
    DEFAULT_PAGE_LIMIT
}

impl From<RawPageRequest> for PageRequest {
    fn from(value: RawPageRequest) -> Self {
        Self::new(value.offset, value.limit)
    }
}

/// One page of results plus the cursor state a frontend needs to page on.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct Page<T> {
    /// Rows in this page.
    pub items: Vec<T>,
    /// Index of the first row, after clamping to the available total.
    pub offset: usize,
    /// Rows matching the request across every page.
    pub total: usize,
    /// Whether rows remain after this page.
    pub has_more: bool,
}

impl<T> Page<T> {
    /// Slices `all` — already ordered and already filtered — into the
    /// requested window.
    #[must_use]
    pub fn paginate(mut all: Vec<T>, request: PageRequest) -> Self {
        let total = all.len();
        let offset = request.offset().min(total);
        all.drain(..offset);
        all.truncate(request.limit());
        Self {
            has_more: offset + all.len() < total,
            items: all,
            offset,
            total,
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    fn rows(count: usize) -> Vec<usize> {
        (0..count).collect()
    }

    #[test]
    fn test_paginate_reports_more_rows_remaining_when_the_window_is_short() {
        let page = Page::paginate(rows(50), PageRequest::new(0, 10));

        assert_eq!(page.items, rows(10));
        assert_eq!(page.total, 50);
        assert!(page.has_more);
    }

    #[test]
    fn test_paginate_reports_no_more_rows_on_the_final_window() {
        let page = Page::paginate(rows(50), PageRequest::new(45, 10));

        assert_eq!(page.items, (45..50).collect::<Vec<_>>());
        assert!(!page.has_more);
    }

    #[test]
    fn test_paginate_clamps_an_offset_past_the_end_to_an_empty_final_page() {
        let page = Page::paginate(rows(3), PageRequest::new(99, 10));

        assert!(page.items.is_empty());
        assert_eq!(page.offset, 3);
        assert_eq!(page.total, 3);
        assert!(!page.has_more);
    }

    #[test]
    fn test_requesting_an_unbounded_limit_is_clamped_to_the_maximum() {
        let request = PageRequest::new(0, usize::MAX);

        assert_eq!(request.limit(), MAX_PAGE_LIMIT);
    }

    #[test]
    fn test_requesting_a_zero_limit_still_returns_one_row() {
        let request = PageRequest::new(0, 0);

        assert_eq!(request.limit(), 1);
    }

    #[test]
    fn test_a_wire_supplied_limit_is_clamped_on_deserialization() {
        let request: PageRequest = serde_json::from_str(r#"{"offset":5,"limit":100000}"#).unwrap();

        assert_eq!(request.offset(), 5);
        assert_eq!(request.limit(), MAX_PAGE_LIMIT);
    }

    #[test]
    fn test_a_wire_request_without_a_limit_uses_the_default() {
        let request: PageRequest = serde_json::from_str(r#"{"offset":0}"#).unwrap();

        assert_eq!(request.limit(), DEFAULT_PAGE_LIMIT);
    }
}
