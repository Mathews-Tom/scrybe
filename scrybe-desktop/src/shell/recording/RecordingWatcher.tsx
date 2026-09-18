import {
  createContext,
  useCallback,
  useContext,
  useEffect,
  useMemo,
  useState,
  type ReactNode,
} from "react";

import { useScrybe } from "../../ipc/ScrybeProvider";
import type { RecordingProgressView, RecordingStatus } from "../../generated/bindings";

/**
 * How often the elapsed display is refreshed while a recording runs.
 *
 * The host is asked again rather than counted forward from the last
 * answer. Elapsed time has one origin, a monotonic instant the service
 * layer owns, and a view that advanced a local counter between host
 * answers would be a second clock — it would drift, and it would keep
 * ticking through a stop the view had not heard about yet.
 */
const ELAPSED_REFRESH_MS = 500;

/**
 * The line a reader is shown for a failed command.
 *
 * The host's message is used verbatim: it is written by the service
 * layer specifically to be read, and it never carries a source chain.
 * A code with no message would leave the reader nothing to act on, so
 * the code is the fallback rather than the other way round.
 */
function failureText(error: unknown): string {
  if (typeof error !== "object" || error === null) {
    return "the recording could not be started";
  }
  if ("message" in error) {
    const { message } = error;
    if (typeof message === "string" && message.length > 0) {
      return message;
    }
  }
  if ("code" in error) {
    return String(error.code);
  }
  return "the recording could not be started";
}

/**
 * The host's recording state, kept current, shared by every surface in
 * this window rather than read independently by each one.
 */
export interface RecordingSnapshot {
  status: RecordingStatus | null;
  /** Whether `status` is one in which the elapsed clock is still moving. */
  live: boolean;
  /** How far through saving, while saving. `null` otherwise. */
  progress: RecordingProgressView | null;
  /** The last refusal or failure, until the next attempt clears it. */
  error: string | null;
  /**
   * Incremented once per recording that reaches a terminal state, after
   * `status` has already been set to that terminal snapshot. A
   * consumer that wants to react once per recording — `RecordingView`
   * re-checks whether this installation can still record — watches
   * this rather than `status`, which settles for many renders.
   */
  settledCount: number;
  /** Starts a recording with the given title. */
  start: (title: string) => void;
  /** Asks the recording in flight to stop and save. */
  stop: () => void;
  /** Re-reads the host's snapshot now. */
  refresh: () => void;
}

const RecordingWatcherContext = createContext<RecordingSnapshot | null>(null);

/**
 * Watches the host's recording state for the whole window, once.
 *
 * Mounted in `AppShell` rather than by `RecordingView`, so a terminal
 * recording is observed and acknowledged regardless of which route is
 * open. `RecordingState::accepts_start` — and the tray's own
 * `Record now` item, which is enabled from it — accepts a start only
 * from `Idle`, so a recording driven from the tray, a hotkey, or the
 * signal bridge while the reader is on another route still has to
 * return this installation to idle before another one can start. A
 * route-scoped subscription is not guaranteed to be mounted when that
 * happens; this one always is.
 *
 * Acknowledging here rather than in `RecordingView` is not merely a
 * relocation. The host leaves a terminal recording observable
 * specifically so the surface showing it can read it before it clears
 * — acknowledging first would race the read back to the same lost
 * confirmation an earlier defect in the host itself once produced. Two
 * independent readers, one rendering the outcome and one clearing it,
 * would still be that same race, just moved into two frontend
 * subscriptions instead of one host command. This is the only place
 * that reads a terminal snapshot, so there is nothing left for it to
 * race against: `RecordingView` renders `status` from here rather than
 * asking the host again itself.
 */
export function RecordingWatcher({ children }: { children: ReactNode }) {
  const scrybe = useScrybe();
  const [status, setStatus] = useState<RecordingStatus | null>(null);
  const [progress, setProgress] = useState<RecordingProgressView | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [settledCount, setSettledCount] = useState(0);
  // The refresh effect below depends on this rather than on `status`,
  // so the interval is built once when a recording starts and torn down
  // once when it ends, instead of being rebuilt by every tick it
  // causes.
  const live = status?.state === "preparing" || status?.state === "recording";

  const refresh = useCallback(() => {
    void scrybe.recordingStatus().then(setStatus, () => {
      // A status read that fails leaves the last answer in place: a
      // surface frozen on a stale state is wrong, but blanking it on a
      // transient IPC failure is worse, and the subscription below
      // delivers the next transition regardless.
    });
  }, [scrybe]);

  useEffect(refresh, [refresh]);

  useEffect(() => {
    let cancelled = false;
    let unlisten: (() => void) | undefined;
    void scrybe.onRecordingProgress((next) => {
      // Guarded by `index` rather than set unconditionally: nothing
      // orders delivery against the IPC event bridge, and a later step
      // arriving after an earlier one must not un-render progress the
      // reader has already seen move forward.
      setProgress((previous) => (previous !== null && next.index < previous.index ? previous : next));
    }).then(
      (stop) => {
        if (cancelled) {
          stop();
          return;
        }
        unlisten = stop;
      },
      () => undefined,
    );
    return () => {
      cancelled = true;
      unlisten?.();
    };
  }, [scrybe]);

  useEffect(() => {
    let cancelled = false;
    let unlisten: (() => void) | undefined;
    void scrybe.onRecordingTransition((transition) => {
      // Saving progress belongs to the recording that produced it. A
      // step left over from the last one would render beside the next
      // recording's elapsed time.
      if (transition.to === "recording" || transition.to === "idle") {
        setProgress(null);
      }
      if (transition.failure_summary !== null) {
        setError(transition.failure_summary);
      }
      if (transition.to === "completed" || transition.to === "failed") {
        // A recording that has ended, either way.
        setSettledCount((count) => count + 1);
        // The host leaves a terminal recording observable until this
        // is acknowledged, specifically so this read lands on it
        // rather than on whatever it settles to next. Acknowledging
        // before this resolves would race the read back to the same
        // lost confirmation the host no longer produces on its own —
        // this is the only reader of a terminal snapshot in the whole
        // window, so nothing else can win that race out from under it.
        void scrybe
          .recordingStatus()
          .then(setStatus, () => undefined)
          .then(() => {
            void scrybe.acknowledgeRecording();
          });
        return;
      }
      refresh();
    }).then(
      (stop) => {
        if (cancelled) {
          stop();
          return;
        }
        unlisten = stop;
      },
      () => undefined,
    );
    return () => {
      cancelled = true;
      unlisten?.();
    };
  }, [scrybe, refresh]);

  useEffect(() => {
    if (!live) {
      return undefined;
    }
    const timer = setInterval(refresh, ELAPSED_REFRESH_MS);
    return () => {
      clearInterval(timer);
    };
  }, [live, refresh]);

  const start = useCallback(
    (title: string) => {
      setError(null);
      setProgress(null);
      void scrybe.startRecording(title.trim() === "" ? null : title.trim()).then(
        setStatus,
        (failure: unknown) => {
          setError(failureText(failure));
          // The host settled its own controller; re-read rather than
          // assume, so the two cannot disagree.
          refresh();
        },
      );
    },
    [scrybe, refresh],
  );

  const stop = useCallback(() => {
    void scrybe.stopRecording().then(setStatus, (failure: unknown) => {
      setError(failureText(failure));
      refresh();
    });
  }, [scrybe, refresh]);

  const value = useMemo<RecordingSnapshot>(
    () => ({ status, live, progress, error, settledCount, start, stop, refresh }),
    [status, live, progress, error, settledCount, start, stop, refresh],
  );

  return <RecordingWatcherContext value={value}>{children}</RecordingWatcherContext>;
}

/**
 * The shared recording state `RecordingWatcher` maintains.
 *
 * @throws if no `RecordingWatcher` is mounted above the caller. Every
 * surface that renders recording state is inside `AppShell`, which
 * mounts one for the life of the window.
 */
export function useRecordingSnapshot(): RecordingSnapshot {
  const context = useContext(RecordingWatcherContext);
  if (context === null) {
    throw new Error("useRecordingSnapshot must be used within a RecordingWatcher");
  }
  return context;
}
