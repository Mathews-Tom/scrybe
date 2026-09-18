import { useId } from "react";
import type { ReactNode } from "react";

import type { SessionProgress, SessionRow, SessionRows } from "../generated/bindings";
import type { Query } from "./useQuery";

const PROGRESS_LABEL: Record<SessionProgress, string> = {
  complete: "Complete",
  unfinished: "Unfinished",
  repairable: "Needs repair",
  failed: "Failed",
};

/** The heading a session with no readable start time is filed under. */
const UNDATED = "Undated";

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

/** One day's worth of rows, in the order the service layer returned them. */
interface DateGroup {
  readonly label: string;
  readonly rows: SessionRow[];
}

/**
 * Splits rows into one group per calendar day, in the viewer's own time
 * zone.
 *
 * Grouped by first appearance rather than by sorting: the service layer
 * already returns rows most recent first, and re-sorting here would
 * make this view's order a second opinion about what "most recent"
 * means. A row whose start time is missing or unreadable is filed under
 * one heading of its own instead of being dropped, because a session
 * that never wrote metadata is exactly the kind the reader is looking
 * for.
 */
function group(rows: SessionRow[]): DateGroup[] {
  const groups = new Map<string, SessionRow[]>();
  for (const row of rows) {
    const at = row.started_at === null ? null : new Date(row.started_at);
    const label =
      at === null || Number.isNaN(at.getTime())
        ? UNDATED
        : at.toLocaleDateString(undefined, { dateStyle: "full" });
    const existing = groups.get(label);
    if (existing === undefined) {
      groups.set(label, [row]);
    } else {
      existing.push(row);
    }
  }
  return [...groups].map(([label, grouped]) => ({ label, rows: grouped }));
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
 * after it. Each day's list carries the reference, because each of them
 * is a window onto the one truncated result.
 *
 * A row is the control that opens the session, rather than a region
 * with a control in it: the whole row is what the reader is aiming at,
 * and a separate `Open` button beside each one would put a second stop
 * in the keyboard order for every session in the list.
 *
 * A row says what state its session is in twice over: in the label it
 * carries, and in the `data-progress` the stylesheet distinguishes an
 * unfinished and a repair-needed session by. Colour alone would leave
 * the distinction invisible to a reader who cannot see it, and text
 * alone would leave it invisible to one scanning the list.
 */
export function SessionList({
  query,
  empty,
  emptyAction,
  pager,
  onOpen,
}: {
  query: Query<SessionRows>;
  empty: string;
  /**
   * Offered beside the empty text so the state that describes having
   * nothing also provides the way out of it. Optional, because a
   * filtered list's emptiness is not something an action can fix.
   */
  emptyAction?: ReactNode;
  /**
   * Moves the window this list is showing. Optional: a caller that
   * reads one page and never offers another should not render controls
   * that go nowhere.
   */
  pager?: { onPage: (offset: number) => void };
  onOpen: (id: string) => void;
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
    return (
      <div className="session-list__empty">
        <p>{empty}</p>
        {emptyAction}
      </div>
    );
  }
  const { offset } = query.value;
  const truncated = total > rows.length;
  const page = pager ?? null;

  return (
    <>
      {group(rows).map((day) => (
        <section key={day.label} className="session-list__day">
          <h2 className="session-list__date">{day.label}</h2>
          <ul
            className="session-list"
            aria-label={day.label}
            aria-describedby={truncated ? truncation : undefined}
          >
            {day.rows.map((row) => (
              <li key={row.id} className="session-list__row" data-progress={row.progress}>
                <button
                  type="button"
                  className="session-list__open"
                  onClick={() => {
                    onOpen(row.id);
                  }}
                >
                  <span className="session-list__title">{row.title ?? row.id}</span>
                  <span className="session-list__meta">
                    <span className="session-list__state">{PROGRESS_LABEL[row.progress]}</span> ·{" "}
                    {started(row.started_at)} · {duration(row.duration_secs)}
                  </span>
                </button>
              </li>
            ))}
          </ul>
        </section>
      ))}
      {truncated ? (
        <p id={truncation} className="session-list__truncation">
          {`Showing ${(offset + 1).toString()}–${(offset + rows.length).toString()} of ${total.toString()}.`}
        </p>
      ) : null}
      {page === null ? null : (
        <nav className="session-list__pages" aria-label="Pages of sessions">
          <button
            type="button"
            disabled={offset === 0}
            onClick={() => {
              page.onPage(Math.max(0, offset - rows.length));
            }}
          >
            Previous
          </button>
          <button
            type="button"
            disabled={!query.value.has_more}
            onClick={() => {
              page.onPage(offset + rows.length);
            }}
          >
            Next
          </button>
        </nav>
      )}
    </>
  );
}
