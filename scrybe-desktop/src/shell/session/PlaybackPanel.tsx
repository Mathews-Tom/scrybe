import { useState } from "react";

/**
 * The scheme's host.
 *
 * A custom scheme still needs one, and the session is named in the path
 * rather than there: a host is not case-preserving in every URL parser
 * and a session folder name carries an upper-case ULID. The identity
 * itself is not encoded, because a session folder name is built from a
 * timestamp, a slug of ASCII letters, digits and hyphens, and a
 * Crockford base32 identifier — nothing a URL escapes. A name from
 * somewhere else that did need escaping would arrive escaped, name no
 * session, and be refused rather than reach a file.
 */
const ORIGIN = "scrybe-audio://localhost";

/** The `MediaError` codes, as numbers. See [`why`]. */
const ABORTED = 1;
const NETWORK = 2;
const DECODE = 3;
const UNSUPPORTED = 4;

/**
 * What the media element knows about why it could not play.
 *
 * `MediaError` carries a code and nothing a reader can act on, and the
 * refusal the scheme returned is not readable from here — the policy
 * that lets an `<audio>` element load this URL does not let script
 * fetch it. So each code is turned into the one sentence it actually
 * supports, and none of them speculates beyond it.
 */
function why(error: MediaError | null): string {
  // The numbers rather than `MediaError.MEDIA_ERR_*`. The interface
  // object is a browser global, and reading a property off it in an
  // environment that does not define it throws inside the handler,
  // which turns "say why playback failed" into "say nothing at all".
  switch (error?.code) {
    case ABORTED:
      return "Playback was stopped before it started.";
    case NETWORK:
      return "This session's audio could not be read from disk.";
    case DECODE:
      return "This session's audio is on disk but could not be decoded.";
    case UNSUPPORTED:
      return "This session has no audio to play.";
    default:
      return "This session's audio could not be played.";
  }
}

/**
 * The session's playback audio.
 *
 * Rendered only for a session whose playback artifact exists, so the
 * player is never the thing that tells a reader their audio is missing.
 *
 * The platform's own controls, rather than a set built here: play,
 * pause, seek, and elapsed time all come with them, already reachable
 * from the keyboard and already in the reader's language. What is added
 * is the one thing they do not carry — what happened when the source
 * could not be loaded.
 */
export function PlaybackPanel({ id }: { id: string }) {
  const [failure, setFailure] = useState<string | null>(null);

  return (
    <section className="playback" aria-label="Playback">
      {/* eslint-disable-next-line jsx-a11y/media-has-caption -- there is
          no caption track to offer: nothing in this product produces
          one, and the transcript on this same screen is the text
          alternative a caption track would be. */}
      <audio
        className="playback__player"
        controls
        preload="metadata"
        src={`${ORIGIN}/${id}/playback`}
        onError={(event) => {
          setFailure(why(event.currentTarget.error));
        }}
        onPlay={() => {
          setFailure(null);
        }}
      />
      {failure === null ? null : (
        <p role="alert" className="session-detail__error">
          {failure}
        </p>
      )}
    </section>
  );
}
