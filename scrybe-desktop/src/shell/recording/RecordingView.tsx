import { useCallback, useId, useState } from "react";

import { useScrybe } from "../../ipc/ScrybeProvider";
import type { PreflightView, RecordingProgressView } from "../../generated/bindings";
import { useQuery } from "../useQuery";
import { useRecording } from "./useRecording";

/** What each saving step is called where a reader can see it. */
const STEP_LABELS: Record<RecordingProgressView["step"], string> = {
  finalizing_transcript: "Finalising the transcript",
  encoding_audio: "Encoding the audio",
  generating_notes: "Generating the notes",
  writing_metadata: "Writing the session details",
};

/** What each state is called where a reader can see it. */
const STATE_LABELS = {
  idle: "Ready",
  preparing: "Preparing",
  recording: "Recording",
  saving: "Saving",
  completed: "Saved",
  failed: "Did not finish",
} as const;

/**
 * Why this installation cannot record, when it cannot.
 *
 * Only the checks that actually block. The unverified ones are listed
 * separately by `UncheckedList`, because presenting "not checked"
 * alongside "failed" would read as though both had been measured.
 */
function BlockedList({ preflight }: { preflight: PreflightView }) {
  const blocking = preflight.findings.filter((finding) => finding.outcome === "failed");
  if (blocking.length === 0) {
    return null;
  }
  return (
    <div className="recording__blocked" role="alert">
      <h3>Recording is not available</h3>
      <ul>
        {blocking.map((finding) => (
          <li key={finding.check}>{finding.summary}</li>
        ))}
      </ul>
    </div>
  );
}

/**
 * What this release did not check.
 *
 * Shown rather than hidden: a reader whose recording later fails on a
 * permission deserves to have been told that nothing here measured it.
 */
function UncheckedList({ preflight }: { preflight: PreflightView }) {
  const unverified = preflight.findings.filter(
    (finding) => finding.outcome === "unverified",
  );
  if (unverified.length === 0) {
    return null;
  }
  return (
    <details className="recording__unchecked">
      <summary>{unverified.length} thing(s) this release does not check</summary>
      <ul>
        {unverified.map((finding) => (
          <li key={finding.check}>{finding.summary}</li>
        ))}
      </ul>
    </details>
  );
}

/**
 * The main window's recording control.
 *
 * Every value it renders comes from the host: the state, the elapsed
 * milliseconds, the saving step, and whether a control should be
 * enabled. Nothing here derives a duration or a state of its own, so
 * this window cannot disagree with the tray, the pill, or a terminal
 * about whether a recording is running.
 */
export function RecordingView() {
  const scrybe = useScrybe();
  // Advanced when a recording settles rather than from an effect on the
  // rendered state: an effect would re-check on every render that
  // observed a settled state, and the thing worth re-checking is the
  // settling itself, which happens once. The counter is the query's
  // identity, which is what `useQuery` keys a re-read on.
  const [checked, setChecked] = useState(0);
  const recording = useRecording(
    useCallback((settled: boolean) => {
      if (settled) {
        setChecked((previous) => previous + 1);
      }
    }, []),
  );
  const [title, setTitle] = useState("");
  const titleId = useId();
  const state = recording.status?.state ?? "idle";
  // A settled recording re-checks. A reader whose model went missing
  // mid-session should see that on the next attempt, not the next
  // launch. The counter is the query's identity, which is what
  // `useQuery` keys a re-read on; it needs no reload of its own.
  const preflight = useQuery(
    () => scrybe.recordingPreflight(),
    `preflight-${String(checked)}`,
  );

  const canRecord = preflight.status === "ready" ? preflight.value.can_record : false;

  return (
    <section className="recording" aria-labelledby="view-heading">
      <h1 id="view-heading">Record</h1>

      {preflight.status === "ready" ? (
        <>
          <BlockedList preflight={preflight.value} />
          <UncheckedList preflight={preflight.value} />
        </>
      ) : null}

      <p className="recording__state">
        <span className="recording__state-label" aria-live="polite">
          {STATE_LABELS[state]}
        </span>
        {recording.elapsed === "" ? null : (
          <span className="recording__elapsed" aria-live="off">
            {" "}
            {recording.elapsed}
          </span>
        )}
      </p>

      {recording.progress === null ? null : (
        <p className="recording__progress" aria-live="polite">
          {STEP_LABELS[recording.progress.step]} — step {recording.progress.index} of{" "}
          {recording.progress.total}
        </p>
      )}

      {recording.error === null ? null : (
        <p className="recording__error" role="alert">
          {recording.error}
        </p>
      )}

      <label htmlFor={titleId}>What is this recording called?</label>
      <input
        id={titleId}
        type="text"
        value={title}
        placeholder="untitled"
        disabled={!recording.startEnabled}
        onChange={(event) => {
          setTitle(event.target.value);
        }}
      />

      <div className="recording__controls">
        <button
          type="button"
          disabled={!recording.startEnabled || !canRecord}
          onClick={() => {
            recording.start(title);
          }}
        >
          Record
        </button>
        <button
          type="button"
          disabled={!recording.stopEnabled}
          onClick={recording.stop}
        >
          Stop &amp; save
        </button>
      </div>
    </section>
  );
}
