// Copyright 2026 Mathews Tom
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//     https://www.apache.org/licenses/LICENSE-2.0

//! The one process-wide recording state model.
//!
//! Every surface that can start or stop a recording — the CLI signal
//! bridge, the macOS tray, the floating pill, the global hotkey, and
//! any future window — drives this controller and renders from its
//! snapshot. None of them owns a clock, a stop flag, or a notion of
//! "recording" of its own, so two surfaces cannot disagree about what
//! is happening.
//!
//! Three things are decided here rather than by callers:
//!
//! - **One clock origin.** Elapsed time is measured from the single
//!   monotonic instant capture began, and freezes the moment the
//!   controller enters [`RecordingState::Saving`]. Nothing counts
//!   ticks or sums event timestamps.
//! - **One accepted stop.** At most one stop request per recording is
//!   accepted, whichever surface wins the race. Every later request is
//!   reported as already stopping and changes nothing.
//! - **Where a failure happened.** A failure is labelled from the state
//!   it happened in: preflight, capture, or finalization. A caller
//!   cannot mislabel one, because it does not choose the label.
//!
//! The state lock is never held across a callback. Each transition
//! mutates under the lock, releases it, and only then notifies the
//! observer, so an observer that reads the controller back cannot
//! deadlock against the transition that woke it.

use std::fmt;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use crate::error::{ApplicationError, ErrorCode};
use crate::recording::contract::{
    RecordingEvent, RecordingFailure, RecordingFailureKind, RecordingSnapshot, RecordingState,
    StopAcceptance, StopSource, RECORDING_EVENT_SCHEMA_VERSION,
};
use crate::Result;

/// The monotonic time source elapsed recording time is measured against.
pub trait MonotonicClock: Send + Sync + fmt::Debug {
    /// The current instant. Must never go backwards.
    fn now(&self) -> Instant;
}

/// The process's monotonic clock.
#[derive(Clone, Copy, Debug, Default)]
pub struct SystemMonotonicClock;

impl MonotonicClock for SystemMonotonicClock {
    fn now(&self) -> Instant {
        Instant::now()
    }
}

/// Receives one call per state transition, in sequence order.
pub type RecordingEventObserver = dyn Fn(&RecordingEvent) + Send + Sync;

#[derive(Debug)]
struct ControllerState {
    state: RecordingState,
    /// The single monotonic instant capture began.
    started_at: Option<Instant>,
    /// Elapsed time as of the accepted stop. Set once, never updated.
    frozen_elapsed: Option<Duration>,
    stop_requested: bool,
    stop_source: Option<StopSource>,
    failure: Option<RecordingFailure>,
    sequence: u64,
}

impl ControllerState {
    const fn idle() -> Self {
        Self {
            state: RecordingState::Idle,
            started_at: None,
            frozen_elapsed: None,
            stop_requested: false,
            stop_source: None,
            failure: None,
            sequence: 0,
        }
    }

    fn elapsed(&self, now: Instant) -> Duration {
        self.frozen_elapsed.unwrap_or_else(|| {
            self.started_at.map_or(Duration::ZERO, |origin| {
                now.saturating_duration_since(origin)
            })
        })
    }

    fn snapshot(&self, now: Instant) -> RecordingSnapshot {
        RecordingSnapshot {
            schema_version: RECORDING_EVENT_SCHEMA_VERSION,
            state: self.state,
            elapsed_ms: duration_ms(self.elapsed(now)),
            stop_requested: self.stop_requested,
            stop_source: self.stop_source,
            failure: self.failure.clone(),
        }
    }
}

fn duration_ms(duration: Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}

/// The process-wide recording state model.
pub struct RecordingController {
    clock: Arc<dyn MonotonicClock>,
    observer: Option<Arc<RecordingEventObserver>>,
    state: Mutex<ControllerState>,
}

impl fmt::Debug for RecordingController {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RecordingController")
            .field("clock", &self.clock)
            .field("observing", &self.observer.is_some())
            .finish_non_exhaustive()
    }
}

impl RecordingController {
    /// An idle controller on the process's monotonic clock.
    #[must_use]
    pub fn new() -> Self {
        Self::with_clock(Arc::new(SystemMonotonicClock))
    }

    /// An idle controller on `clock`.
    #[must_use]
    pub fn with_clock(clock: Arc<dyn MonotonicClock>) -> Self {
        Self {
            clock,
            observer: None,
            state: Mutex::new(ControllerState::idle()),
        }
    }

    /// The same controller, notifying `observer` once per transition.
    #[must_use]
    pub fn observing(mut self, observer: Arc<RecordingEventObserver>) -> Self {
        self.observer = Some(observer);
        self
    }

    /// What every surface renders from.
    #[must_use]
    pub fn snapshot(&self) -> RecordingSnapshot {
        let now = self.clock.now();
        self.lock()
            .map_or_else(poisoned_snapshot, |state| state.snapshot(now))
    }

    /// Begins resolving configuration, permissions, devices, providers,
    /// model readiness, storage, and capture construction.
    ///
    /// No session folder exists yet, so a failure from here leaves
    /// nothing on disk.
    ///
    /// # Errors
    ///
    /// [`ErrorCode::RecordingStateConflict`] unless the controller is
    /// idle.
    pub fn begin_preparing(&self) -> Result<RecordingSnapshot> {
        self.transition(RecordingState::Preparing, |state| {
            if !state.state.accepts_start() {
                return Err(conflict("start", state.state));
            }
            *state = ControllerState {
                sequence: state.sequence,
                ..ControllerState::idle()
            };
            Ok(())
        })
    }

    /// Capture has begun. Starts the one monotonic clock origin.
    ///
    /// A stop requested during preflight is honored here rather than
    /// dropped: the controller goes straight to
    /// [`RecordingState::Saving`] with no elapsed time.
    ///
    /// # Errors
    ///
    /// [`ErrorCode::RecordingStateConflict`] unless the controller is
    /// preparing.
    pub fn mark_recording(&self) -> Result<RecordingSnapshot> {
        let now = self.clock.now();
        let stopped_during_preflight = self
            .lock()
            .is_some_and(|state| state.stop_requested && state.state == RecordingState::Preparing);
        let target = if stopped_during_preflight {
            RecordingState::Saving
        } else {
            RecordingState::Recording
        };
        self.transition(target, move |state| {
            if state.state != RecordingState::Preparing {
                return Err(conflict("begin recording", state.state));
            }
            state.started_at = Some(now);
            if stopped_during_preflight {
                state.frozen_elapsed = Some(Duration::ZERO);
            }
            Ok(())
        })
    }

    /// Requests a stop on behalf of `source`.
    ///
    /// Idempotent: at most one request per recording is accepted and
    /// every later one changes nothing, whichever surface it comes
    /// from. Accepting a stop while recording freezes elapsed time and
    /// enters [`RecordingState::Saving`] in the same step, so no
    /// surface can show a timer that is still running after the user
    /// pressed stop.
    pub fn request_stop(&self, source: StopSource) -> StopAcceptance {
        let now = self.clock.now();
        let Some(mut state) = self.lock() else {
            return StopAcceptance::NotRecording;
        };

        if state.stop_requested {
            return StopAcceptance::AlreadyStopping;
        }
        match state.state {
            // Finalization is already under way, whether or not a stop
            // request started it.
            RecordingState::Saving => StopAcceptance::AlreadyStopping,
            RecordingState::Preparing => {
                // Capture has not begun, so there is no transition to
                // make yet; `mark_recording` honors the request.
                state.stop_requested = true;
                state.stop_source = Some(source);
                StopAcceptance::Accepted
            }
            RecordingState::Recording => {
                state.stop_requested = true;
                state.stop_source = Some(source);
                state.frozen_elapsed = Some(state.elapsed(now));
                let event = advance(&mut state, RecordingState::Saving, now);
                drop(state);
                self.notify(&event);
                StopAcceptance::Accepted
            }
            RecordingState::Idle | RecordingState::Completed | RecordingState::Failed => {
                StopAcceptance::NotRecording
            }
        }
    }

    /// Capture has stopped and finalization has begun without an
    /// explicit stop request — the capture source ended on its own.
    ///
    /// # Errors
    ///
    /// [`ErrorCode::RecordingStateConflict`] unless the controller is
    /// recording.
    pub fn begin_saving(&self) -> Result<RecordingSnapshot> {
        let now = self.clock.now();
        self.transition(RecordingState::Saving, move |state| {
            if state.state != RecordingState::Recording {
                return Err(conflict("begin saving", state.state));
            }
            state.frozen_elapsed = Some(state.elapsed(now));
            Ok(())
        })
    }

    /// Finalization succeeded and the session is durable.
    ///
    /// # Errors
    ///
    /// [`ErrorCode::RecordingStateConflict`] unless the controller is
    /// saving.
    pub fn complete(&self) -> Result<RecordingSnapshot> {
        self.transition(RecordingState::Completed, |state| {
            if state.state != RecordingState::Saving {
                return Err(conflict("complete", state.state));
            }
            Ok(())
        })
    }

    /// The attempt failed.
    ///
    /// The failure is labelled from the state it happened in —
    /// preflight, capture, or finalization — so the label cannot
    /// disagree with where the controller actually was.
    ///
    /// # Errors
    ///
    /// [`ErrorCode::RecordingStateConflict`] when nothing was in
    /// flight to fail.
    pub fn fail(&self, summary: impl Into<String>) -> Result<RecordingSnapshot> {
        let summary = summary.into();
        self.transition(RecordingState::Failed, move |state| {
            let kind = match state.state {
                RecordingState::Preparing => RecordingFailureKind::Preflight,
                RecordingState::Recording => RecordingFailureKind::Capture,
                RecordingState::Saving => RecordingFailureKind::Finalization,
                other => return Err(conflict("fail", other)),
            };
            state.failure = Some(RecordingFailure::new(kind, summary));
            Ok(())
        })
    }

    /// Settles a terminal state back to [`RecordingState::Idle`], so
    /// the next recording can start.
    ///
    /// # Errors
    ///
    /// [`ErrorCode::RecordingStateConflict`] unless the controller has
    /// completed or failed.
    pub fn acknowledge(&self) -> Result<RecordingSnapshot> {
        self.transition(RecordingState::Idle, |state| {
            if !state.state.is_terminal() {
                return Err(conflict("acknowledge", state.state));
            }
            // The attempt is over, so nothing about it survives into
            // the idle state. The failure was already carried by the
            // event that entered `Failed`.
            state.started_at = None;
            state.frozen_elapsed = None;
            state.stop_requested = false;
            state.stop_source = None;
            state.failure = None;
            Ok(())
        })
    }

    fn transition(
        &self,
        to: RecordingState,
        mutate: impl FnOnce(&mut ControllerState) -> Result<()>,
    ) -> Result<RecordingSnapshot> {
        let now = self.clock.now();
        let mut state = self.lock().ok_or_else(poisoned)?;
        mutate(&mut state)?;
        let event = advance(&mut state, to, now);
        let snapshot = state.snapshot(now);
        drop(state);
        self.notify(&event);
        Ok(snapshot)
    }

    fn notify(&self, event: &RecordingEvent) {
        if let Some(observer) = self.observer.as_ref() {
            observer(event);
        }
    }

    fn lock(&self) -> Option<MutexGuard<'_, ControllerState>> {
        self.state.lock().ok()
    }
}

impl Default for RecordingController {
    fn default() -> Self {
        Self::new()
    }
}

/// Moves `state` to `to` and returns the one event that describes it.
fn advance(state: &mut ControllerState, to: RecordingState, now: Instant) -> RecordingEvent {
    let from = state.state;
    state.state = to;
    state.sequence += 1;
    RecordingEvent {
        schema_version: RECORDING_EVENT_SCHEMA_VERSION,
        sequence: state.sequence,
        from,
        to,
        elapsed_ms: duration_ms(state.elapsed(now)),
        stop_source: state.stop_source,
        failure: state.failure.clone(),
    }
}

fn conflict(attempted: &str, state: RecordingState) -> ApplicationError {
    ApplicationError::new(
        ErrorCode::RecordingStateConflict,
        format!("cannot {attempted} while {state:?}"),
    )
}

fn poisoned() -> ApplicationError {
    ApplicationError::new(
        ErrorCode::RecordingStateConflict,
        "the recording state is unreadable after a panic in another thread",
    )
}

/// The snapshot reported when the state cannot be read at all. Idle is
/// the only safe answer: it offers no stop control and no running
/// timer.
const fn poisoned_snapshot() -> RecordingSnapshot {
    RecordingSnapshot {
        schema_version: RECORDING_EVENT_SCHEMA_VERSION,
        state: RecordingState::Idle,
        elapsed_ms: 0,
        stop_requested: false,
        stop_source: None,
        failure: None,
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;
    use std::sync::atomic::{AtomicU64, Ordering};

    /// A clock the test advances by hand, so elapsed-time assertions
    /// are exact rather than approximate.
    #[derive(Debug)]
    struct ManualClock {
        origin: Instant,
        offset_ms: AtomicU64,
    }

    impl ManualClock {
        fn new() -> Arc<Self> {
            Arc::new(Self {
                origin: Instant::now(),
                offset_ms: AtomicU64::new(0),
            })
        }

        fn advance(&self, millis: u64) {
            self.offset_ms.fetch_add(millis, Ordering::SeqCst);
        }
    }

    impl MonotonicClock for ManualClock {
        fn now(&self) -> Instant {
            self.origin + Duration::from_millis(self.offset_ms.load(Ordering::SeqCst))
        }
    }

    #[derive(Default)]
    struct Recorded {
        events: Mutex<Vec<RecordingEvent>>,
    }

    impl Recorded {
        fn transitions(&self) -> Vec<(RecordingState, RecordingState)> {
            self.events
                .lock()
                .unwrap()
                .iter()
                .map(|event| (event.from, event.to))
                .collect()
        }

        fn sequences(&self) -> Vec<u64> {
            self.events
                .lock()
                .unwrap()
                .iter()
                .map(|event| event.sequence)
                .collect()
        }
    }

    fn controller() -> (RecordingController, Arc<ManualClock>, Arc<Recorded>) {
        let clock = ManualClock::new();
        let recorded = Arc::new(Recorded::default());
        let sink = Arc::clone(&recorded);
        let controller =
            RecordingController::with_clock(Arc::clone(&clock) as Arc<dyn MonotonicClock>)
                .observing(Arc::new(move |event: &RecordingEvent| {
                    sink.events.lock().unwrap().push(event.clone());
                }));
        (controller, clock, recorded)
    }

    #[test]
    fn test_a_whole_recording_walks_the_documented_state_machine() {
        let (controller, clock, recorded) = controller();

        controller.begin_preparing().unwrap();
        controller.mark_recording().unwrap();
        clock.advance(5_000);
        controller.request_stop(StopSource::Tray);
        controller.complete().unwrap();
        controller.acknowledge().unwrap();

        assert_eq!(
            recorded.transitions(),
            vec![
                (RecordingState::Idle, RecordingState::Preparing),
                (RecordingState::Preparing, RecordingState::Recording),
                (RecordingState::Recording, RecordingState::Saving),
                (RecordingState::Saving, RecordingState::Completed),
                (RecordingState::Completed, RecordingState::Idle),
            ]
        );
        assert_eq!(recorded.sequences(), vec![1, 2, 3, 4, 5]);
    }

    #[test]
    fn test_only_an_idle_controller_accepts_a_start() {
        let (controller, _clock, _recorded) = controller();
        controller.begin_preparing().unwrap();

        let error = controller.begin_preparing().unwrap_err();

        assert_eq!(error.code(), ErrorCode::RecordingStateConflict);
    }

    #[test]
    fn test_saving_refuses_another_recording() {
        let (controller, _clock, _recorded) = controller();
        controller.begin_preparing().unwrap();
        controller.mark_recording().unwrap();
        controller.request_stop(StopSource::Tray);

        let error = controller.begin_preparing().unwrap_err();

        assert_eq!(error.code(), ErrorCode::RecordingStateConflict);
    }

    #[test]
    fn test_exactly_one_stop_source_is_accepted_whichever_surface_wins() {
        let (controller, _clock, _recorded) = controller();
        controller.begin_preparing().unwrap();
        controller.mark_recording().unwrap();

        let first = controller.request_stop(StopSource::Hotkey);
        let second = controller.request_stop(StopSource::Tray);
        let third = controller.request_stop(StopSource::Signal);

        assert_eq!(first, StopAcceptance::Accepted);
        assert_eq!(second, StopAcceptance::AlreadyStopping);
        assert_eq!(third, StopAcceptance::AlreadyStopping);
        assert_eq!(
            controller.snapshot().stop_source,
            Some(StopSource::Hotkey),
            "the first accepted source must survive every later request"
        );
    }

    #[test]
    fn test_a_repeated_stop_emits_no_further_transition() {
        let (controller, _clock, recorded) = controller();
        controller.begin_preparing().unwrap();
        controller.mark_recording().unwrap();

        controller.request_stop(StopSource::Tray);
        controller.request_stop(StopSource::Hotkey);
        controller.request_stop(StopSource::FloatingWindow);

        assert_eq!(
            recorded
                .transitions()
                .iter()
                .filter(|(_, to)| *to == RecordingState::Saving)
                .count(),
            1
        );
    }

    #[test]
    fn test_stopping_an_idle_controller_reports_that_nothing_was_recording() {
        let (controller, _clock, _recorded) = controller();

        assert_eq!(
            controller.request_stop(StopSource::Signal),
            StopAcceptance::NotRecording
        );
    }

    #[test]
    fn test_elapsed_time_runs_from_one_origin_and_freezes_at_the_stop() {
        let (controller, clock, _recorded) = controller();
        controller.begin_preparing().unwrap();
        clock.advance(2_000);
        controller.mark_recording().unwrap();
        clock.advance(7_000);

        assert_eq!(controller.snapshot().elapsed_ms, 7_000);

        controller.request_stop(StopSource::Tray);
        clock.advance(60_000);

        assert_eq!(
            controller.snapshot().elapsed_ms,
            7_000,
            "time spent saving must not be counted as recorded time"
        );
        assert_eq!(controller.snapshot().elapsed_label(), "00:07");
    }

    #[test]
    fn test_a_stop_during_preflight_is_honored_rather_than_dropped() {
        let (controller, clock, _recorded) = controller();
        controller.begin_preparing().unwrap();

        let accepted = controller.request_stop(StopSource::Signal);
        clock.advance(3_000);
        let snapshot = controller.mark_recording().unwrap();

        assert_eq!(accepted, StopAcceptance::Accepted);
        assert_eq!(snapshot.state, RecordingState::Saving);
        assert_eq!(snapshot.elapsed_ms, 0);
    }

    #[test]
    fn test_a_preflight_failure_is_labelled_preflight() {
        let (controller, _clock, _recorded) = controller();
        controller.begin_preparing().unwrap();

        let snapshot = controller.fail("no microphone was available").unwrap();

        assert_eq!(
            snapshot.failure.map(|failure| failure.kind),
            Some(RecordingFailureKind::Preflight)
        );
    }

    #[test]
    fn test_a_failure_while_recording_is_labelled_capture() {
        let (controller, _clock, _recorded) = controller();
        controller.begin_preparing().unwrap();
        controller.mark_recording().unwrap();

        let snapshot = controller.fail("the capture device disappeared").unwrap();

        assert_eq!(
            snapshot.failure.map(|failure| failure.kind),
            Some(RecordingFailureKind::Capture)
        );
    }

    #[test]
    fn test_a_failure_while_saving_is_labelled_finalization() {
        let (controller, _clock, _recorded) = controller();
        controller.begin_preparing().unwrap();
        controller.mark_recording().unwrap();
        controller.request_stop(StopSource::Tray);

        let snapshot = controller.fail("the journal could not be merged").unwrap();

        assert_eq!(
            snapshot.failure.map(|failure| failure.kind),
            Some(RecordingFailureKind::Finalization)
        );
    }

    #[test]
    fn test_a_failed_attempt_returns_to_idle_and_a_new_recording_may_start() {
        let (controller, _clock, recorded) = controller();
        controller.begin_preparing().unwrap();
        controller.fail("no microphone was available").unwrap();

        controller.acknowledge().unwrap();

        assert_eq!(controller.snapshot().state, RecordingState::Idle);
        assert!(controller.snapshot().failure.is_none());
        assert!(controller.begin_preparing().is_ok());
        assert!(recorded
            .transitions()
            .contains(&(RecordingState::Failed, RecordingState::Idle)));
    }

    #[test]
    fn test_a_capture_source_that_ends_on_its_own_still_freezes_elapsed_time() {
        let (controller, clock, _recorded) = controller();
        controller.begin_preparing().unwrap();
        controller.mark_recording().unwrap();
        clock.advance(4_000);

        controller.begin_saving().unwrap();
        clock.advance(30_000);

        let snapshot = controller.snapshot();
        assert_eq!(snapshot.state, RecordingState::Saving);
        assert_eq!(snapshot.elapsed_ms, 4_000);
        assert!(!snapshot.stop_requested);
    }

    #[test]
    fn test_the_stop_control_is_disabled_the_moment_a_stop_is_accepted() {
        let (controller, _clock, _recorded) = controller();
        controller.begin_preparing().unwrap();
        controller.mark_recording().unwrap();
        assert!(controller.snapshot().stop_enabled());

        controller.request_stop(StopSource::FloatingWindow);

        assert!(!controller.snapshot().stop_enabled());
    }

    #[test]
    fn test_no_transition_event_carries_recorded_content() {
        let (controller, clock, recorded) = controller();
        controller.begin_preparing().unwrap();
        controller.mark_recording().unwrap();
        clock.advance(1_000);
        controller.request_stop(StopSource::Tray);
        controller.complete().unwrap();

        for event in recorded.events.lock().unwrap().iter() {
            let encoded = serde_json::to_value(event).unwrap();
            let keys: Vec<&str> = encoded
                .as_object()
                .unwrap()
                .keys()
                .map(String::as_str)
                .collect();
            for key in keys {
                assert!(
                    matches!(
                        key,
                        "schema_version"
                            | "sequence"
                            | "from"
                            | "to"
                            | "elapsed_ms"
                            | "stop_source"
                            | "failure"
                    ),
                    "unexpected field {key} on a recording event"
                );
            }
        }
    }

    #[test]
    fn test_an_observer_may_read_the_controller_back_without_deadlocking() {
        let clock = ManualClock::new();
        let seen: Arc<Mutex<Vec<RecordingState>>> = Arc::new(Mutex::new(Vec::new()));
        let controller = Arc::new(Mutex::new(None::<Arc<RecordingController>>));

        let observer_controller = Arc::clone(&controller);
        let observer_seen = Arc::clone(&seen);
        let built = Arc::new(
            RecordingController::with_clock(clock as Arc<dyn MonotonicClock>).observing(Arc::new(
                move |_event: &RecordingEvent| {
                    if let Some(controller) = observer_controller.lock().unwrap().as_ref() {
                        observer_seen
                            .lock()
                            .unwrap()
                            .push(controller.snapshot().state);
                    }
                },
            )),
        );
        *controller.lock().unwrap() = Some(Arc::clone(&built));

        built.begin_preparing().unwrap();
        built.mark_recording().unwrap();

        assert_eq!(
            *seen.lock().unwrap(),
            vec![RecordingState::Preparing, RecordingState::Recording]
        );
    }
}
