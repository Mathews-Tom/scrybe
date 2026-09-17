import { useState } from "react";

import type { ReadinessReport, SettingsForm } from "../generated/bindings";
import { useScrybe } from "../ipc/ScrybeProvider";

/** The two capabilities macOS gates a meeting recording behind. */
const CAPABILITIES = [
  {
    slug: "microphone",
    label: "Microphone",
    why: "Your own side of the conversation is captured through the microphone. Without this, a recording holds only what the other people said.",
  },
  {
    slug: "system_audio_recording",
    label: "Screen & System Audio Recording",
    why: "The other participants arrive as the sound your Mac is playing, which macOS groups with screen recording. Scrybe captures audio only — it requests no video frames and writes no screenshots.",
  },
] as const;

/**
 * The microphone, and the two permissions macOS asks for.
 *
 * Each capability is explained before anything is asked for, which is
 * the point of the step: a permission dialog that arrives with no
 * stated reason is one most people refuse. macOS raises the dialog
 * itself the first time a recording actually starts; this step does not
 * raise it, so it cannot raise it repeatedly, and what it offers
 * instead is the System Settings pane where a refused capability is
 * granted afterwards.
 */
export function RecordingStep({
  settings,
  readiness,
  onSaved,
  onRecheck,
}: {
  settings: SettingsForm;
  readiness: ReadinessReport;
  onSaved: () => void;
  onRecheck: () => void;
}) {
  const scrybe = useScrybe();
  const [device, setDevice] = useState(settings.capture_mic_device);
  const [saved, setSaved] = useState(false);
  const [failure, setFailure] = useState<string | null>(null);

  function save() {
    setFailure(null);
    scrybe.applySettings([{ field: "capture_mic_device", value: device }]).then(
      () => {
        setSaved(true);
        onSaved();
      },
      (error: unknown) => {
        setFailure(describe(error));
      },
    );
  }

  return (
    <>
      <h3>Microphone</h3>
      <p>
        Name the input device to record from. <code>default</code> follows whatever macOS has
        selected in Sound settings, which is what most setups want.
      </p>
      <div className="setup__field">
        <label htmlFor="setup-mic">Microphone device</label>
        <input
          id="setup-mic"
          type="text"
          value={device}
          onChange={(event) => {
            setDevice(event.target.value);
            setSaved(false);
          }}
        />
        <button type="button" onClick={save} disabled={device === settings.capture_mic_device}>
          Save
        </button>
      </div>
      {saved && (
        <p role="status" className="setup__saved">
          Saved. Your advanced settings and comments were left untouched.
        </p>
      )}
      {failure !== null && (
        <p role="alert" className="setup__failure">
          {failure}
        </p>
      )}

      <h3>Permissions</h3>
      <p>
        macOS asks for each of these the first time a recording needs it. Here is what it is
        asking for and why, so the dialog is not a surprise.
      </p>
      <ul className="setup__capabilities">
        {CAPABILITIES.map((capability) => (
          <li key={capability.slug}>
            <h4>{capability.label}</h4>
            <p>{capability.why}</p>
            <button
              type="button"
              onClick={() => {
                void scrybe.openSystemSettings(capability.slug);
              }}
            >
              Open {capability.label} settings
            </button>
          </li>
        ))}
      </ul>
      <p className="setup__note">
        If you have already refused one of these, macOS will not ask again — use the button for
        it above, turn Scrybe on, and then check again here.
      </p>
      <p aria-live="polite" className="setup__facet" data-state={readiness.capture.state}>
        {readiness.capture.summary}
      </p>
      <button type="button" onClick={onRecheck}>
        Check again
      </button>
    </>
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
