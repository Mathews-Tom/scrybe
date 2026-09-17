import { useRef, useState, type SubmitEvent } from "react";

import type { SessionRows } from "../../generated/bindings";
import { useScrybe } from "../../ipc/ScrybeProvider";
import { nextRequestId, useStorageRootRevision } from "../library";
import { SessionList } from "../SessionList";
import { useQuery } from "../useQuery";

const PAGE = 20;

const NOTHING_ASKED: SessionRows = { rows: [], offset: 0, total: 0, has_more: false };

export function SearchView() {
  const scrybe = useScrybe();
  const revision = useStorageRootRevision();
  const [query, setQuery] = useState("");
  const [submitted, setSubmitted] = useState("");
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
      return scrybe.searchSessions(requestId, submitted, 0, PAGE);
    },
    // What identifies the request: the query asked, and a storage root
    // that may have changed under the last answer. The revision is
    // digits, so the first colon separates the two unambiguously
    // however the query is spelled.
    `${revision.toString()}:${submitted}`,
  );

  function search(event: SubmitEvent<HTMLFormElement>) {
    event.preventDefault();
    setSubmitted(query);
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
        <SessionList query={results} empty={`Nothing matches “${submitted}”.`} />
      )}
    </>
  );
}
