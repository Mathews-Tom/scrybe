import type { SessionDetail, SessionProgress } from "../../generated/bindings";

const PROGRESS_LABEL: Record<SessionProgress, string> = {
  complete: "Complete",
  unfinished: "Unfinished",
  repairable: "Needs repair",
  failed: "Failed",
};

/** One row of the facts list, or nothing when there is no fact. */
interface Fact {
  readonly term: string;
  readonly value: string;
}

function at(value: string | null): string | null {
  if (value === null) {
    return null;
  }
  const parsed = new Date(value);
  return Number.isNaN(parsed.getTime()) ? null : parsed.toLocaleString();
}

function minutes(seconds: number | null): string | null {
  if (seconds === null) {
    return null;
  }
  const whole = Math.round(seconds / 60);
  return whole === 1 ? "1 minute" : `${whole.toString()} minutes`;
}

/**
 * The artifacts the session has, named rather than ticked.
 *
 * A row of checkboxes tells a reader which five things exist only if
 * they already know what the five are. A sentence naming what is there
 * and what is not works either way, and is what the reader needs when
 * an action is offered or withheld because of one of them.
 */
function artifacts(detail: SessionDetail): string {
  const present: string[] = [];
  const absent: string[] = [];
  const named: [boolean, string][] = [
    [detail.artifacts.notes, "notes"],
    [detail.artifacts.transcript, "transcript"],
    [detail.artifacts.audio, "audio"],
    [detail.artifacts.playback, "playback audio"],
    [detail.artifacts.metadata, "metadata"],
  ];
  for (const [has, name] of named) {
    (has ? present : absent).push(name);
  }
  if (present.length === 0) {
    return `None on disk. Missing: ${absent.join(", ")}.`;
  }
  return absent.length === 0
    ? `${present.join(", ")}.`
    : `${present.join(", ")}. Missing: ${absent.join(", ")}.`;
}

function capture(detail: SessionDetail): string | null {
  const { channels, layout, sample_rate_hz, bitrate_bps } = detail.capture;
  const parts: string[] = [];
  if (channels !== null) {
    parts.push(channels === 1 ? "mono" : `${channels.toString()} channels`);
  }
  if (layout !== null) {
    parts.push(layout);
  }
  if (sample_rate_hz !== null) {
    parts.push(`${(sample_rate_hz / 1000).toString()} kHz`);
  }
  if (bitrate_bps !== null) {
    parts.push(`${(bitrate_bps / 1000).toString()} kbps`);
  }
  return parts.length === 0 ? null : parts.join(" · ");
}

function providers(detail: SessionDetail): string | null {
  const named = [
    detail.providers.stt === null ? null : `transcribed by ${detail.providers.stt}`,
    detail.providers.llm === null ? null : `noted by ${detail.providers.llm}`,
    detail.providers.diarizer === null ? null : `attributed by ${detail.providers.diarizer}`,
  ].filter((part): part is string => part !== null);
  return named.length === 0 ? null : named.join(" · ");
}

/**
 * What is known about this session, from its own metadata.
 *
 * A session with no readable `meta.toml` has none of it, and the rows
 * it has no answer for are left out rather than filled with a dash: an
 * absent fact and a fact whose value is unknown are the same thing
 * here, and inventing a placeholder for either would put a value on
 * screen that nothing on disk says.
 */
export function SessionFacts({ detail }: { detail: SessionDetail }) {
  const facts: Fact[] = [
    { term: "State", value: PROGRESS_LABEL[detail.progress] },
    { term: "Identity", value: detail.session_id ?? detail.id },
    { term: "Started", value: at(detail.started_at) ?? "" },
    { term: "Ended", value: at(detail.ended_at) ?? "" },
    { term: "Length", value: minutes(detail.duration_secs) ?? "" },
    { term: "Capture", value: capture(detail) ?? "" },
    { term: "Providers", value: providers(detail) ?? "" },
    { term: "On disk", value: artifacts(detail) },
  ].filter((fact) => fact.value !== "");

  return (
    <dl className="session-facts">
      {facts.map((fact) => (
        <div key={fact.term} className="session-facts__row">
          <dt>{fact.term}</dt>
          <dd>{fact.value}</dd>
        </div>
      ))}
    </dl>
  );
}
