// Copyright 2026 Mathews Tom
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//     https://www.apache.org/licenses/LICENSE-2.0

//! `Hook` trait, `LifecycleEvent` enum, and the concurrent dispatcher
//! (`docs/system-design.md` §4.5).
//!
//! Tier-1 stability: `LifecycleEvent` variants are frozen at v1.0
//! because `meta.toml` records hook outcomes against these names.
//! Adding a variant is a major-version change.

pub mod git;
#[cfg(feature = "hook-webhook")]
pub mod webhook;

pub use git::{GitHook, GitHookConfig};
#[cfg(feature = "hook-webhook")]
pub use webhook::{sign_body as webhook_sign_body, WebhookHook, WebhookHookConfig};

use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use futures::future::join_all;
use tracing::{debug, warn};

use crate::context::MeetingContext;
use crate::error::HookError;
use crate::types::{AttributedChunk, ConsentAttestation, SessionId};

/// Type-erased error suitable for wrapping into a `LifecycleEvent`.
/// Implementers store this on `SessionFailed` / `HookFailed`.
pub type DynError = Arc<dyn std::error::Error + Send + Sync + 'static>;

/// Pipeline-emitted events that hooks subscribe to.
#[derive(Clone, Debug)]
pub enum LifecycleEvent {
    SessionStart {
        id: SessionId,
        ctx: Arc<MeetingContext>,
    },
    ConsentRecorded {
        id: SessionId,
        attestation: ConsentAttestation,
    },
    ChunkTranscribed {
        id: SessionId,
        chunk: AttributedChunk,
    },
    SessionEnd {
        id: SessionId,
        transcript_path: PathBuf,
    },
    NotesGenerated {
        id: SessionId,
        notes_path: PathBuf,
    },
    SessionFailed {
        id: SessionId,
        error: DynError,
    },
    HookFailed {
        id: SessionId,
        hook_name: String,
        error: DynError,
    },
}

impl LifecycleEvent {
    /// The session this event belongs to. Useful for routing logs and
    /// for the `meta.toml` writer that joins by `SessionId`.
    #[must_use]
    pub const fn session_id(&self) -> SessionId {
        match self {
            Self::SessionStart { id, .. }
            | Self::ConsentRecorded { id, .. }
            | Self::ChunkTranscribed { id, .. }
            | Self::SessionEnd { id, .. }
            | Self::NotesGenerated { id, .. }
            | Self::SessionFailed { id, .. }
            | Self::HookFailed { id, .. } => *id,
        }
    }

    /// Stable kind tag for filtering and logging.
    #[must_use]
    pub const fn kind(&self) -> &'static str {
        match self {
            Self::SessionStart { .. } => "session_start",
            Self::ConsentRecorded { .. } => "consent_recorded",
            Self::ChunkTranscribed { .. } => "chunk_transcribed",
            Self::SessionEnd { .. } => "session_end",
            Self::NotesGenerated { .. } => "notes_generated",
            Self::SessionFailed { .. } => "session_failed",
            Self::HookFailed { .. } => "hook_failed",
        }
    }
}

/// Lifecycle subscriber. Implementations are statically registered in
/// the CLI binary behind cargo features; there is no dynamic loader.
#[async_trait]
pub trait Hook: Send + Sync {
    /// Handle a single event. Implementations MUST be tolerant of
    /// receiving every event variant — filter on `event.kind()` and
    /// return `Ok(())` for ones that do not apply.
    ///
    /// # Errors
    ///
    /// `HookError::Timeout` when the implementation enforces its own
    /// timeout, `HookError::Hook` for any other failure.
    async fn on_event(&self, event: &LifecycleEvent) -> Result<(), HookError>;

    /// Stable hook identifier surfaced in `meta.toml`'s `[hooks]` table.
    fn name(&self) -> &str;
}

/// Outcome of dispatching one event across registered hooks.
#[derive(Debug)]
pub struct DispatchOutcome {
    pub failures: Vec<LifecycleEvent>,
}

impl DispatchOutcome {
    /// True when every hook acknowledged the event without error.
    #[must_use]
    pub fn all_ok(&self) -> bool {
        self.failures.is_empty()
    }
}

/// Dispatch `event` across `hooks` concurrently.
///
/// One hook's failure does not block siblings. For each failure, a
/// synthetic `LifecycleEvent::HookFailed` is appended to the returned
/// outcome's `failures` so the session-end recorder can persist the
/// disposition in `meta.toml`.
pub async fn dispatch_hooks(hooks: &[Box<dyn Hook>], event: &LifecycleEvent) -> DispatchOutcome {
    let id = event.session_id();
    let futures = hooks.iter().map(|h| async move {
        let name = h.name().to_string();
        let result = h.on_event(event).await;
        (name, result)
    });

    let results = join_all(futures).await;
    let mut failures = Vec::new();

    for (name, result) in results {
        match result {
            Ok(()) => {
                debug!(hook = %name, kind = %event.kind(), "hook completed");
            }
            Err(err) => {
                warn!(hook = %name, kind = %event.kind(), error = %err, "hook failed");
                failures.push(LifecycleEvent::HookFailed {
                    id,
                    hook_name: name,
                    error: Arc::new(err),
                });
            }
        }
    }

    DispatchOutcome { failures }
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::unnecessary_literal_bound
)]
mod tests {
    use std::sync::Mutex;
    use std::time::Duration;

    use super::*;
    use pretty_assertions::assert_eq;

    fn evt(id: SessionId) -> LifecycleEvent {
        LifecycleEvent::SessionStart {
            id,
            ctx: Arc::new(MeetingContext::default()),
        }
    }

    struct Recorder {
        identifier: &'static str,
        seen: Arc<Mutex<Vec<&'static str>>>,
        delay: Option<Duration>,
        fail: bool,
    }

    #[async_trait]
    impl Hook for Recorder {
        async fn on_event(&self, event: &LifecycleEvent) -> Result<(), HookError> {
            if let Some(d) = self.delay {
                tokio::time::sleep(d).await;
            }
            self.seen.lock().unwrap().push(event.kind());
            if self.fail {
                Err(HookError::Hook(Box::new(std::io::Error::new(
                    std::io::ErrorKind::Other,
                    format!("{} simulated failure", self.identifier),
                ))))
            } else {
                Ok(())
            }
        }
        fn name(&self) -> &str {
            self.identifier
        }
    }

    #[test]
    fn test_lifecycle_event_kind_matches_variant_name_in_snake_case() {
        let id = SessionId::new();
        let cases: Vec<(LifecycleEvent, &'static str)> = vec![
            (evt(id), "session_start"),
            (
                LifecycleEvent::SessionEnd {
                    id,
                    transcript_path: PathBuf::from("/tmp/t.md"),
                },
                "session_end",
            ),
            (
                LifecycleEvent::NotesGenerated {
                    id,
                    notes_path: PathBuf::from("/tmp/n.md"),
                },
                "notes_generated",
            ),
        ];
        for (event, expected) in cases {
            assert_eq!(event.kind(), expected);
        }
    }

    #[test]
    fn test_lifecycle_event_session_id_returns_owning_session() {
        let id = SessionId::new();
        let event = evt(id);

        assert_eq!(event.session_id(), id);
    }

    fn boxed(
        identifier: &'static str,
        seen: Arc<Mutex<Vec<&'static str>>>,
        delay: Option<Duration>,
        fail: bool,
    ) -> Box<dyn Hook> {
        Box::new(Recorder {
            identifier,
            seen,
            delay,
            fail,
        })
    }

    #[tokio::test]
    async fn test_dispatch_hooks_runs_all_hooks_for_a_single_event() {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let hooks: Vec<Box<dyn Hook>> = vec![
            boxed("git", Arc::clone(&seen), None, false),
            boxed("webhook", Arc::clone(&seen), None, false),
        ];

        let outcome = dispatch_hooks(&hooks, &evt(SessionId::new())).await;

        assert!(outcome.all_ok());
        assert_eq!(seen.lock().unwrap().len(), 2);
    }

    #[tokio::test(start_paused = true)]
    async fn test_dispatch_hooks_runs_concurrently_not_serially() {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let hooks: Vec<Box<dyn Hook>> = vec![
            boxed(
                "slow_a",
                Arc::clone(&seen),
                Some(Duration::from_millis(500)),
                false,
            ),
            boxed(
                "slow_b",
                Arc::clone(&seen),
                Some(Duration::from_millis(500)),
                false,
            ),
        ];

        let start = tokio::time::Instant::now();
        let outcome = dispatch_hooks(&hooks, &evt(SessionId::new())).await;
        let elapsed = start.elapsed();

        assert!(outcome.all_ok());
        assert!(
            elapsed < Duration::from_millis(900),
            "expected concurrent dispatch under 900 ms; got {elapsed:?}"
        );
    }

    #[tokio::test]
    async fn test_dispatch_hooks_one_failure_does_not_block_siblings() {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let hooks: Vec<Box<dyn Hook>> = vec![
            boxed("failing", Arc::clone(&seen), None, true),
            boxed("ok", Arc::clone(&seen), None, false),
        ];

        let outcome = dispatch_hooks(&hooks, &evt(SessionId::new())).await;

        assert!(!outcome.all_ok());
        assert_eq!(outcome.failures.len(), 1);
        match &outcome.failures[0] {
            LifecycleEvent::HookFailed { hook_name, .. } => {
                assert_eq!(hook_name, "failing");
            }
            other => panic!("expected HookFailed, got {other:?}"),
        }
        assert_eq!(seen.lock().unwrap().len(), 2);
    }

    #[tokio::test]
    async fn test_dispatch_hooks_failure_records_session_id_and_hook_name() {
        let id = SessionId::new();
        let seen = Arc::new(Mutex::new(Vec::new()));
        let hooks: Vec<Box<dyn Hook>> = vec![boxed("git", seen, None, true)];

        let outcome = dispatch_hooks(&hooks, &evt(id)).await;

        assert_eq!(outcome.failures.len(), 1);
        let failure = &outcome.failures[0];
        assert_eq!(failure.session_id(), id);
        assert_eq!(failure.kind(), "hook_failed");
    }

    #[tokio::test]
    async fn test_dispatch_hooks_with_no_hooks_returns_all_ok() {
        let hooks: Vec<Box<dyn Hook>> = vec![];

        let outcome = dispatch_hooks(&hooks, &evt(SessionId::new())).await;

        assert!(outcome.all_ok());
        assert!(outcome.failures.is_empty());
    }
}
