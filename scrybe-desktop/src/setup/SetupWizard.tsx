import { useCallback, useState } from "react";

import { useScrybe } from "../ipc/ScrybeProvider";
import { useQuery } from "../shell/useQuery";
import { ReadyStep } from "./ReadyStep";
import { RecordingStep } from "./RecordingStep";
import { firstStep, STEPS, stepAfter, stepBefore, type Step } from "./steps";
import { TranscriptionStep } from "./TranscriptionStep";
import { useReadiness } from "./useReadiness";
import { WelcomeStep } from "./WelcomeStep";

/**
 * Guided setup: four steps, each of which can be left.
 *
 * The wizard owns navigation and nothing else. Every step reads its own
 * state from Rust and writes through the same services the settings
 * screen uses, so there is no setup-only path into the configuration
 * and nothing here can leave a value the settings screen cannot show.
 *
 * `onExit` is how the application is reached without finishing. It is
 * on every step, not only the last, because a user who wants to look at
 * what is already recorded should not have to complete an installation
 * first.
 */
export function SetupWizard({ onExit }: { onExit: () => void }) {
  const scrybe = useScrybe();
  const [step, setStep] = useState<Step>(firstStep);
  const [generation, setGeneration] = useState(0);
  const { readiness, recheck } = useReadiness();
  const settings = useQuery(() => scrybe.settingsForm(), `settings:${String(generation)}`);

  const changed = useCallback(() => {
    setGeneration((previous) => previous + 1);
    recheck();
  }, [recheck]);

  const index = STEPS.findIndex((candidate) => candidate.id === step.id);
  const previous = stepBefore(step.id);
  const next = stepAfter(step.id);

  return (
    <section className="setup" aria-labelledby="view-heading">
      <h1 id="view-heading">Setup</h1>
      <ol className="setup__progress" aria-label="Setup steps">
        {STEPS.map((candidate, position) => (
          <li
            key={candidate.id}
            className="setup__progress-step"
            aria-current={candidate.id === step.id ? "step" : undefined}
          >
            <span className="setup__progress-index">{position + 1}</span>
            {candidate.label}
          </li>
        ))}
      </ol>
      <h2>{step.label}</h2>

      {(settings.status === "loading" || readiness.status === "loading") && (
        <p>Checking this installation…</p>
      )}
      {settings.status === "failed" && (
        <p role="alert" className="setup__failure">
          {settings.message}
        </p>
      )}
      {readiness.status === "failed" && (
        <p role="alert" className="setup__failure">
          {readiness.message}
        </p>
      )}
      {settings.status === "ready" && readiness.status === "ready" && (
        <div className="setup__body">
          {step.id === "welcome" && <WelcomeStep settings={settings.value} />}
          {step.id === "recording" && (
            <RecordingStep
              settings={settings.value}
              readiness={readiness.value}
              onSaved={changed}
              onRecheck={recheck}
            />
          )}
          {step.id === "transcription" && (
            <TranscriptionStep
              settings={settings.value}
              readiness={readiness.value}
              onChanged={changed}
            />
          )}
          {step.id === "ready" && (
            <ReadyStep readiness={readiness.value} onRecheck={recheck} onFinish={onExit} />
          )}
        </div>
      )}

      <nav className="setup__nav" aria-label="Setup navigation">
        <button
          type="button"
          onClick={() => {
            if (previous !== undefined) {
              setStep(previous);
            }
          }}
          disabled={previous === undefined}
        >
          Back
        </button>
        <span className="setup__position">
          Step {index + 1} of {STEPS.length}
        </span>
        <button
          type="button"
          onClick={() => {
            if (next !== undefined) {
              setStep(next);
            }
          }}
          disabled={next === undefined}
        >
          Next
        </button>
        <button type="button" className="setup__exit" onClick={onExit}>
          Leave setup
        </button>
      </nav>
    </section>
  );
}
