import { useState } from "react";

import { ROUTES, defaultRoute } from "./routes";

const FIRST_ROUTE = defaultRoute();

/**
 * The persistent frame: a primary navigation sidebar, the local/offline
 * status indicator, and the active view.
 */
export function AppShell() {
  const [activeId, setActiveId] = useState(FIRST_ROUTE.id);
  const active = ROUTES.find((route) => route.id === activeId) ?? FIRST_ROUTE;

  return (
    <div className="app-shell">
      <nav className="app-shell__sidebar" aria-label="Primary">
        <ul className="app-shell__nav">
          {ROUTES.map((route) => (
            <li key={route.id}>
              <button
                type="button"
                className="app-shell__nav-item"
                aria-current={route.id === active.id ? "page" : undefined}
                onClick={() => {
                  setActiveId(route.id);
                }}
              >
                {route.label}
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
        {active.render()}
      </main>
    </div>
  );
}
