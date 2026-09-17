import { useState } from "react";

import { useScrybe } from "../../ipc/ScrybeProvider";
import { useStorageRootRevision } from "../library";
import { useQuery } from "../useQuery";
import { PlaybackPanel } from "./PlaybackPanel";
import { SessionActions, type Outcome } from "./SessionActions";
import { SessionFacts } from "./SessionFacts";
import { TranscriptPanel } from "./TranscriptPanel";

/** Which document the reader is looking at. */
type Document = "notes" | "transcript";

/**
 * One session, read-only.
 *
 * Notes first, because a reader opening a finished meeting is looking
 * for what it concluded, not for what was said. The transcript is a
 * window away and is never read whole.
 *
 * Everything on screen is re-read when the storage root may have
 * changed, and again after an action that changed this session, so a
 * transcript edited in another editor or a recording repaired here
 * shows as it is rather than as it was when the view opened. What an
 * action did is held here rather than beside the buttons, because the
 * re-read rebuilds those and would take the sentence with them. It is
 * cleared the moment the storage root itself is why this session is
 * being re-read, though, rather than surviving indefinitely: a
 * sentence about this reader's own last action describes nothing once
 * the CLI, another window, or a file moved in Finder is what changed
 * this session, and a repair made that way must not leave an earlier
 * in-app failure sitting on screen forever.
 */
export function SessionDetail({ id, onBack }: { id: string; onBack: () => void }) {
  const scrybe = useScrybe();
  const storage = useStorageRootRevision();
  const [acted, setActed] = useState(0);
  const [outcome, setOutcome] = useState<Outcome | null>(null);
  const revision = `${storage.toString()}:${acted.toString()}:${id}`;
  const session = useQuery(() => scrybe.getSession(id), revision);
  const [shown, setShown] = useState<Document>("notes");

  // Adjusting state during rendering rather than in an effect: `storage`
  // changing is what a stale outcome must not survive, and comparing
  // against the value last cleared for keeps this synchronous with the
  // render it affects instead of leaving one stale frame behind it.
  const [clearedFor, setClearedFor] = useState(storage);
  if (storage !== clearedFor) {
    setClearedFor(storage);
    setOutcome(null);
  }

  const said =
    outcome === null ? null : (
      <p
        role={outcome.kind === "problem" ? "alert" : "status"}
        className={
          outcome.kind === "problem" ? "session-actions__problem" : "session-actions__note"
        }
      >
        {outcome.message}
      </p>
    );

  if (session.status !== "ready") {
    return (
      <>
        <button type="button" className="session-detail__back" onClick={onBack}>
          Back to sessions
        </button>
        <h1 id="view-heading">Session</h1>
        {said}
        {session.status === "failed" ? (
          <p role="alert" className="session-detail__error">
            {session.message}
          </p>
        ) : (
          <p role="status">Loading this session…</p>
        )}
      </>
    );
  }
  const detail = session.value;

  return (
    <>
      <button type="button" className="session-detail__back" onClick={onBack}>
        Back to sessions
      </button>
      <h1 id="view-heading">{detail.title ?? detail.id}</h1>
      <SessionFacts detail={detail} />
      <SessionActions
        session={detail}
        onFinished={(result) => {
          setOutcome(result);
          // The session's state, its artifacts, and therefore what it
          // will accept next are all decided on disk, so the view
          // re-reads rather than predicting what the action left behind.
          if (result.kind === "note") {
            setActed((count) => count + 1);
          }
        }}
      />
      {said}
      {/*
        Offered only for a session whose playback artifact is actually
        there. A player rendered for a session without one would be the
        misleading affordance this whole surface exists to avoid: the
        reader presses play and learns nothing about why nothing
        happened.
      */}
      {detail.artifacts.playback ? <PlaybackPanel id={detail.id} /> : null}
      {/*
        Two buttons rather than a tablist. A tablist owes the reader a
        roving tabindex and arrow-key movement, and buys nothing here:
        there are two documents, each reachable with the key every other
        control on this screen answers to.
      */}
      <div className="session-detail__documents">
        {(["notes", "transcript"] as const).map((document) => (
          <button
            key={document}
            type="button"
            aria-pressed={shown === document}
            onClick={() => {
              setShown(document);
            }}
          >
            {document === "notes" ? "Notes" : "Transcript"}
          </button>
        ))}
      </div>
      {shown === "notes" ? (
        <NotesPanel id={id} revision={revision} />
      ) : (
        <TranscriptPanel id={id} revision={revision} />
      )}
    </>
  );
}

/**
 * A session's notes, as they are on disk.
 *
 * Rendered as the markdown that was written rather than as formatted
 * output: this surface reads, and a renderer here would be a second
 * opinion about what the document says. Nothing on this screen can edit
 * it.
 */
function NotesPanel({ id, revision }: { id: string; revision: string }) {
  const scrybe = useScrybe();
  const notes = useQuery(() => scrybe.readNotes(id), revision);

  if (notes.status === "loading") {
    return <p role="status">Loading the notes…</p>;
  }
  if (notes.status === "failed") {
    return (
      <p role="alert" className="session-detail__error">
        {notes.message}
      </p>
    );
  }
  const { markdown } = notes.value;
  if (markdown === null) {
    return <p>This session has no notes.</p>;
  }
  return (
    <pre className="notes" aria-label="Notes">
      {markdown}
    </pre>
  );
}
