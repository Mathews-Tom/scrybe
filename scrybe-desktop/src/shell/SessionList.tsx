import { useId } from "react";

import type { SessionProgress, SessionRow } from "../generated/bindings";
import type { Query } from "./useQuery";

const PROGRESS_LABEL: Record<SessionProgress, string> = {
  complete: "Complete",
  unfinished: "Unfinished",
  repairable: "Needs repair",
  failed: "Failed",
};

function duration(seconds: number | null): string {
  if (seconds === null) {
    return "Unknown length";
  }
  const minutes = Math.round(seconds / 60);
  return minutes === 1 ? "1 minute" : `${minutes.toString()} minutes`;
}

function started(at: string | null): string {
  if (at === null) {
    return "Unknown date";
  }
  const parsed = new Date(at);
  return Number.isNaN(parsed.getTime()) ? "Unknown date" : parsed.toLocaleString();
}

/**
 * Renders the outcome of a session read.
 *
 * A failure is shown as itself. Rendering an empty list when Rust
 * reported an unreadable storage root would tell the reader their
 * recordings are gone.
 *
 * A truncated read is shown as itself for the same reason. Both views
 * ask for one page and Rust answers with the total, so a storage root
 * holding twenty-one sessions used to render exactly like one holding
 * twenty: nothing on screen separated "these are your sessions" from
 * "these are the first twenty of them", and there is no paging control
 * to go looking for the rest with. The count is what makes the
 * difference visible, and it describes the list rather than sitting
 * beside it, so it is read out with the list rather than stranded
 * after it.
 */
export function SessionList({
  query,
  empty,
}: {
  query: Query<{ rows: SessionRow[]; total: number }>;
  empty: string;
}) {
  const truncation = useId();

  if (query.status === "loading") {
    return <p role="status">Loading sessions…</p>;
  }
  if (query.status === "failed") {
    return (
      <p role="alert" className="session-list__error">
        {query.message}
      </p>
    );
  }
  const { rows, total } = query.value;
  if (rows.length === 0) {
    return <p>{empty}</p>;
  }
  const truncated = total > rows.length;

  return (
    <>
      <ul className="session-list" aria-describedby={truncated ? truncation : undefined}>
        {rows.map((row) => (
          <li key={row.id} className="session-list__row">
            <span className="session-list__title">{row.title ?? row.id}</span>
            <span className="session-list__meta">
              {PROGRESS_LABEL[row.progress]} · {started(row.started_at)} ·{" "}
              {duration(row.duration_secs)}
            </span>
          </li>
        ))}
      </ul>
      {truncated ? (
        <p id={truncation} className="session-list__truncation">
          {`Showing the first ${rows.length.toString()} of ${total.toString()}.`}
        </p>
      ) : null}
    </>
  );
}
