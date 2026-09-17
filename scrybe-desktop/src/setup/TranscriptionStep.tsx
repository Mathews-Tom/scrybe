import type { ReadinessReport, SettingsForm } from "../generated/bindings";
import { useScrybe } from "../ipc/ScrybeProvider";
import { useQuery } from "../shell/useQuery";
import { ModelOfferPanel } from "./ModelOfferPanel";

/** The catalog entry this application offers for local transcription. */
const MANAGED_MODEL = "whisper-small-en";

/**
 * Speech to text, and text to notes — reported separately because they
 * can succeed separately.
 *
 * Transcription needs a model file on this Mac. Notes need a local
 * provider answering on this Mac. Neither asks for a hosted account,
 * and this step collects no credential: a hosted provider remains
 * configurable through the advanced settings, where its credential
 * lives in an environment variable this application never reads into
 * the interface.
 *
 * Notes being unavailable is stated as acceptable rather than as a
 * failure, because it is: a recording completes without them and says
 * so afterwards.
 */
export function TranscriptionStep({
  settings,
  readiness,
  onChanged,
}: {
  settings: SettingsForm;
  readiness: ReadinessReport;
  onChanged: () => void;
}) {
  const scrybe = useScrybe();
  const offer = useQuery(() => scrybe.modelOffer(MANAGED_MODEL), `offer:${settings.stt_model}`);

  return (
    <>
      <h3>Transcription</h3>
      <p aria-live="polite" className="setup__facet" data-state={readiness.transcription.state}>
        {readiness.transcription.summary}
      </p>
      {offer.status === "loading" && <p>Reading the model catalog…</p>}
      {offer.status === "failed" && (
        <p role="alert" className="setup__failure">
          {offer.message}
        </p>
      )}
      {offer.status === "ready" && (
        <ModelOfferPanel offer={offer.value} onInstalled={onChanged} />
      )}

      <h3>Notes</h3>
      <p>
        Notes are written by a language model running on this Mac, reached at{" "}
        <span className="setup__path">{settings.llm_base_url}</span>. Nothing here asks for an
        account or an API key.
      </p>
      <p aria-live="polite" className="setup__facet" data-state={readiness.notes.state}>
        {readiness.notes.summary}
      </p>
      {readiness.notes.state !== "ready" && (
        <p className="setup__note">
          You can finish setup without this. Recordings will still be captured and transcribed;
          each one records that notes were unavailable, and you can generate them later once a
          local provider is running.
        </p>
      )}
      <button type="button" onClick={onChanged}>
        Check again
      </button>
    </>
  );
}
