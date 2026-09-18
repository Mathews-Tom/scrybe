import { useEffect, useRef, useState, type SubmitEvent } from "react";

import type { SessionRows } from "../../generated/bindings";
import { useScrybe } from "../../ipc/ScrybeProvider";
import { nextRequestId, useStorageRootRevision } from "../library";
import { SessionDetail } from "../session/SessionDetail";
import { SessionList } from "../SessionList";
import { useQuery } from "../useQuery";

const PAGE = 20;

/**
 * How long a reader stops typing before the search runs.
 *
 * Long enough that a word is not searched letter by letter, short
 * enough to feel like the list is following. Every keystroke past this
 * still supersedes whatever is in flight by name, so the cost of
 * guessing low is a cancelled query rather than a wrong answer.
 */
const SETTLE_MS = 200;

const NOTHING_ASKED: SessionRows = { rows: [], offset: 0, total: 0, has_more: false };

export function SearchView() {
  const scrybe = useScrybe();
  const revision = useStorageRootRevision();
  const [query, setQuery] = useState("");
  const [submitted, setSubmitted] = useState("");
  // What the query settled to. A reader who is still typing has not
  // asked anything yet; one who has paused has, and pressing the button
  // or Enter asks immediately rather than waiting out the delay.
  useEffect(() => {
    if (query === submitted) {
      return undefined;
    }
    const timer = setTimeout(() => {
      setSubmitted(query);
    }, SETTLE_MS);
    return () => {
      clearTimeout(timer);
    };
  }, [query, submitted]);
  const [opened, setOpened] = useState<string | null>(null);
  // Reset with the query: page three of a previous search is not a
  // window into this one.
  const [offset, setOffset] = useState(0);
  // The identifier the search still in flight carries, so the next
  // search can abandon it by name.
  const live = useRef<string | null>(null);
  const results = useQuery(
    () => {
      if (submitted === "") {
        return Promise.resolve(NOTHING_ASKED);
      }
      const abandoned = live.current;
      const requestId = nextRequestId();
      live.current = requestId;
      if (abandoned !== null) {
        // A cancel that never lands leaves an abandoned search running
        // to the end, and its answer is still discarded: the identifier
        // it carries is no longer the one this view is waiting for. So
        // a failure here is reported rather than propagated — it costs
        // work, not correctness — and it is never swallowed, because a
        // cancel command that had stopped working would otherwise look
        // exactly like one that was working.
        scrybe.cancelQuery(abandoned).catch((error: unknown) => {
          console.error("scrybe: a superseded search could not be cancelled", error);
        });
      }
      return scrybe.searchSessions(requestId, submitted, offset, PAGE);
    },
    // What identifies the request: the query asked, and a storage root
    // that may have changed under the last answer. The revision is
    // digits, so the first colon separates the two unambiguously
    // however the query is spelled.
    `${revision.toString()}:${offset.toString()}:${submitted}`,
  );

  function search(event: SubmitEvent<HTMLFormElement>) {
    event.preventDefault();
    setSubmitted(query);
  }

  // A result opens the same session surface the list does, and leaving
  // it returns to the results rather than to the top of the view.
  if (opened !== null) {
    return (
      <SessionDetail
        key={opened}
        id={opened}
        onBack={() => {
          setOpened(null);
        }}
      />
    );
  }

  return (
    <>
      <h1 id="view-heading">Search</h1>
      <form className="search" onSubmit={search}>
        <label className="search__label" htmlFor="search-query">
          Search transcripts and notes
        </label>
        <input
          id="search-query"
          className="search__input"
          type="search"
          value={query}
          onChange={(event) => {
            setQuery(event.target.value);
          }}
        />
        <button type="submit">Search</button>
      </form>
      {submitted === "" ? (
        <p>Search runs over the transcripts and notes already on this machine.</p>
      ) : (
        <SessionList
          query={results}
          empty={`Nothing matches “${submitted}”.`}
          pager={{ onPage: setOffset }}
          onOpen={setOpened}
        />
      )}
    </>
  );
}
