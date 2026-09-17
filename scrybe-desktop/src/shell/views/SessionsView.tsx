import { useState } from "react";

import { useScrybe } from "../../ipc/ScrybeProvider";
import { useStorageRootRevision } from "../library";
import { SessionDetail } from "../session/SessionDetail";
import { SessionList } from "../SessionList";
import { useQuery } from "../useQuery";

/** How many rows one page of the shell's list holds. */
const PAGE = 20;

export function SessionsView() {
  const scrybe = useScrybe();
  // Part of the read's identity, so returning to the window re-reads the
  // root through the same path the first render took.
  const revision = useStorageRootRevision();
  const [opened, setOpened] = useState<string | null>(null);
  const sessions = useQuery(
    () => scrybe.listSessions(0, PAGE),
    `sessions-${revision.toString()}`,
  );

  // Selection lives here rather than in the route registry: a route is
  // a destination in the sidebar, and one session is not a destination
  // the sidebar offers.
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
      <h1 id="view-heading">Sessions</h1>
      <SessionList
        query={sessions}
        empty="Nothing has been recorded into this storage root yet."
        onOpen={setOpened}
      />
    </>
  );
}
