import { useState } from "react";

import { SearchView } from "./views/SearchView";
import { SessionsView } from "./views/SessionsView";
import { SettingsView } from "./views/SettingsView";

type ViewId = "sessions" | "search" | "settings";

const NAVIGATION: readonly { id: ViewId; label: string }[] = [
  { id: "sessions", label: "Sessions" },
  { id: "search", label: "Search" },
  { id: "settings", label: "Settings" },
];

function view(id: ViewId) {
  switch (id) {
    case "sessions":
      return <SessionsView />;
    case "search":
      return <SearchView />;
    case "settings":
      return <SettingsView />;
  }
}

/**
 * The persistent frame: a primary navigation sidebar, the local/offline
 * status indicator, and the active view.
 */
export function AppShell() {
  const [active, setActive] = useState<ViewId>("sessions");

  return (
    <div className="app-shell">
      <nav className="app-shell__sidebar" aria-label="Primary">
        <ul className="app-shell__nav">
          {NAVIGATION.map(({ id, label }) => (
            <li key={id}>
              <button
                type="button"
                className="app-shell__nav-item"
                aria-current={active === id ? "page" : undefined}
                onClick={() => {
                  setActive(id);
                }}
              >
                {label}
              </button>
            </li>
          ))}
        </ul>
        <p className="app-shell__status">
          <span aria-hidden="true" className="app-shell__status-dot" />
          Local only — no account, no cloud
        </p>
      </nav>
      <main className="app-shell__main" aria-labelledby="view-heading">
        {view(active)}
      </main>
    </div>
  );
}
