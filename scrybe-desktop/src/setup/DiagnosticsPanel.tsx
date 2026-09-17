import { useState } from "react";

import type { DiagnosticRow, RecoveryActionView } from "../generated/bindings";
import { useScrybe } from "../ipc/ScrybeProvider";
import { useQuery } from "../shell/useQuery";

/** What a recovery's button says, per action. */
function actionLabel(action: RecoveryActionView): string {
  switch (action.action) {
    case "create_storage_root":
      return "Create the storage root";
    case "repair_session":
      return `Repair ${action.id ?? "the session"}`;
    case "remove_stale_session_lock":
      return `Remove the stale lock on ${action.id ?? "the session"}`;
    case "remove_orphaned_partial":
      return `Delete ${action.name ?? "the leftover file"}`;
    case "remove_model_partial":
      return `Delete ${action.name ?? "the interrupted download"}`;
    case "install_transcription_model":
      return "Install it from Setup";
    case "open_system_settings":
      return `Open ${action.capability_label ?? "System Settings"}`;
    case "review_configuration":
    case "open_advanced_configuration":
      return "Open advanced configuration";
    default:
      return "Act on this";
  }
}

const SEVERITY: Record<string, string> = {
  info: "Information",
  warning: "Warning",
  error: "Problem",
};

/**
 * What diagnosis found, and the repairs a reader can choose to run.
 *
 * Opening this reads. Nothing here is applied on mount, on a timer, or
 * as a side effect of rendering: every mutation is a button a person
 * pressed.
 *
 * What decides whether a finding gets a button is the `elsewhere` test
 * in `Finding` below, and nothing else. A recovery this application
 * performs itself gets a button; `install_transcription_model` and
 * `review_configuration` get a line of text instead, because the first
 * goes through the confirmation flow in Setup and the second is not
 * something any code can do on the user's behalf. That list lives in
 * this file, so widening it is a change here.
 *
 * `mutation_required` reaches this screen on every row and is read by
 * nothing. It is the service layer's own statement about whether
 * applying an action would change anything, derived there from the
 * action's shape; it does not govern this button and never has.
 */
export function DiagnosticsPanel({ onRepaired }: { onRepaired: () => void }) {
  const scrybe = useScrybe();
  const [generation, setGeneration] = useState(0);
  const report = useQuery(() => scrybe.diagnosticsReport(), `diagnostics:${String(generation)}`);
  const [outcome, setOutcome] = useState<string | null>(null);
  const [failure, setFailure] = useState<string | null>(null);

  function apply(action: RecoveryActionView) {
    setOutcome(null);
    setFailure(null);
    scrybe.applyRecovery(action).then(
      (result) => {
        setOutcome(result.summary);
        setGeneration((previous) => previous + 1);
        onRepaired();
      },
      (error: unknown) => {
        setFailure(describe(error));
      },
    );
  }

  return (
    <section aria-labelledby="diagnostics-heading">
      <h2 id="diagnostics-heading">Diagnostics</h2>
      <p className="setup__note">
        Reading this page changes nothing. Each repair below runs only when you choose it.
      </p>
      {report.status === "loading" && <p>Checking this installation…</p>}
      {report.status === "failed" && (
        <p role="alert" className="setup__failure">
          {report.message}
        </p>
      )}
      {report.status === "ready" && (
        <>
          <p aria-live="polite">
            {report.value.warning_count === 0
              ? "Nothing needs attention."
              : `${String(report.value.warning_count)} of ${String(report.value.rows.length)} findings need attention.`}
          </p>
          <ul className="diagnostics">
            {report.value.rows.map((row) => (
              <Finding key={`${row.code}:${row.summary}`} row={row} onApply={apply} />
            ))}
          </ul>
        </>
      )}
      <button
        type="button"
        onClick={() => {
          setGeneration((previous) => previous + 1);
        }}
      >
        Check again
      </button>
      {outcome !== null && (
        <p role="status" className="setup__saved">
          {outcome}
        </p>
      )}
      {failure !== null && (
        <p role="alert" className="setup__failure">
          {failure}
        </p>
      )}
    </section>
  );
}

function Finding({
  row,
  onApply,
}: {
  row: DiagnosticRow;
  onApply: (action: RecoveryActionView) => void;
}) {
  const action = row.recovery_action;
  // A recovery this application does not perform itself is a link to
  // where it is performed, not a repair button: installing a model goes
  // through the confirmation flow in Setup, and a permission is granted
  // in System Settings.
  const elsewhere =
    action !== null &&
    (action.action === "install_transcription_model" ||
      action.action === "review_configuration");
  return (
    <li className="diagnostics__row" data-severity={row.severity}>
      <span className="diagnostics__severity">{SEVERITY[row.severity] ?? row.severity}</span>
      <span className="diagnostics__summary">{row.summary}</span>
      {action !== null && !elsewhere && (
        <button
          type="button"
          onClick={() => {
            onApply(action);
          }}
        >
          {actionLabel(action)}
        </button>
      )}
      {elsewhere && <span className="setup__note">{actionLabel(action)}</span>}
    </li>
  );
}

function describe(error: unknown): string {
  if (
    typeof error === "object" &&
    error !== null &&
    "message" in error &&
    typeof error.message === "string"
  ) {
    return error.message;
  }
  return "The application could not reach its own services.";
}
