import { useEffect, useId, useRef, useState } from "react";

import type { RetentionOutcome } from "../../generated/bindings";
import { useScrybe } from "../../ipc/ScrybeProvider";
import { describe } from "../useQuery";

/** What a retention action did, in the words the reader is shown. */
export interface RetentionResult {
  readonly kind: "note" | "problem";
  readonly message: string;
}

/** Which of the two moves is being offered. */
export type Retention = "delete" | "archive";

/**
 * The confirmation and move shared by both retention actions.
 *
 * Shared rather than written twice: delete and archive must each state
 * where the recording goes, and a second implementation of this flow
 * is a second chance for one consequence to be wrong.
 */
export function useRetention(onSettled: (result: RetentionResult) => void) {
  const scrybe = useScrybe();
  const [pending, setPending] = useState<{ id: string; what: Retention } | null>(null);
  const [running, setRunning] = useState<string | null>(null);

  function proceed(id: string, what: Retention, retentionDays: number) {
    setPending(null);
    setRunning(`${what}:${id}`);
    const move =
      what === "delete" ?
        scrybe.deleteSession(id, retentionDays).then(deleted)
      : scrybe.archiveSession(id).then(archived);
    move.then(
      (message) => {
        setRunning(null);
        onSettled({ kind: "note", message });
      },
      (error: unknown) => {
        setRunning(null);
        onSettled({ kind: "problem", message: describe(error) });
      },
    );
  }

  return {
    /** The action in flight, as `<what>:<id>`, or `null`. */
    running,
    pending,
    ask: (id: string, what: Retention) => {
      setPending({ id, what });
    },
    cancel: () => {
      setPending(null);
    },
    proceed,
  };
}

function deleted(outcome: RetentionOutcome): string {
  const days = outcome.retention_days;
  return `Deleted. It is in the trash for ${days.toString()} day${days === 1 ? "" : "s"}.`;
}

function archived(): string {
  return "Archived. It is in the archive folder inside your storage root.";
}

/**
 * The sentence a reader reads before an irreversible move, and the two
 * buttons that answer it.
 *
 * Part of the page rather than a system dialog: a `window.confirm`
 * cannot be given the sentence that says where the data goes, cannot be
 * reached by a screen reader in the order the rest of the surface is
 * read, and cannot be exercised by a test at all.
 *
 * `alertdialog` with an assertive label is what makes a reader who is
 * not looking at the control hear the consequence before the buttons.
 * The label is the statement itself, so there is nothing to read past.
 */
export function ConfirmRetention({
  what,
  retentionDays,
  onProceed,
  onCancel,
}: {
  what: Retention;
  /** The configured window. Only a delete names it. */
  retentionDays: number;
  onProceed: () => void;
  onCancel: () => void;
}) {
  const proceed = useRef<HTMLButtonElement>(null);
  const previousFocus = useRef<HTMLElement | null>(null);
  useEffect(() => {
    previousFocus.current =
      document.activeElement instanceof HTMLElement ? document.activeElement : null;
    proceed.current?.focus();
    return () => {
      if (previousFocus.current?.isConnected === true) {
        previousFocus.current.focus();
      }
    };
  }, []);
  const label = useId();
  return (
    <div
      role="alertdialog"
      aria-modal="false"
      aria-labelledby={label}
      className="session-actions__confirm"
    >
      <p id={label} className="session-actions__consequence">
        {what === "delete" ? deleteConsequence(retentionDays) : ARCHIVE_CONSEQUENCE}
      </p>
      <button
        ref={proceed}
        type="button"
        className="session-actions__proceed"
        onClick={onProceed}
      >
        {what === "delete" ? "Delete this session" : "Archive this session"}
      </button>
      <button type="button" onClick={onCancel}>
        Keep it
      </button>
    </div>
  );
}

function deleteConsequence(days: number): string {
  return (
    `This session moves to the trash folder inside your storage root ` +
    `and is removed permanently after ${days.toString()} ` +
    `day${days === 1 ? "" : "s"}. It stops appearing in Sessions and in ` +
    `Search. Nothing in the application restores it.`
  );
}

const ARCHIVE_CONSEQUENCE =
  "This session moves to the archive folder inside your storage root. " +
  "It stops appearing in Sessions and in Search, and it is kept " +
  "indefinitely — nothing removes it.";
