import { useScrybe } from "../../ipc/ScrybeProvider";
import { SessionList } from "../SessionList";
import { useQuery } from "../useQuery";

/** How many rows one page of the shell's list holds. */
const PAGE = 20;

export function SessionsView() {
  const scrybe = useScrybe();
  const sessions = useQuery(() => scrybe.listSessions(0, PAGE), "sessions");

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
