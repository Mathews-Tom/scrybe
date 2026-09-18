import { relaunch } from "@tauri-apps/plugin-process";
import { check, type DownloadEvent, type Update } from "@tauri-apps/plugin-updater";
import { useState } from "react";

import { useScrybe } from "../../ipc/ScrybeProvider";

const RECORDING_STATES: Record<string, true> = {
  preparing: true,
  recording: true,
  saving: true,
};

function failureMessage(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

function downloadedBytes(event: DownloadEvent, previous: number): number {
  return event.event === "Progress" ? previous + event.data.chunkLength : previous;
}

/**
 * User-initiated checks for project-authenticated application updates.
 *
 * The updater plugin verifies the archive's minisign signature against
 * the public key compiled into the host. Installation and relaunch stay
 * separate from download so an active recording can never be replaced
 * or interrupted by an update.
 */
export function UpdatePanel() {
  const scrybe = useScrybe();
  const [available, setAvailable] = useState<Update | null>(null);
  const [downloaded, setDownloaded] = useState(false);
  const [busy, setBusy] = useState(false);
  const [message, setMessage] = useState(
    "Checks contact the Scrybe GitHub release feed only when you ask.",
  );

  async function checkForUpdate(): Promise<void> {
    setBusy(true);
    setDownloaded(false);
    setMessage("Checking for an authenticated update…");
    try {
      const update = await check({ timeout: 15_000 });
      setAvailable(update);
      setMessage(
        update === null
          ? "This installation is current."
          : `Scrybe ${update.version} is available.`,
      );
    } catch (error) {
      setAvailable(null);
      setMessage(`The update check failed: ${failureMessage(error)}`);
    } finally {
      setBusy(false);
    }
  }

  async function downloadUpdate(): Promise<void> {
    if (available === null) {
      return;
    }
    setBusy(true);
    setMessage(`Downloading Scrybe ${available.version}…`);
    let received = 0;
    try {
      await available.download((event) => {
        received = downloadedBytes(event, received);
        if (event.event === "Progress") {
          setMessage(`Downloading Scrybe ${available.version}… ${received.toLocaleString()} bytes`);
        }
      });
      setDownloaded(true);
      setMessage(
        `Scrybe ${available.version} is authenticated and ready to install. ` +
          "Installation relaunches the app.",
      );
    } catch (error) {
      setMessage(`The authenticated update was not downloaded: ${failureMessage(error)}`);
    } finally {
      setBusy(false);
    }
  }

  async function installUpdate(): Promise<void> {
    if (available === null || !downloaded) {
      return;
    }
    setBusy(true);
    try {
      const recording = await scrybe.recordingStatus();
      if (RECORDING_STATES[recording.state]) {
        setMessage("Stop the active recording before installing and relaunching Scrybe.");
        return;
      }
      setMessage(`Installing Scrybe ${available.version}…`);
      await available.install();
      await relaunch();
    } catch (error) {
      setMessage(`The update was not installed: ${failureMessage(error)}`);
    } finally {
      setBusy(false);
    }
  }

  return (
    <section aria-labelledby="settings-update-heading">
      <h2 id="settings-update-heading">Application updates</h2>
      <p aria-live="polite">{message}</p>
      <div className="setup__actions">
        <button type="button" disabled={busy} onClick={() => void checkForUpdate()}>
          Check for updates
        </button>
        {available !== null && !downloaded && (
          <button type="button" disabled={busy} onClick={() => void downloadUpdate()}>
            Download update
          </button>
        )}
        {available !== null && downloaded && (
          <button type="button" disabled={busy} onClick={() => void installUpdate()}>
            Install and relaunch
          </button>
        )}
      </div>
    </section>
  );
}
