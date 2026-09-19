import { useUpdate } from "../updates/UpdateProvider";

/**
 * User-initiated controls for project-authenticated application updates.
 *
 * The updater plugin verifies the archive's Minisign signature against the
 * public key compiled into the host. Installation and relaunch stay separate
 * from download so an active recording can never be replaced or interrupted.
 */
export function UpdatePanel() {
  const {
    busy,
    checkForUpdate,
    downloadUpdate,
    installUpdate,
    message,
    operation,
    version,
  } = useUpdate();

  return (
    <section aria-labelledby="settings-update-heading">
      <h2 id="settings-update-heading">Application updates</h2>
      <p aria-live="polite">{message}</p>
      <div className="setup__actions">
        <button type="button" disabled={busy} onClick={() => void checkForUpdate()}>
          Check for updates
        </button>
        {version !== null && operation === "idle" && (
          <button type="button" disabled={busy} onClick={() => void downloadUpdate()}>
            Download update
          </button>
        )}
        {operation === "downloaded" && (
          <button type="button" disabled={busy} onClick={() => void installUpdate()}>
            Install and relaunch
          </button>
        )}
      </div>
    </section>
  );
}
