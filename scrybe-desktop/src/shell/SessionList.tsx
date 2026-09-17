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
 */
export function SessionList({
  query,
  empty,
}: {
  query: Query<{ rows: SessionRow[]; total: number }>;
  empty: string;
}) {
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
  if (query.value.rows.length === 0) {
    return <p>{empty}</p>;
  }

  return (
    <ul className="session-list">
      {query.value.rows.map((row) => (
        <li key={row.id} className="session-list__row">
          <span className="session-list__title">{row.title ?? row.id}</span>
          <span className="session-list__meta">
            {PROGRESS_LABEL[row.progress]} · {started(row.started_at)} ·{" "}
            {duration(row.duration_secs)}
          </span>
        </li>
      ))}
    </ul>
  );
}
