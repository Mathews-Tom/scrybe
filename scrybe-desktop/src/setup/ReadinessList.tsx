import type { ReadinessFacet, ReadinessReport } from "../generated/bindings";

/** Which facets are shown, in order, and what each is called. */
type FacetKey = Exclude<keyof ReadinessReport, "can_record">;

const FACETS: readonly { key: FacetKey; label: string }[] = [
  { key: "capture", label: "Recording" },
  { key: "transcription", label: "Transcription" },
  { key: "notes", label: "Notes" },
  { key: "storage", label: "Storage" },
  { key: "egress", label: "Privacy" },
];

function word(facet: ReadinessFacet): string {
  switch (facet.state) {
    case "ready":
      return "Ready";
    case "blocked":
      return "Needs attention";
    case "not_configured":
      return "Not configured";
    case "unverified":
      return "Not checked";
  }
}

/**
 * The five answers, reported separately and in words.
 *
 * State is carried by text rather than by colour alone: a reader using
 * a screen reader, or one who cannot distinguish the palette, gets the
 * same answer as everyone else. The colour is a hint layered on top.
 */
export function ReadinessList({ readiness }: { readiness: ReadinessReport }) {
  return (
    <dl className="readiness">
      {FACETS.map(({ key, label }) => {
        const facet = readiness[key];
        return (
          <div className="readiness__row" key={key}>
            <dt className="readiness__term">{label}</dt>
            <dd className="readiness__state" data-state={facet.state}>
              {word(facet)}
            </dd>
            <dd className="readiness__detail">{facet.summary}</dd>
          </div>
        );
      })}
    </dl>
  );
}
