import { useState } from "react";

import type { SettingsChange, SettingsField, SettingsForm } from "../generated/bindings";
import { useScrybe } from "../ipc/ScrybeProvider";

/** A field, its label, and the value it currently holds. */
interface Row {
  readonly field: SettingsField;
  readonly label: string;
  readonly help?: string;
}

/**
 * The fields this release exposes, in the order the design lists them.
 *
 * A subset of what the service layer will accept: the closed write
 * surface is wider than what one screen should present, and the rest
 * stays behind `Open advanced configuration`. Nothing here can name a
 * credential key, because no such field exists in the enumeration to
 * name.
 */
const ROWS: readonly Row[] = [
  { field: "storage_root", label: "Storage root", help: "Where every session folder is written." },
  { field: "capture_mic_device", label: "Microphone", help: "`default` follows macOS Sound settings." },
  {
    field: "record_system_backend",
    label: "System-audio backend",
    help: "`sck` is ScreenCaptureKit. `tap` is the legacy Core Audio Tap recovery path.",
  },
  { field: "stt_model", label: "Transcription model" },
  { field: "llm_base_url", label: "Local notes endpoint" },
  { field: "llm_model", label: "Local notes model" },
  { field: "shell_indicators", label: "Recording indicators", help: "One per line." },
  { field: "capture_hotkey", label: "Global hotkey", help: "Leave empty for none." },
];

function currentValue(form: SettingsForm, field: SettingsField): string {
  switch (field) {
    case "storage_root":
      return form.storage_root;
    case "capture_mic_device":
      return form.capture_mic_device;
    case "capture_hotkey":
      return form.capture_hotkey ?? "";
    case "record_system_backend":
      return form.record_system_backend;
    case "stt_model":
      return form.stt_model;
    case "llm_base_url":
      return form.llm_base_url;
    case "llm_model":
      return form.llm_model;
    case "shell_indicators":
      return form.shell_indicators.join("\n");
    default:
      return "";
  }
}

function proposed(field: SettingsField, text: string): SettingsChange {
  if (field === "shell_indicators") {
    return {
      field,
      value: text
        .split("\n")
        .map((line) => line.trim())
        .filter((line) => line !== ""),
    };
  }
  return { field, value: text };
}

/**
 * The GUI-owned fields, saved as one unit.
 *
 * Rust edits the document in place, validates the complete candidate
 * through the strict schema, and only then replaces the file — so a
 * rejected change leaves the previous file exactly as it was, and an
 * accepted one leaves every comment and every advanced block this form
 * has no concept of untouched. Both of those are stated to the reader
 * rather than left to be trusted, because a settings screen that
 * silently rewrote a hand-edited file is precisely what this one must
 * not be mistaken for.
 *
 * The confirmation for a successful write is not this component's, and
 * cannot be: `onSaved` reloads the form, which unmounts this panel
 * while the reload is in flight, so any state set beside that call is
 * destroyed before it can render. `SettingsView` owns it.
 */
export function SettingsFormPanel({
  form,
  onSaved,
  onEdited,
}: {
  form: SettingsForm;
  onSaved: () => void;
  /// Called when a field changes, so the view can retire a "Saved."
  /// that no longer describes what is in the form.
  onEdited: () => void;
}) {
  const scrybe = useScrybe();
  const [edits, setEdits] = useState<Partial<Record<SettingsField, string>>>({});
  const [failure, setFailure] = useState<string | null>(null);

  const changes = ROWS.filter(
    (row) => edits[row.field] !== undefined && edits[row.field] !== currentValue(form, row.field),
  ).map((row) => proposed(row.field, edits[row.field] ?? ""));

  function save() {
    setFailure(null);
    scrybe.applySettings(changes).then(
      () => {
        setEdits({});
        // `onSaved` reloads the form, which unmounts this component
        // while the new one is in flight — so the confirmation cannot
        // live in this component's state. It is the view's, and is
        // rendered beside the panel rather than inside it.
        onSaved();
      },
      (error: unknown) => {
        setFailure(describe(error));
      },
    );
  }

  function openAdvanced() {
    setFailure(null);
    scrybe.openAdvancedConfiguration().then(
      () => undefined,
      (error: unknown) => {
        // Reported rather than dropped. On a fresh install the file
        // does not exist yet and the call fails, and swallowing it made
        // the button look like it had done nothing at all.
        setFailure(describe(error));
      },
    );
  }

  return (
    <section aria-labelledby="settings-fields-heading">
      <h2 id="settings-fields-heading">Configuration</h2>
      <p className="setup__note">
        Saving rewrites only the fields below. Comments, ordering, and any advanced block this
        form does not model are preserved, and a change the schema rejects leaves the previous
        file exactly as it was.
      </p>
      {ROWS.map((row) => {
        const id = `settings-${row.field}`;
        const value = edits[row.field] ?? currentValue(form, row.field);
        return (
          <div className="setup__field" key={row.field}>
            <label htmlFor={id}>{row.label}</label>
            {row.field === "shell_indicators" ? (
              <textarea
                id={id}
                rows={3}
                value={value}
                aria-describedby={row.help === undefined ? undefined : `${id}-help`}
                onChange={(event) => {
                  setEdits({ ...edits, [row.field]: event.target.value });
                  onEdited();
                }}
              />
            ) : (
              <input
                id={id}
                type="text"
                value={value}
                aria-describedby={row.help === undefined ? undefined : `${id}-help`}
                onChange={(event) => {
                  setEdits({ ...edits, [row.field]: event.target.value });
                  onEdited();
                }}
              />
            )}
            {row.help !== undefined && (
              <span className="setup__note" id={`${id}-help`}>
                {row.help}
              </span>
            )}
          </div>
        );
      })}
      <div className="setup__actions">
        <button
          type="button"
          className="setup__primary"
          onClick={save}
          disabled={changes.length === 0}
        >
          Save changes
        </button>
        <button type="button" onClick={openAdvanced}>
          Open advanced configuration
        </button>
      </div>
      <p className="setup__note">
        Advanced configuration opens {form.config_path} in whatever macOS opens it with. Hosted
        provider credentials live in environment variables named there, and are never read into
        this window.
      </p>
      {form.warnings.length > 0 && (
        <>
          <h3>Warnings</h3>
          <ul className="settings__warnings">
            {form.warnings.map((warning) => (
              <li key={warning.message}>
                {warning.severity === "error" ? "Error" : "Warning"}: {warning.message}
              </li>
            ))}
          </ul>
        </>
      )}
      {failure !== null && (
        <p role="alert" className="setup__failure">
          {failure}
        </p>
      )}
    </section>
  );
}

function describe(error: unknown): string {
  if (
    typeof error === "object" &&
    error !== null &&
    "message" in error &&
    typeof error.message === "string"
  ) {
    return error.message;
  }
  return "The application could not reach its own services.";
}
