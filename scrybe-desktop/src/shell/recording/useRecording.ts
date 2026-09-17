import { useCallback, useEffect, useRef, useState } from "react";

import { useScrybe } from "../../ipc/ScrybeProvider";
import type {
  RecordingProgressView,
  RecordingStatus,
} from "../../generated/bindings";

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

/** The state model every recording surface in this window renders. */
export interface Recording {
  status: RecordingStatus | null;
  /** `HH:MM:SS`, or `MM:SS` under an hour. Empty while idle. */
  elapsed: string;
  /** Whether a stop control should be enabled. */
  stopEnabled: boolean;
  /** Whether a new recording may be started right now. */
  startEnabled: boolean;
  /** How far through saving, while saving. `null` otherwise. */
  progress: RecordingProgressView | null;
  /** The last refusal or failure, until the next attempt clears it. */
  error: string | null;
  /** Starts a recording with the given title. */
  start: (title: string) => void;
  /** Asks the recording in flight to stop and save. */
  stop: () => void;
  /** Re-reads the host's snapshot now. */
  refresh: () => void;
}

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
 * `HH:MM:SS`, or `MM:SS` under an hour.
 *
 * The same rule `RecordingSnapshot::elapsed_label` applies in Rust.
 * Formatted here rather than sent over the wire because the number is
 * what the host owns and the shape is what the view chooses; a locale
 * or a compact indicator can render the same milliseconds differently
 * without the host having to know.
 */
export function elapsedLabel(milliseconds: number): string {
  const total = Math.floor(milliseconds / 1000);
  const seconds = String(total % 60).padStart(2, "0");
  const minutes = String(Math.floor(total / 60) % 60).padStart(2, "0");
  const hours = Math.floor(total / 3600);
  return hours === 0
    ? `${minutes}:${seconds}`
    : `${String(hours).padStart(2, "0")}:${minutes}:${seconds}`;
}

/** Whether a state is one in which time is still moving. */
function isLive(status: RecordingStatus | null): boolean {
  return status?.state === "preparing" || status?.state === "recording";
}

/**
 * The host's recording state, kept current.
 *
 * Two sources, both of them the host's: a subscription that delivers
 * every transition the moment it happens, and — only while a recording
 * is live — a refresh that re-reads the elapsed milliseconds. Nothing
 * here derives a state or a duration of its own, so no surface in this
 * window can disagree with the service layer about whether a recording
 * is running.
 */
export function useRecording(onSettled?: (settled: boolean) => void): Recording {
  const scrybe = useScrybe();
  const [status, setStatus] = useState<RecordingStatus | null>(null);
  const [progress, setProgress] = useState<RecordingProgressView | null>(null);
  const [error, setError] = useState<string | null>(null);
  // Held in a ref so a caller passing an inline callback does not tear
  // the subscription down and rebuild it on every render.
  const settled = useRef(onSettled);
  useEffect(() => {
    settled.current = onSettled;
  }, [onSettled]);
  // The refresh effect below depends on this rather than on `status`,
  // so the interval is built once when a recording starts and torn down
  // once when it ends, instead of being rebuilt by every tick it
  // causes.
  const live = isLive(status);

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
      setProgress(next);
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
      // A recording that has ended, either way. A caller that wants to
      // re-check something once per recording hooks it here rather than
      // watching the rendered state, which settles for many renders.
      if (transition.to === "completed" || transition.to === "failed") {
        settled.current?.(true);
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

  return {
    status,
    elapsed:
      status === null || status.state === "idle" ? "" : elapsedLabel(status.elapsed_ms),
    stopEnabled: live && !(status?.stop_requested ?? true),
    // A new recording is refused while one is in flight, and while the
    // last one is still saving. The host refuses it too — this only
    // keeps the control from offering what would be refused.
    startEnabled: status !== null && (status.state === "idle" || status.state === "completed" || status.state === "failed"),
    progress: status?.state === "saving" ? progress : null,
    error,
    start,
    stop,
    refresh,
  };
}
