import type { ReadinessReport } from "../generated/bindings";
import { ReadinessList } from "./ReadinessList";

/**
 * The five answers, and whether they permit a recording.
 *
 * Reported separately rather than reduced to one verdict: notes being
 * unavailable is visible and is deliberately not a blocker, and where
 * content goes is stated whether or not anything is wrong.
 */
export function ReadyStep({
  readiness,
  onRecheck,
  onFinish,
}: {
  readiness: ReadinessReport;
  onRecheck: () => void;
  onFinish: () => void;
}) {
  return (
    <>
      <ReadinessList readiness={readiness} />
      <p aria-live="polite" className="setup__verdict">
        {readiness.can_record
          ? "This Mac is ready to record."
          : "Recording is not available yet. The entries marked “Needs attention” above say why."}
      </p>
      {readiness.notes.state !== "ready" && readiness.can_record && (
        <p className="setup__note">
          Notes are unavailable. Recordings will be captured and transcribed anyway, and each one
          will record that notes were missing.
        </p>
      )}
      <div className="setup__actions">
        <button type="button" onClick={onRecheck}>
          Check again
        </button>
        <button type="button" className="setup__primary" onClick={onFinish}>
          {readiness.can_record ? "Start using Scrybe" : "Leave setup for now"}
        </button>
      </div>
    </>
  );
}
