import { useCallback, useState } from "react";

import { useScrybe } from "../../ipc/ScrybeProvider";
import { DiagnosticsPanel } from "../../setup/DiagnosticsPanel";
import { ModelOfferPanel } from "../../setup/ModelOfferPanel";
import { ReadinessList } from "../../setup/ReadinessList";
import { SettingsFormPanel } from "../../setup/SettingsForm";
import { useReadiness } from "../../setup/useReadiness";
import { useQuery } from "../useQuery";
import { UpdatePanel } from "./UpdatePanel";

/** The catalog entry this application offers for local transcription. */
const MANAGED_MODEL = "whisper-small-en";

/**
 * Settings: readiness, the fields this release exposes, managed model
 * storage, and diagnostics.
 *
 * The four sections read independently and write through the same
 * services guided setup uses, so there is no settings-only path into
 * the configuration and nothing here can produce a value setup cannot
 * show. Reading the page performs no mutation — the diagnostics section
 * makes that explicit, and its repairs are buttons rather than
 * consequences of rendering.
 */
export function SettingsView() {
  const scrybe = useScrybe();
  const [generation, setGeneration] = useState(0);
  // The write confirmation lives here rather than in the form panel.
  // Saving calls `changed`, which reloads the form and unmounts that
  // panel while the reload is in flight, so a flag set beside the write
  // was destroyed before it could ever render.
  const [saved, setSaved] = useState(false);
  const { readiness, recheck } = useReadiness();
  const form = useQuery(() => scrybe.settingsForm(), `settings-form:${String(generation)}`);
  const offer = useQuery(() => scrybe.modelOffer(MANAGED_MODEL), `offer:${String(generation)}`);

  const changed = useCallback(() => {
    setSaved(false);
    setGeneration((previous) => previous + 1);
    recheck();
  }, [recheck]);

  const settingsSaved = useCallback(() => {
    setGeneration((previous) => previous + 1);
    recheck();
    setSaved(true);
  }, [recheck]);

  const settingsEdited = useCallback(() => {
    setSaved(false);
  }, []);

  return (
    <>
      <h1 id="view-heading">Settings</h1>

      <section aria-labelledby="settings-readiness-heading">
        <h2 id="settings-readiness-heading">Readiness</h2>
        {readiness.status === "loading" && <p>Checking this installation…</p>}
        {readiness.status === "failed" && (
          <p role="alert" className="setup__failure">
            {readiness.message}
          </p>
        )}
        {readiness.status === "ready" && <ReadinessList readiness={readiness.value} />}
      </section>

      {form.status === "loading" && <p>Reading the configuration…</p>}
      {form.status === "failed" && (
        <p role="alert" className="setup__failure">
          {form.message}
        </p>
      )}
      {form.status === "ready" && (
        <SettingsFormPanel
          form={form.value}
          onSaved={settingsSaved}
          onEdited={settingsEdited}
        />
      )}
      {saved && (
        <p role="status" className="setup__saved">
          Saved.
        </p>
      )}

      <section aria-labelledby="settings-model-heading">
        <h2 id="settings-model-heading">Managed model storage</h2>
        {offer.status === "failed" && (
          <p role="alert" className="setup__failure">
            {offer.message}
          </p>
        )}
        {offer.status === "ready" && (
          <ModelOfferPanel offer={offer.value} onInstalled={changed} />
        )}
      </section>

      <UpdatePanel />

      <DiagnosticsPanel onRepaired={changed} />
    </>
  );
}
