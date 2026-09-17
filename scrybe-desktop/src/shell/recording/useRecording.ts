import { useCallback, useEffect, useState } from "react";

import { useScrybe } from "../../ipc/ScrybeProvider";
import type { RecordingStatus } from "../../generated/bindings";

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
export function useRecording(): Recording {
  const scrybe = useScrybe();
  const [status, setStatus] = useState<RecordingStatus | null>(null);
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
    void scrybe.onRecordingTransition(() => {
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

  return {
    status,
    elapsed: status === null || status.state === "idle" ? "" : elapsedLabel(status.elapsed_ms),
    stopEnabled: live && !(status?.stop_requested ?? true),
    refresh,
  };
}
