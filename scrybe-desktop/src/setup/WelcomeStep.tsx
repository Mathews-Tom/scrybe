import type { SettingsForm } from "../generated/bindings";

/**
 * What this application is, before it asks for anything.
 *
 * The four claims here are the ones a user has to be able to check
 * rather than take on trust, so each names the thing that makes it
 * checkable: the storage root is a path they can open, the artifacts
 * are ordinary files, and the one network request setup makes is named
 * before the step that would make it.
 */
export function WelcomeStep({ settings }: { settings: SettingsForm }) {
  return (
    <>
      <p>
        Scrybe records meetings on this Mac and keeps everything it produces on this Mac. No
        meeting bot joins a call, and nothing is uploaded unless you configure a hosted provider
        yourself.
      </p>
      <h3>Where recordings are kept</h3>
      <p>
        Every session becomes an ordinary folder under{" "}
        <span className="setup__path">{settings.storage_root}</span>, holding an audio file, a
        transcript, notes, and a small metadata file. You can open, copy, back up, or delete any
        of them with the Finder; nothing is in a database and nothing is in a format only this
        application reads.
      </p>
      <h3>The one network request</h3>
      <p>
        Setup can download a transcription model so speech is turned into text on this machine.
        That download is the only network request guided setup makes, you are shown exactly what
        it would fetch before it is requested, and it does not begin until you confirm it.
      </p>
      <h3>You can leave at any time</h3>
      <p>
        Leaving setup keeps the application usable — you can read and search anything already
        recorded. Recording stays unavailable until the requirements on the last step are met,
        and you can come back to setup whenever you like.
      </p>
    </>
  );
}
