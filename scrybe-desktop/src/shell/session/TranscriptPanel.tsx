import { useState } from "react";

import type { TranscriptWindow } from "../../generated/bindings";
import { useScrybe } from "../../ipc/ScrybeProvider";
import { useQuery } from "../useQuery";

/**
 * Lines in one window.
 *
 * The largest transcript in the measured storage root is 95 lines, so
 * this is not a figure tuned against a corpus: it is a window big
 * enough that an ordinary transcript is one or two of them, and small
 * enough that a transcript an order of magnitude larger still costs the
 * same to show.
 */
const LINES = 50;

/**
 * A transcript, one window at a time.
 *
 * Only the window being read is fetched, and only its lines are in the
 * document. A view that rendered the whole transcript would hold a
 * payload it never shows and grow linearly with a file that has no
 * stated bound; this grows with the window instead.
 */
export function TranscriptPanel({ id, revision }: { id: string; revision: string }) {
  const scrybe = useScrybe();
  const [cursor, setCursor] = useState(0);
  const read = useQuery(
    () => scrybe.readTranscriptPage(id, cursor, LINES),
    `${revision}:${cursor.toString()}`,
  );

  if (read.status === "loading") {
    return <p role="status">Loading the transcript…</p>;
  }
  if (read.status === "failed") {
    return (
      <p role="alert" className="session-detail__error">
        {read.message}
      </p>
    );
  }
  const shown = read.value;
  if (shown.total_lines === 0) {
    return <p>This session has no transcript.</p>;
  }

  return (
    <>
      <ol className="transcript" start={shown.cursor + 1} aria-label="Transcript">
        {shown.lines.map((line, index) => (
          <li key={`${(shown.cursor + index).toString()}:${line}`} className="transcript__line">
            {line}
          </li>
        ))}
      </ol>
      <p className="transcript__position">{position(shown)}</p>
      <div className="transcript__paging">
        <button
          type="button"
          disabled={shown.cursor === 0}
          onClick={() => {
            setCursor(Math.max(shown.cursor - LINES, 0));
          }}
        >
          Earlier lines
        </button>
        <button
          type="button"
          disabled={shown.next === null}
          onClick={() => {
            setCursor(shown.next ?? shown.cursor);
          }}
        >
          Later lines
        </button>
      </div>
    </>
  );
}

function position(shown: TranscriptWindow): string {
  const first = shown.cursor + 1;
  const last = shown.cursor + shown.lines.length;
  return `Lines ${first.toString()}–${last.toString()} of ${shown.total_lines.toString()}.`;
}
