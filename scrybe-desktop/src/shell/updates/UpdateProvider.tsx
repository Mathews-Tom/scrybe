import { relaunch } from "@tauri-apps/plugin-process";
import { check, type Update } from "@tauri-apps/plugin-updater";
import {
  createContext,
  type ReactNode,
  use,
  useCallback,
  useMemo,
  useState,
} from "react";

import { useScrybe } from "../../ipc/ScrybeProvider";

const RECORDING_STATES: Record<string, true> = {
  preparing: true,
  recording: true,
  saving: true,
};

export type UpdateAvailability = "unknown" | "current" | "available";

type UpdateOperation = "idle" | "checking" | "downloading" | "downloaded" | "installing";

interface UpdateContextValue {
  readonly availability: UpdateAvailability;
  readonly version: string | null;
  readonly operation: UpdateOperation;
  readonly message: string;
  readonly busy: boolean;
  readonly checkForUpdate: () => Promise<void>;
  readonly downloadUpdate: () => Promise<void>;
  readonly installUpdate: () => Promise<void>;
}

const UpdateContext = createContext<UpdateContextValue | null>(null);

function failureMessage(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}


/**
 * Owns the explicit, authenticated application-update lifecycle for one
 * running application. It performs no updater request until the reader asks.
 */
export function UpdateProvider({ children }: { readonly children: ReactNode }) {
  const scrybe = useScrybe();
  const [update, setUpdate] = useState<Update | null>(null);
  const [availability, setAvailability] = useState<UpdateAvailability>("unknown");
  const [operation, setOperation] = useState<UpdateOperation>("idle");
  const [message, setMessage] = useState(
    "Checks contact the Scrybe GitHub release feed only when you ask.",
  );

  const checkForUpdate = useCallback(async (): Promise<void> => {
    setOperation("checking");
    setUpdate(null);
    setAvailability("unknown");
    setMessage("Checking for an authenticated update…");
    try {
      const available = await check({ timeout: 15_000 });
      setUpdate(available);
      setAvailability(available === null ? "current" : "available");
      setMessage(
        available === null
          ? "This installation is current."
          : `Scrybe ${available.version} is available.`,
      );
    } catch (error) {
      setAvailability("unknown");
      setMessage(`The update check failed: ${failureMessage(error)}`);
    } finally {
      setOperation("idle");
    }
  }, []);

  const downloadUpdate = useCallback(async (): Promise<void> => {
    if (update === null) {
      return;
    }
    setOperation("downloading");
    setMessage(`Downloading Scrybe ${update.version}…`);
    let received = 0;
    try {
      await update.download((event) => {
        if (event.event === "Progress") {
          received += event.data.chunkLength;
          setMessage(`Downloading Scrybe ${update.version}… ${received.toLocaleString()} bytes`);
        }
      });
      setMessage(
        `Scrybe ${update.version} is authenticated and ready to install. ` +
          "Installation relaunches the app.",
      );
      setOperation("downloaded");
    } catch (error) {
      setMessage(`The authenticated update was not downloaded: ${failureMessage(error)}`);
      setOperation("idle");
    }
  }, [update]);

  const installUpdate = useCallback(async (): Promise<void> => {
    if (update === null || operation !== "downloaded") {
      return;
    }
    setOperation("installing");
    try {
      const recording = await scrybe.recordingStatus();
      if (RECORDING_STATES[recording.state]) {
        setMessage("Stop the active recording before installing and relaunching Scrybe.");
        setOperation("downloaded");
        return;
      }
      setMessage(`Installing Scrybe ${update.version}…`);
      await update.install();
      await relaunch();
    } catch (error) {
      setMessage(`The update was not installed: ${failureMessage(error)}`);
      setOperation("downloaded");
    }
  }, [operation, scrybe, update]);

  const value = useMemo<UpdateContextValue>(
    () => ({
      availability,
      version: update?.version ?? null,
      operation,
      message,
      busy: operation === "checking" || operation === "downloading" || operation === "installing",
      checkForUpdate,
      downloadUpdate,
      installUpdate,
    }),
    [availability, checkForUpdate, downloadUpdate, installUpdate, message, operation, update],
  );

  return <UpdateContext value={value}>{children}</UpdateContext>;
}

export function useUpdate(): UpdateContextValue {
  const update = use(UpdateContext);
  if (update === null) {
    throw new Error("update state is unavailable outside the application shell");
  }
  return update;
}
