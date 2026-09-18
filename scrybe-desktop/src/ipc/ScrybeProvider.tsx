import { createContext, use, type ReactNode } from "react";

import type {
  DiagnosticRows,
  NotesRegeneration,
  ModelOffer,
  ModelOutcome,
  ModelProgress,
  PreflightView,
  RecordingProgressView,
  RecordingTransition,
  ReadinessReport,
  RecordingStatus,
  RecoveryActionView,
  RepairOutcome,
  RetentionOutcome,
  SessionDetail,
  SessionNotes,
  SessionRepair,
  SessionRows,
  SettingsChange,
  SettingsForm,
  SettingsSummary,
  TranscriptWindow,
} from "../generated/bindings";
import {
  acknowledgeRecording,
  cancelQuery,
  copyNotes,
  copyTranscript,
  getSession,
  listSessions,
  readNotes,
  onRecordingProgress,
  onRecordingTransition,
  readTranscriptPage,
  recordingPreflight,
  recordingStatus,
  regenerateNotes,
  startRecording,
  stopRecording,
  repairSession,
  revealSession,
  deleteSession,
  archiveSession,
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
  getSession: (id: string) => Promise<SessionDetail>;
  readNotes: (id: string) => Promise<SessionNotes>;
  /** One window of a transcript. The whole document is never read. */
  readTranscriptPage: (
    id: string,
    offset: number,
    limit: number,
  ) => Promise<TranscriptWindow>;
  /** Mutates, and only by completing a recording already made. */
  repairSession: (id: string) => Promise<SessionRepair>;
  /** Mutates, and only `notes.md`. */
  regenerateNotes: (id: string) => Promise<NotesRegeneration>;
  revealSession: (id: string) => Promise<void>;
  /** Moves a session to the trash under the days the reader confirmed. */
  deleteSession: (id: string, retentionDays: number) => Promise<RetentionOutcome>;
  /** Moves a session to the archive, which the sweep never touches. */
  archiveSession: (id: string) => Promise<RetentionOutcome>;
  copyNotes: (id: string) => Promise<void>;
  /** Read and copied in Rust, so the document never reaches this side. */
  copyTranscript: (id: string) => Promise<void>;
  settingsSummary: () => Promise<SettingsSummary>;
  recordingStatus: () => Promise<RecordingStatus>;
  /**
   * Whether this installation could record right now, check by check.
   *
   * Separate from `readinessReport`, which describes the install as a
   * setup wizard sees it. This one answers the narrower question a
   * recording control asks: would pressing record work, against the
   * configuration as it stands and the adapters this build carries.
   */
  recordingPreflight: () => Promise<PreflightView>;
  /**
   * Calls `onTransition` for every recording state change until the
   * returned function is called.
   *
   * On the interface rather than imported straight from `client.ts` so
   * a test can drive a transition without a `WebView` to emit one.
   */
  onRecordingTransition: (
    onTransition: (transition: RecordingTransition) => void,
  ) => Promise<() => void>;
  /** Calls `onProgress` each time saving moves a step. */
  onRecordingProgress: (
    onProgress: (progress: RecordingProgressView) => void,
  ) => Promise<() => void>;
  /** Starts a recording, returning as soon as it is under way. */
  startRecording: (title: string | null) => Promise<RecordingStatus>;
  /** Asks the recording in flight to stop and save. */
  stopRecording: () => Promise<RecordingStatus>;
  /**
   * Acknowledges a terminal recording once this surface has rendered
   * its outcome, returning the host to idle.
   */
  acknowledgeRecording: () => Promise<RecordingStatus>;
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
  getSession,
  readNotes,
  readTranscriptPage,
  repairSession,
  regenerateNotes,
  revealSession,
  deleteSession,
  archiveSession,
  copyNotes,
  copyTranscript,
  settingsSummary,
  recordingStatus,
  recordingPreflight,
  onRecordingTransition,
  onRecordingProgress,
  startRecording,
  stopRecording,
  acknowledgeRecording,
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
