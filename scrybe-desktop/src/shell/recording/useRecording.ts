import { useEffect, useRef } from "react";

import type { RecordingProgressView, RecordingStatus } from "../../generated/bindings";
import { useRecordingSnapshot } from "./RecordingWatcher";

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

/**
 * `RecordingWatcher`'s shared state, shaped for one surface to render.
 *
 * Reads nothing itself: `RecordingWatcher`, mounted once in `AppShell`,
 * is the only place in the window that asks the host for a terminal
 * recording's outcome. This derives `elapsed`, `stopEnabled`, and
 * `startEnabled` from whatever it already read, and calls `onSettled`
 * once per recording that reaches a terminal state — `RecordingView`
 * uses it to re-check whether this installation can still record —
 * rather than on every render the terminal state settles for.
 */
export function useRecording(onSettled?: (settled: boolean) => void): Recording {
  const { status, live, progress, error, settledCount, start, stop, refresh } =
    useRecordingSnapshot();
  // Held in a ref so a caller passing an inline callback does not tear
  // this down and rebuild it on every render.
  const settled = useRef(onSettled);
  useEffect(() => {
    settled.current = onSettled;
  }, [onSettled]);
  const previousSettledCount = useRef(settledCount);
  useEffect(() => {
    if (settledCount !== previousSettledCount.current) {
      previousSettledCount.current = settledCount;
      settled.current?.(true);
    }
  }, [settledCount]);

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
