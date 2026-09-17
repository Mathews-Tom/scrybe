import { useEffect, useState } from "react";

import type { ModelOffer, ModelOutcome } from "../generated/bindings";
import { useScrybe } from "../ipc/ScrybeProvider";

/** Bytes as a figure a person reads, alongside the exact count. */
function readable(bytes: string): string {
  const exact = Number(bytes);
  if (!Number.isFinite(exact)) {
    return `${bytes} bytes`;
  }
  const units = ["bytes", "kB", "MB", "GB", "TB"];
  let value = exact;
  let unit = 0;
  while (value >= 1000 && unit < units.length - 1) {
    value /= 1000;
    unit += 1;
  }
  const rounded = unit === 0 ? String(value) : value.toFixed(1);
  return `${rounded} ${units[unit] ?? "bytes"}`;
}

type Stage =
  | { readonly kind: "offer" }
  | { readonly kind: "downloading"; readonly received: string; readonly total: string }
  | { readonly kind: "done"; readonly outcome: ModelOutcome }
  | { readonly kind: "failed"; readonly message: string };

/**
 * Everything the user is shown before a model is requested, and the
 * confirmation that permits the request.
 *
 * The confirmation Rust accepts carries the digest, and the digest is
 * read from this offer — so a panel that did not render the offer could
 * not produce an acceptable confirmation. That is the acceptance
 * criterion expressed as a data dependency rather than as a rule this
 * component promises to follow.
 *
 * Progress is determinate while byte counts are arriving and
 * indeterminate before the first report, which is what the two
 * `aria-valuenow` states below mean. Cancelling and retrying are both
 * ordinary buttons; neither deletes anything, because a cancelled
 * download leaves its partial for the user to decide about.
 */
export function ModelOfferPanel({
  offer,
  onInstalled,
}: {
  offer: ModelOffer;
  onInstalled: () => void;
}) {
  const scrybe = useScrybe();
  const [stage, setStage] = useState<Stage>({ kind: "offer" });

  useEffect(() => {
    if (stage.kind !== "downloading") {
      return undefined;
    }
    let live = true;
    const subscription = scrybe.onModelProgress((progress) => {
      if (live && progress.id === offer.id) {
        setStage({
          kind: "downloading",
          received: progress.received_bytes,
          total: progress.total_bytes,
        });
      }
    });
    return () => {
      live = false;
      void subscription.then((stop) => {
        stop();
      });
    };
    // Keyed on the stage's kind rather than its payload, so the
    // subscription is established once per download rather than once
    // per progress report.
  }, [stage.kind, offer.id, scrybe]);

  function confirm() {
    setStage({ kind: "downloading", received: "0", total: offer.size_bytes });
    scrybe.installModel(offer.id, offer.sha256).then(
      (outcome) => {
        setStage({ kind: "done", outcome });
        if (outcome.state === "ready") {
          onInstalled();
        }
      },
      (error: unknown) => {
        setStage({ kind: "failed", message: describe(error) });
      },
    );
  }

  if (offer.state === "ready") {
    return (
      <p className="model__installed">
        The transcription model is installed and verified at {offer.destination_path}.
      </p>
    );
  }

  return (
    <section className="model" aria-labelledby="model-heading">
      <h3 id="model-heading">Transcription model</h3>
      <p>
        Downloading this model is the only network request guided setup makes. Nothing is
        requested until you choose to download it.
      </p>
      <dl className="model__facts">
        <div className="model__row">
          <dt>Source</dt>
          <dd>
            <span className="model__url">{offer.source_url}</span>
          </dd>
        </div>
        <div className="model__row">
          <dt>Revision</dt>
          <dd>
            <code>{offer.source_revision}</code>
          </dd>
        </div>
        <div className="model__row">
          <dt>Licence</dt>
          <dd>{offer.license}</dd>
        </div>
        <div className="model__row">
          <dt>Download size</dt>
          <dd>
            {readable(offer.size_bytes)} ({offer.size_bytes} bytes)
          </dd>
        </div>
        <div className="model__row">
          <dt>SHA-256</dt>
          <dd>
            <code className="model__digest">{offer.sha256}</code>
          </dd>
        </div>
        <div className="model__row">
          <dt>Runtime</dt>
          <dd>{offer.runtime}</dd>
        </div>
        <div className="model__row">
          <dt>Saved to</dt>
          <dd>{offer.destination_path}</dd>
        </div>
        <div className="model__row">
          <dt>Disk needed</dt>
          <dd>
            {readable(offer.required_bytes)}
            {offer.available_bytes === null
              ? " · free space unknown on this volume"
              : ` · ${readable(offer.available_bytes)} free`}
          </dd>
        </div>
      </dl>
      {!offer.sufficient_space && (
        <p className="model__warning" role="status">
          There is not enough room on this volume to install it. Free some space and check again.
        </p>
      )}
      {stage.kind === "offer" && (
        <button type="button" className="model__confirm" onClick={confirm} disabled={!offer.sufficient_space}>
          Download and verify this model
        </button>
      )}
      {stage.kind === "downloading" && (
        <Downloading
          received={stage.received}
          total={stage.total}
          onCancel={() => {
            void scrybe.cancelModelInstall();
          }}
        />
      )}
      {stage.kind === "done" && <Outcome outcome={stage.outcome} onRetry={confirm} />}
      {stage.kind === "failed" && (
        <>
          <p className="model__warning" role="alert">
            {stage.message}
          </p>
          <button type="button" onClick={confirm}>
            Try again
          </button>
        </>
      )}
    </section>
  );
}

function Downloading({
  received,
  total,
  onCancel,
}: {
  received: string;
  total: string;
  onCancel: () => void;
}) {
  const exact = Number(received);
  const whole = Number(total);
  const started = Number.isFinite(exact) && exact > 0 && Number.isFinite(whole) && whole > 0;
  return (
    <>
      <div
        className="model__progress"
        role="progressbar"
        aria-label="Downloading the transcription model"
        aria-valuemin={0}
        aria-valuemax={started ? 100 : undefined}
        aria-valuenow={started ? Math.round((exact / whole) * 100) : undefined}
        aria-valuetext={
          started ? `${readable(received)} of ${readable(total)}` : "starting the download"
        }
      >
        <span
          className="model__progress-fill"
          style={{ width: started ? `${String((exact / whole) * 100)}%` : "0%" }}
        />
      </div>
      <p aria-live="polite">
        {started ? `${readable(received)} of ${readable(total)}` : "Starting the download."}
      </p>
      <button type="button" onClick={onCancel}>
        Cancel
      </button>
    </>
  );
}

function Outcome({ outcome, onRetry }: { outcome: ModelOutcome; onRetry: () => void }) {
  if (outcome.state === "ready") {
    return (
      <p className="model__installed" role="status">
        The model is installed and verified.
      </p>
    );
  }
  const reason =
    outcome.state === "cancelled"
      ? "The download was cancelled. Nothing was installed, and the part that had arrived was kept where you can see it."
      : (outcome.failure ?? "The download did not complete.");
  return (
    <>
      <p className="model__warning" role="alert">
        {reason}
      </p>
      <button type="button" onClick={onRetry}>
        Try again
      </button>
    </>
  );
}

/**
 * A command failure arrives as the payload Rust serialized. Anything
 * else is a bridge failure and says so.
 */
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
