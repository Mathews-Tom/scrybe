import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

import {
  MODEL_PROGRESS_EVENT,
  type DiagnosticRows,
  type ModelOffer,
  type ModelOutcome,
  type ModelProgress,
  type ReadinessReport,
  type RecoveryActionView,
  type RepairOutcome,
  type SettingsChange,
  type SettingsForm,
} from "../generated/bindings";

/**
 * Every setup command name, kept beside the setup calls rather than in
 * the shared list.
 *
 * `client.ts` concatenates this into the list it checks against the
 * host's capability files, so a name added here without a matching
 * grant fails `client.test.ts` rather than surfacing at runtime as a
 * screen that never loads.
 */
export const SETUP_COMMANDS = [
  "settings_form",
  "apply_settings",
  "diagnostics_report",
  "apply_recovery",
  "readiness_report",
  "model_offer",
  "install_model",
  "cancel_model_install",
  "open_system_settings",
  "open_advanced_configuration",
] as const;

/** The configuration a settings form renders, and what it may write. */
export function settingsForm(): Promise<SettingsForm> {
  return invoke<SettingsForm>("settings_form");
}

/** Applies a set of field changes as one unit. */
export function applySettings(changes: SettingsChange[]): Promise<SettingsForm> {
  return invoke<SettingsForm>("apply_settings", { changes });
}

/** Everything one read-only diagnosis found. Mutates nothing. */
export function diagnosticsReport(): Promise<DiagnosticRows> {
  return invoke<DiagnosticRows>("diagnostics_report");
}

/** Performs exactly one repair, chosen by the user. */
export function applyRecovery(action: RecoveryActionView): Promise<RepairOutcome> {
  return invoke<RepairOutcome>("apply_recovery", { action });
}

/** Capture, transcription, notes, storage, and egress, separately. */
export function readinessReport(): Promise<ReadinessReport> {
  return invoke<ReadinessReport>("readiness_report");
}

/**
 * What acquiring a model would involve.
 *
 * Reads the catalog and the filesystem; requests nothing. This is what
 * the confirmation prompt is built from.
 */
export function modelOffer(id: string): Promise<ModelOffer> {
  return invoke<ModelOffer>("model_offer", { id });
}

/**
 * Acquires a model, having shown the user `acknowledgedSha256`.
 *
 * Rust refuses a digest that is not the one the catalog offers, and
 * refuses it before opening a connection, so this cannot be called
 * usefully without having rendered the offer first.
 */
export function installModel(id: string, acknowledgedSha256: string): Promise<ModelOutcome> {
  return invoke<ModelOutcome>("install_model", { id, acknowledgedSha256 });
}

/** Asks whichever acquisition is running to stop. */
export function cancelModelInstall(): Promise<boolean> {
  return invoke<boolean>("cancel_model_install");
}

/**
 * Opens the System Settings pane where a refused capability is granted.
 *
 * Rust answers with the unit type, which `invoke` surfaces as `null`;
 * the return is narrowed to `Promise<void>` here so no caller is
 * tempted to read a value that carries no information.
 */
export async function openSystemSettings(capability: string): Promise<void> {
  await invoke<null>("open_system_settings", { capability });
}

/** Opens the configuration file for the settings no form models. */
export async function openAdvancedConfiguration(): Promise<void> {
  await invoke<null>("open_advanced_configuration");
}

/**
 * Calls `onProgress` for every model-download progress report until the
 * returned function is called.
 */
export function onModelProgress(
  onProgress: (progress: ModelProgress) => void,
): Promise<() => void> {
  return listen<ModelProgress>(MODEL_PROGRESS_EVENT, (event) => {
    onProgress(event.payload);
  });
}
