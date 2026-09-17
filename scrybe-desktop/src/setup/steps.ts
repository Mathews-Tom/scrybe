import type { ReadinessReport } from "../generated/bindings";

/** One step of guided setup, in the order they are presented. */
export interface Step {
  /** Stable, used as the React key and in the progress indicator. */
  readonly id: "welcome" | "recording" | "transcription" | "ready";
  /** The step's heading and its label in the progress indicator. */
  readonly label: string;
}

export const STEPS: readonly Step[] = [
  { id: "welcome", label: "Welcome" },
  { id: "recording", label: "Recording" },
  { id: "transcription", label: "Transcription and notes" },
  { id: "ready", label: "Ready" },
];

/**
 * The first step, which the wizard opens on.
 *
 * @throws if the registry is empty — the wizard has to start somewhere,
 * and a blank first step would hide the defect.
 */
export function firstStep(): Step {
  const [first] = STEPS;
  if (first === undefined) {
    throw new Error("the setup step registry is empty");
  }
  return first;
}

/** The step after `id`, or `undefined` at the end. */
export function stepAfter(id: Step["id"]): Step | undefined {
  return STEPS[STEPS.findIndex((step) => step.id === id) + 1];
}

/** The step before `id`, or `undefined` at the start. */
export function stepBefore(id: Step["id"]): Step | undefined {
  const index = STEPS.findIndex((step) => step.id === id);
  return index > 0 ? STEPS[index - 1] : undefined;
}

/**
 * Whether guided setup should be presented rather than the session
 * list.
 *
 * Derived from readiness rather than from a "setup completed" flag.
 * A flag would have to be written somewhere, would go stale the moment
 * a model was deleted or a permission revoked, and would mean a user
 * whose install broke was shown a session list that could not record.
 * The question the wizard exists to answer is whether this install can
 * record, so that is the question asked.
 */
export function needsSetup(readiness: ReadinessReport): boolean {
  return !readiness.can_record;
}
