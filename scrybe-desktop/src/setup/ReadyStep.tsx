import type { ReadinessReport } from "../generated/bindings";
import { ReadinessList } from "./ReadinessList";

/**
 * The five answers, and whether anything among them stops a recording.
 *
 * Reported separately rather than reduced to one verdict: notes being
 * unavailable is visible and is deliberately not a blocker, and where
 * content goes is stated whether or not anything is wrong.
 *
 * The verdict says what was checked rather than that the Mac is ready.
 * Capture permission is not checked anywhere in this application — the
 * readiness snapshot reports it as unverified — so a verdict claiming
 * readiness would be telling a user who had refused both prompts that
 * recording will work.
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
          ? "Nothing above is blocking a recording. Scrybe does not check whether macOS has granted microphone and system-audio recording; macOS asks the first time a recording needs it."
          : "Recording is not available yet. The entries marked “Needs attention” above say why."}
      </p>
      {readiness.notes.state !== "ready" && readiness.can_record && (
        <p className="setup__note">
          Notes need a language model running on this Mac. You can finish setup without one and
          configure it later.
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
