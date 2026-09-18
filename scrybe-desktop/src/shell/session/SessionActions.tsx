import { useEffect, useId, useState } from "react";

import type { SessionDetail } from "../../generated/bindings";
import { useScrybe } from "../../ipc/ScrybeProvider";
import { describe } from "../useQuery";
import { ConfirmRetention, useRetention } from "./retention";

/** What an action did, in the words the reader is shown. */
export interface Outcome {
  readonly kind: "note" | "problem";
  readonly message: string;
}

/** One offer: what it is called, whether it applies, and what it does. */
interface Action {
  readonly key: string;
  readonly label: string;
  /** Why it does not apply, or `null` when it does. */
  readonly blocked: string | null;
  readonly run: () => Promise<string>;
}

/**
 * The actions this session accepts, and only those.
 *
 * Eligibility comes from the service layer's own answer rather than
 * from the view's reading of the artifact list, and a blocked action
 * says why beside itself instead of disappearing. A button that is
 * simply absent leaves the reader with no way to find out what the
 * session would need; one that is present and does nothing but fail is
 * worse.
 *
 * Two of them move the session out of the listing, and both state
 * where it goes before they do it. Everything else is a read or a
 * recovery, and the only document any of them replaces is `notes.md`.
 */
export function SessionActions({
  session,
  retentionDays,
  retentionAvailable,
  onFinished,
  onRetained,
}: {
  session: SessionDetail;
  /**
   * Days a deleted session stays in the trash, as configured, or `null`
   * while that read is still in flight.
   *
   * Passed in rather than hardcoded so the confirmation cannot go on
   * saying seven after a reader changes it. `null` blocks the delete
   * rather than falling back to the default: nothing re-checks the
   * number after the confirmation, so a confirmation naming a window
   * this installation does not use would be a false statement about
   * what is about to happen to a reader's recording.
   */
  retentionDays: number | null;
  /** False while recording or saving, when moving any session is unsafe. */
  retentionAvailable: boolean;
  /**
   * Reported upward rather than rendered here, because a successful
   * action re-reads the session and this strip is rebuilt from the
   * answer. A sentence owned by the thing being rebuilt would vanish
   * exactly when the reader went looking for it.
   */
  onFinished: (outcome: Outcome) => void;
  /**
   * The session is no longer listed, so this strip and the view around
   * it are about to describe something that is not there. Re-reading
   * would only produce a refusal; the caller leaves instead.
   */
  onRetained: (outcome: Outcome) => void;
}) {
  const scrybe = useScrybe();
  const reason = useId();
  const [running, setRunning] = useState<string | null>(null);
  const retention = useRetention((result) => {
    if (result.kind === "note") {
      onRetained(result);
    } else {
      onFinished(result);
    }
  });
  useEffect(() => {
    if (!retentionAvailable && retention.pending !== null) {
      retention.cancel();
    }
  }, [retentionAvailable, retention.pending, retention]);

  function run(action: Action) {
    setRunning(action.key);
    action.run().then(
      (message) => {
        setRunning(null);
        onFinished({ kind: "note", message });
      },
      (error: unknown) => {
        setRunning(null);
        onFinished({ kind: "problem", message: describe(error) });
      },
    );
  }

  const actions: Action[] = [
    {
      key: "regenerate",
      label: "Regenerate notes",
      blocked: session.actions.regenerate_notes
        ? null
        : "Needs a durable transcript on a completed session.",
      run: () =>
        scrybe.regenerateNotes(session.id).then((result) =>
          result.outcome === "replaced"
            ? `Notes replaced — ${result.bytes.toString()} bytes.`
            : "The generator produced the notes already on disk.",
        ),
    },
    {
      key: "repair",
      label: "Repair recording",
      blocked: session.actions.repair
        ? null
        : "Nothing durable is left for a repair to recover.",
      run: () => scrybe.repairSession(session.id).then(repairMessage),
    },
    {
      key: "reveal",
      label: "Reveal in Finder",
      blocked: null,
      run: () => scrybe.revealSession(session.id).then(() => "Opened the session folder."),
    },
    {
      key: "copy-notes",
      label: "Copy notes",
      blocked: session.artifacts.notes ? null : "This session has no notes.",
      run: () => scrybe.copyNotes(session.id).then(() => "Notes copied."),
    },
    {
      key: "copy-transcript",
      label: "Copy transcript",
      blocked: session.artifacts.transcript ? null : "This session has no transcript.",
      run: () => scrybe.copyTranscript(session.id).then(() => "Transcript copied."),
    },
  ];

  return (
    <>
      <ul className="session-actions">
        {actions.map((action) => (
          <li key={action.key}>
            <button
              type="button"
              disabled={
                action.blocked !== null ||
                running !== null ||
                retention.running !== null ||
                retention.pending !== null
              }
              aria-describedby={action.blocked === null ? undefined : `${reason}-${action.key}`}
              onClick={() => {
                run(action);
              }}
            >
              {running === action.key ? `${action.label}…` : action.label}
            </button>
            {action.blocked === null ? null : (
              <span id={`${reason}-${action.key}`} className="session-actions__why">
                {action.blocked}
              </span>
            )}
          </li>
        ))}
      </ul>
      {retentionAvailable ? (
        <>
          <ul className="session-actions">
            <li>
              <button
                type="button"
                disabled={retention.running !== null || retention.pending !== null}
                onClick={() => {
                  retention.ask(session.id, "archive");
                }}
              >
                Archive
              </button>
            </li>
            <li>
              <button
                type="button"
                disabled={
                  retentionDays === null ||
                  retention.running !== null ||
                  retention.pending !== null
                }
                aria-describedby={retentionDays === null ? `${reason}-delete` : undefined}
                onClick={() => {
                  retention.ask(session.id, "delete");
                }}
              >
                Delete
              </button>
              {retentionDays === null ? (
                <span id={`${reason}-delete`} className="session-actions__why">
                  Reading how long the trash is kept.
                </span>
              ) : null}
            </li>
          </ul>
          {retention.pending === null ? null : (
            <ConfirmRetention
              what={retention.pending.what}
              // Only a delete names the window, and the delete control is
              // disabled until it is known, so a pending delete always has
              // one. The archive is kept indefinitely and names no number.
              retentionDays={retentionDays ?? 0}
              onProceed={() => {
                if (retention.pending !== null) {
                  retention.proceed(
                    retention.pending.id,
                    retention.pending.what,
                    retentionDays ?? 0,
                  );
                }
              }}
              onCancel={retention.cancel}
            />
          )}
        </>
      ) : null}
    </>
  );
}

function repairMessage(result: { outcome: string; recovered_secs: number | null }): string {
  if (result.outcome === "nothing_to_repair") {
    return "Nothing needed recovering.";
  }
  if (result.outcome === "metadata_reconstructed") {
    return "Metadata reconstructed from the recorded audio.";
  }
  const seconds = result.recovered_secs;
  return seconds === null
    ? "Recording recovered."
    : `Recording recovered — ${Math.round(seconds / 60).toString()} minutes.`;
}
