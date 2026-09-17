import { useScrybe } from "../../ipc/ScrybeProvider";
import { useQuery } from "../useQuery";

export function SettingsView() {
  const scrybe = useScrybe();
  const settings = useQuery(() => scrybe.settingsSummary(), "settings");

  if (settings.status === "loading") {
    return (
      <>
        <h1 id="view-heading">Settings</h1>
        <p role="status">Loading settings…</p>
      </>
    );
  }
  if (settings.status === "failed") {
    return (
      <>
        <h1 id="view-heading">Settings</h1>
        <p role="alert">{settings.message}</p>
      </>
    );
  }

  const summary = settings.value;
  const entries: [string, string][] = [
    ["Storage root", summary.storage_root],
    [
      "Configuration file",
      summary.config_exists ? summary.config_path : `${summary.config_path} (not yet written)`,
    ],
    ["Capture source", summary.capture_source],
    ["Transcription", `${summary.transcription_provider} · ${summary.transcription_model}`],
    ["Notes", `${summary.notes_provider} · ${summary.notes_model}`],
  ];

  return (
    <>
      <h1 id="view-heading">Settings</h1>
      <dl className="settings">
        {entries.map(([term, value]) => (
          <div className="settings__row" key={term}>
            <dt>{term}</dt>
            <dd>{value}</dd>
          </div>
        ))}
      </dl>
      {summary.warnings.length > 0 && (
        <>
          <h2>Warnings</h2>
          <ul className="settings__warnings">
            {summary.warnings.map((warning) => (
              <li key={warning.message}>
                {warning.severity === "error" ? "Error" : "Warning"}: {warning.message}
              </li>
            ))}
          </ul>
        </>
      )}
    </>
  );
}
