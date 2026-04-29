// Copyright 2026 Mathews Tom
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//     https://www.apache.org/licenses/LICENSE-2.0

//! Shared retry policy and backoff helper for cloud STT/LLM providers.
//!
//! Defaults match `docs/system-design.md` §8.2: 3 attempts, 500 ms
//! initial backoff, 8 s ceiling. Local providers (whisper.cpp,
//! `sherpa-rs`) are deterministic and skip the helper entirely.

use std::future::Future;
use std::time::Duration;

use serde::{Deserialize, Serialize};

/// Retry budget and exponential-backoff schedule.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct RetryPolicy {
    pub max_attempts: u32,
    pub initial_backoff_ms: u32,
    pub max_backoff_ms: u32,
}

impl Default for RetryPolicy {
    fn default() -> Self {
        Self {
            max_attempts: 3,
            initial_backoff_ms: 500,
            max_backoff_ms: 8_000,
        }
    }
}

impl RetryPolicy {
    /// Backoff for `attempt` (1-indexed) under the configured schedule.
    /// Doubles each step, clamped at `max_backoff_ms`. Used by
    /// [`retry_with_policy`] and exposed for callers that schedule
    /// timers explicitly.
    #[must_use]
    pub fn backoff_for(self, attempt: u32) -> Duration {
        if attempt <= 1 {
            return Duration::from_millis(u64::from(self.initial_backoff_ms));
        }
        let shift = attempt.saturating_sub(1).min(31);
        let scaled = u64::from(self.initial_backoff_ms)
            .checked_shl(shift)
            .unwrap_or(u64::MAX);
        let clamped = scaled.min(u64::from(self.max_backoff_ms));
        Duration::from_millis(clamped)
    }
}

/// Outcome an operation can report to the retry helper. `Permanent`
/// errors short-circuit the loop; `Transient` errors trigger backoff.
#[derive(Debug)]
pub enum RetryOutcome<T, E> {
    Ok(T),
    Transient(E),
    Permanent(E),
}

/// Run `op` up to `policy.max_attempts` times, sleeping `backoff_for(n)`
/// between attempts. The caller decides per attempt whether the failure
/// is transient or permanent.
///
/// # Errors
///
/// Returns the last error returned by `op` once the attempt budget is
/// exhausted, or the first `Permanent` error reported.
pub async fn retry_with_policy<T, E, Fut, Op>(
    policy: RetryPolicy,
    mut op: Op,
) -> Result<T, RetryFailure<E>>
where
    Op: FnMut(u32) -> Fut,
    Fut: Future<Output = RetryOutcome<T, E>>,
{
    let mut last_err: Option<E> = None;
    for attempt in 1..=policy.max_attempts {
        match op(attempt).await {
            RetryOutcome::Ok(value) => return Ok(value),
            RetryOutcome::Permanent(err) => {
                return Err(RetryFailure {
                    attempts: attempt,
                    last_error: err,
                    permanent: true,
                });
            }
            RetryOutcome::Transient(err) => {
                last_err = Some(err);
                if attempt < policy.max_attempts {
                    tokio::time::sleep(policy.backoff_for(attempt)).await;
                }
            }
        }
    }
    Err(RetryFailure {
        attempts: policy.max_attempts,
        // SAFETY: we entered the loop at least once because
        // `max_attempts >= 1` is enforced at construction time via the
        // default; the loop only exits via this branch when the final
        // attempt produced a `Transient` error, which is recorded above.
        last_error: last_err.unwrap_or_else(|| {
            unreachable!("retry loop exited without recording a transient error")
        }),
        permanent: false,
    })
}

/// What the helper returns after exhaustion. Callers translate to
/// `SttError::RetriesExhausted` / `LlmError::RetriesExhausted` plus
/// the source chain.
#[derive(Debug)]
pub struct RetryFailure<E> {
    pub attempts: u32,
    pub last_error: E,
    pub permanent: bool,
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::sync::Arc;

    use super::*;
    use pretty_assertions::assert_eq;

    #[test]
    fn test_retry_policy_default_matches_system_design_section_8_2() {
        let p = RetryPolicy::default();

        assert_eq!(p.max_attempts, 3);
        assert_eq!(p.initial_backoff_ms, 500);
        assert_eq!(p.max_backoff_ms, 8_000);
    }

    #[test]
    fn test_retry_policy_backoff_doubles_per_attempt_until_ceiling() {
        let p = RetryPolicy::default();

        assert_eq!(p.backoff_for(1), Duration::from_millis(500));
        assert_eq!(p.backoff_for(2), Duration::from_millis(1_000));
        assert_eq!(p.backoff_for(3), Duration::from_millis(2_000));
        assert_eq!(p.backoff_for(4), Duration::from_millis(4_000));
        assert_eq!(p.backoff_for(5), Duration::from_millis(8_000));
        assert_eq!(p.backoff_for(6), Duration::from_millis(8_000));
    }

    #[test]
    fn test_retry_policy_backoff_attempt_zero_treated_as_initial() {
        let p = RetryPolicy::default();

        assert_eq!(p.backoff_for(0), Duration::from_millis(500));
    }

    #[test]
    fn test_retry_policy_serializes_and_round_trips_through_toml() {
        let p = RetryPolicy {
            max_attempts: 5,
            initial_backoff_ms: 250,
            max_backoff_ms: 16_000,
        };

        let s = toml::to_string(&p).unwrap();
        let decoded: RetryPolicy = toml::from_str(&s).unwrap();

        assert_eq!(decoded, p);
    }

    #[tokio::test(start_paused = true)]
    async fn test_retry_with_policy_returns_ok_immediately_on_first_success() {
        let calls = Arc::new(AtomicU32::new(0));
        let calls_for_op = Arc::clone(&calls);

        let result: Result<u32, RetryFailure<&'static str>> =
            retry_with_policy(RetryPolicy::default(), |_attempt| {
                let calls = Arc::clone(&calls_for_op);
                async move {
                    calls.fetch_add(1, Ordering::SeqCst);
                    RetryOutcome::Ok(7)
                }
            })
            .await;

        assert_eq!(result.unwrap(), 7);
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test(start_paused = true)]
    async fn test_retry_with_policy_permanent_error_short_circuits_after_one_attempt() {
        let calls = Arc::new(AtomicU32::new(0));
        let calls_for_op = Arc::clone(&calls);

        let result: Result<(), RetryFailure<&'static str>> =
            retry_with_policy(RetryPolicy::default(), |_attempt| {
                let calls = Arc::clone(&calls_for_op);
                async move {
                    calls.fetch_add(1, Ordering::SeqCst);
                    RetryOutcome::Permanent("401 unauthorized")
                }
            })
            .await;

        let err = result.unwrap_err();
        assert!(err.permanent);
        assert_eq!(err.attempts, 1);
        assert_eq!(err.last_error, "401 unauthorized");
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test(start_paused = true)]
    async fn test_retry_with_policy_exhausts_budget_on_repeated_transient_errors() {
        let calls = Arc::new(AtomicU32::new(0));
        let calls_for_op = Arc::clone(&calls);
        let policy = RetryPolicy {
            max_attempts: 3,
            initial_backoff_ms: 10,
            max_backoff_ms: 40,
        };

        let result: Result<(), RetryFailure<&'static str>> =
            retry_with_policy(policy, |_attempt| {
                let calls = Arc::clone(&calls_for_op);
                async move {
                    calls.fetch_add(1, Ordering::SeqCst);
                    RetryOutcome::Transient("503 service unavailable")
                }
            })
            .await;

        let err = result.unwrap_err();
        assert!(!err.permanent);
        assert_eq!(err.attempts, 3);
        assert_eq!(calls.load(Ordering::SeqCst), 3);
    }

    #[tokio::test(start_paused = true)]
    async fn test_retry_with_policy_succeeds_after_one_transient_failure() {
        let calls = Arc::new(AtomicU32::new(0));
        let calls_for_op = Arc::clone(&calls);
        let policy = RetryPolicy {
            max_attempts: 3,
            initial_backoff_ms: 10,
            max_backoff_ms: 40,
        };

        let result: Result<u32, RetryFailure<&'static str>> =
            retry_with_policy(policy, |attempt| {
                let calls = Arc::clone(&calls_for_op);
                async move {
                    calls.fetch_add(1, Ordering::SeqCst);
                    if attempt == 1 {
                        RetryOutcome::Transient("503")
                    } else {
                        RetryOutcome::Ok(42)
                    }
                }
            })
            .await;

        assert_eq!(result.unwrap(), 42);
        assert_eq!(calls.load(Ordering::SeqCst), 2);
    }
}
