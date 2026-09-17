import { createContext, use, type ReactNode } from "react";

import type {
  DiagnosticRows,
  ModelOffer,
  ModelOutcome,
  ModelProgress,
  ReadinessReport,
  RecordingStatus,
  RecoveryActionView,
  RepairOutcome,
  SessionRows,
  SettingsChange,
  SettingsForm,
  SettingsSummary,
} from "../generated/bindings";
import {
  cancelQuery,
  listSessions,
  recordingStatus,
  searchSessions,
  settingsSummary,
} from "./client";
import {
  applyRecovery,
  applySettings,
  cancelModelInstall,
  diagnosticsReport,
  installModel,
  modelOffer,
  onModelProgress,
  openAdvancedConfiguration,
  openSystemSettings,
  readinessReport,
  settingsForm,
} from "./setup";

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
  /**
   * `requestId` names the search, so `cancelQuery` can abandon it. A
   * view mints one per query and cancels the previous identifier before
   * firing the next.
   */
  searchSessions: (
    requestId: string,
    query: string,
    offset: number,
    limit: number,
  ) => Promise<SessionRows>;
  /** Whether a query was running under `requestId`. */
  cancelQuery: (requestId: string) => Promise<boolean>;
  settingsSummary: () => Promise<SettingsSummary>;
  recordingStatus: () => Promise<RecordingStatus>;
  settingsForm: () => Promise<SettingsForm>;
  applySettings: (changes: SettingsChange[]) => Promise<SettingsForm>;
  /** Reads. A view may call this on mount; it mutates nothing. */
  diagnosticsReport: () => Promise<DiagnosticRows>;
  /** Mutates, and only the one action it is handed. */
  applyRecovery: (action: RecoveryActionView) => Promise<RepairOutcome>;
  readinessReport: () => Promise<ReadinessReport>;
  /** Reads the catalog and the filesystem. Requests nothing. */
  modelOffer: (id: string) => Promise<ModelOffer>;
  /** Requires the digest the offer showed. */
  installModel: (id: string, acknowledgedSha256: string) => Promise<ModelOutcome>;
  cancelModelInstall: () => Promise<boolean>;
  /**
   * Subscribes to download progress until the returned function is
   * called. Here rather than imported directly by the panel that uses
   * it, for the reason every other service is here: the event bridge
   * exists only inside the host's WebView, so a component wired
   * straight to it could only be exercised by launching the
   * application.
   */
  onModelProgress: (onProgress: (progress: ModelProgress) => void) => Promise<() => void>;
  openSystemSettings: (capability: string) => Promise<void>;
  openAdvancedConfiguration: () => Promise<void>;
}

const REAL: Scrybe = {
  listSessions,
  searchSessions,
  cancelQuery,
  settingsSummary,
  recordingStatus,
  settingsForm,
  applySettings,
  diagnosticsReport,
  applyRecovery,
  readinessReport,
  modelOffer,
  installModel,
  cancelModelInstall,
  onModelProgress,
  openSystemSettings,
  openAdvancedConfiguration,
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
