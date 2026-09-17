import { useCallback, useState } from "react";

import { useScrybe } from "../ipc/ScrybeProvider";
import { NavigationProvider } from "./navigation";
import { ROUTES, SETUP_ROUTE_ID, defaultRoute } from "./routes";
import { useQuery, type Query } from "./useQuery";
import type { SettingsSummary } from "../generated/bindings";

const FIRST_ROUTE = defaultRoute();

/**
 * What the persistent status indicator says.
 *
 * It reports whether this installation is configured to reach a hosted
 * provider, which is the one thing about its configuration that changes
 * whether using it involves the network at all. It is text, not a
 * colour: the dot beside it is decorative and hidden from assistive
 * technology.
 */
function statusText(settings: Query<SettingsSummary>): string {
  switch (settings.status) {
    case "loading":
      return "Checking providers…";
    case "failed":
      return "Provider configuration unavailable";
    case "ready":
      return settings.value.hosted_credential_required
        ? "A hosted provider is configured"
        : "Local only — no account, no cloud";
  }
}

/**
 * The persistent frame: a primary navigation sidebar, the local/offline
 * status indicator, and the active view.
 */
export function AppShell() {
  const scrybe = useScrybe();
  // `null` until the reader picks a destination, which is what lets the
  // readiness answer below choose the first one without overriding a
  // choice already made.
  const [chosenId, setChosenId] = useState<string | null>(null);
  const settings = useQuery(() => scrybe.settingsSummary(), "settings");
  const readiness = useQuery(() => scrybe.readinessReport(), "readiness");

  // An installation that cannot record opens on setup. Derived from
  // readiness rather than from a "setup completed" flag: a flag would
  // have to be written somewhere, would go stale the moment a model was
  // deleted or a permission revoked, and would leave a reader whose
  // installation had broken looking at a session list that could not
  // record. Until readiness settles the shell opens where it always
  // has, so a healthy launch is not delayed by a check that will say
  // nothing is wrong.
  const opensOnSetup = readiness.status === "ready" && !readiness.value.can_record;
  const activeId = chosenId ?? (opensOnSetup ? SETUP_ROUTE_ID : FIRST_ROUTE.id);
  const active = ROUTES.find((route) => route.id === activeId) ?? FIRST_ROUTE;
  const navigate = useCallback((routeId: string) => {
    setChosenId(routeId);
  }, []);

  return (
    <NavigationProvider value={navigate}>
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
                  navigate(route.id);
                }}
              >
                {route.label}
              </button>
            </li>
          ))}
        </ul>
        <p className="app-shell__status">
          <span aria-hidden="true" className="app-shell__status-dot" />
          {statusText(settings)}
        </p>
      </nav>
        <main className="app-shell__main" aria-labelledby="view-heading">
          {active.render()}
        </main>
      </div>
    </NavigationProvider>
  );
}
