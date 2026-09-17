import { useState, type SubmitEvent } from "react";

import { useScrybe } from "../../ipc/ScrybeProvider";
import { SessionList } from "../SessionList";
import { useQuery } from "../useQuery";

const PAGE = 20;

export function SearchView() {
  const scrybe = useScrybe();
  const [query, setQuery] = useState("");
  const [submitted, setSubmitted] = useState("");
  const results = useQuery(
    () =>
      submitted === ""
        ? Promise.resolve({ rows: [], total: 0, offset: 0, has_more: false })
        : scrybe.searchSessions(submitted, 0, PAGE),
    submitted,
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
