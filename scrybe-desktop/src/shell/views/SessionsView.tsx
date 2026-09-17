import { useScrybe } from "../../ipc/ScrybeProvider";
import { useStorageRootRevision } from "../library";
import { SessionList } from "../SessionList";
import { useQuery } from "../useQuery";

/** How many rows one page of the shell's list holds. */
const PAGE = 20;

export function SessionsView() {
  const scrybe = useScrybe();
  // Part of the read's identity, so returning to the window re-reads the
  // root through the same path the first render took.
  const revision = useStorageRootRevision();
  const sessions = useQuery(
    () => scrybe.listSessions(0, PAGE),
    `sessions-${revision.toString()}`,
  );

  return (
    <>
      <h1 id="view-heading">Sessions</h1>
      <SessionList
        query={sessions}
        empty="Nothing has been recorded into this storage root yet."
      />
    </>
  );
}
