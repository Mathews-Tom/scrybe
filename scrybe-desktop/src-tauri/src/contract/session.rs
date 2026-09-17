// Copyright 2026 Mathews Tom
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//     https://www.apache.org/licenses/LICENSE-2.0

//! One listing row, and a page of them.
//!
//! Deliberately not the service layer's `SessionDetail`: the shell
//! lists sessions and does not open one, so nothing here carries
//! artifact availability, provider metadata, or a resolved path.

use scrybe_application::sessions::{SessionPage, SessionState, SessionSummary};
use serde::Serialize;
use ts_rs::TS;

/// How far a session got.
///
/// Mirrors `scrybe_application::sessions::SessionState` through an
/// exhaustive match, so a new state cannot reach the frontend
/// unannounced.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum SessionProgress {
    Complete,
    Unfinished,
    Repairable,
    Failed,
}

impl From<SessionState> for SessionProgress {
    fn from(state: SessionState) -> Self {
        match state {
            SessionState::Complete => Self::Complete,
            SessionState::Unfinished => Self::Unfinished,
            SessionState::Repairable => Self::Repairable,
            SessionState::Failed => Self::Failed,
        }
    }
}

/// One row of the sessions list.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, TS)]
pub struct SessionRow {
    /// The opaque identity later commands address this session by. It
    /// is a folder name, never a path: the service layer refuses
    /// separators, traversal, and drive- or home-relative forms at
    /// construction, and resolves it beneath the configured root.
    pub id: String,
    pub progress: SessionProgress,
    pub title: Option<String>,
    /// RFC 3339. Rendered in the viewer's locale by the frontend, which
    /// is the only side that knows the viewer's locale.
    pub started_at: Option<String>,
    /// `u64` on the wire is a JSON number, not a `bigint`.
    #[ts(type = "number | null")]
    pub duration_secs: Option<u64>,
}

impl From<&SessionSummary> for SessionRow {
    fn from(summary: &SessionSummary) -> Self {
        Self {
            id: summary.id.as_str().to_owned(),
            progress: summary.state.into(),
            title: summary.title.clone(),
            started_at: summary.started_at.map(|at| at.to_rfc3339()),
            duration_secs: summary.duration_secs,
        }
    }
}

/// One window onto the sessions under the configured root.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, TS)]
pub struct SessionRows {
    pub rows: Vec<SessionRow>,
    pub offset: usize,
    pub total: usize,
    pub has_more: bool,
}

impl From<SessionPage> for SessionRows {
    fn from(page: SessionPage) -> Self {
        Self {
            rows: page.items.iter().map(SessionRow::from).collect(),
            offset: page.offset,
            total: page.total,
            has_more: page.has_more,
        }
    }
}
