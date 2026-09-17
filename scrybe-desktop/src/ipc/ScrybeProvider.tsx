import { createContext, use, type ReactNode } from "react";

import type {
  RecordingStatus,
  SessionRows,
  SettingsSummary,
} from "../generated/bindings";
import {
  listSessions,
  recordingStatus,
  searchSessions,
  settingsSummary,
} from "./client";

/**
 * Everything the views may ask Rust for.
 *
 * Views depend on this rather than on `client.ts` directly so a test
 * can substitute a stand-in. The IPC bridge only exists inside the
 * host's WebView, so a view wired straight to `invoke` could only be
 * exercised by launching the application.
 */
export interface Scrybe {
  listSessions: (offset: number, limit: number) => Promise<SessionRows>;
  searchSessions: (
    query: string,
    offset: number,
    limit: number,
  ) => Promise<SessionRows>;
  settingsSummary: () => Promise<SettingsSummary>;
  recordingStatus: () => Promise<RecordingStatus>;
}

const REAL: Scrybe = {
  listSessions,
  searchSessions,
  settingsSummary,
  recordingStatus,
};

const ScrybeContext = createContext<Scrybe>(REAL);

export function ScrybeProvider({
  scrybe,
  children,
}: {
  scrybe: Scrybe;
  children: ReactNode;
}) {
  return <ScrybeContext value={scrybe}>{children}</ScrybeContext>;
}

/** The services, real in the application and substituted in tests. */
export function useScrybe(): Scrybe {
  return use(ScrybeContext);
}
