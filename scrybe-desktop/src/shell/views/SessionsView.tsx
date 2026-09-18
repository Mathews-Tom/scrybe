import { useId, useState } from "react";

import { useScrybe } from "../../ipc/ScrybeProvider";
import { useStorageRootRevision } from "../library";
import { useRecording } from "../recording/useRecording";
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
  // The window of the list being shown. Part of the read's identity, so
  // moving the window re-reads rather than re-rendering stale rows.
  const [offset, setOffset] = useState(0);
  const sessions = useQuery(
    () => scrybe.listSessions(offset, PAGE),
    `sessions-${revision.toString()}-${offset.toString()}`,
  );

  // Recording is reachable from here as well as from its own
  // destination, because this is the view a reader arrives in and the
  // approved design puts the action where they already are. Both routes
  // drive the one watcher mounted above them, so there is no second
  // clock and no second way to start a recording — only a second place
  // to ask for one.
  const recording = useRecording();
  const titleId = useId();
  const [title, setTitle] = useState("");
  // Keyed on the revision so a reader whose model went missing since
  // the last read is told on this attempt rather than the next launch.
  const preflight = useQuery(
    () => scrybe.recordingPreflight(),
    `sessions-preflight-${revision.toString()}`,
  );
  const canRecord = preflight.status === "ready" ? preflight.value.can_record : false;
  // Disabled rather than hidden: an action that disappears leaves a
  // reader wondering where it went, and the reason it is unavailable is
  // what the Setup destination exists to explain.
  const recordDisabled = !recording.startEnabled || !canRecord;

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
      <div className="view-toolbar">
        <h1 id="view-heading">Sessions</h1>
        <div className="view-toolbar__actions">
          <label className="view-toolbar__title-label" htmlFor={titleId}>
            Title
          </label>
          <input
            id={titleId}
            type="text"
            value={title}
            placeholder="untitled"
            disabled={!recording.startEnabled}
            onChange={(event) => {
              setTitle(event.target.value);
            }}
          />
          <button
            className="view-toolbar__record"
            type="button"
            disabled={recordDisabled}
            onClick={() => {
              recording.start(title);
            }}
          >
            Record now
          </button>
        </div>
      </div>
      <SessionList
        query={sessions}
        empty="Nothing has been recorded into this storage root yet."
        emptyAction={
          <button
            type="button"
            disabled={recordDisabled}
            onClick={() => {
              recording.start(title);
            }}
          >
            Start the first recording
          </button>
        }
        pager={{ onPage: setOffset }}
        onOpen={setOpened}
      />
    </>
  );
}
