import type { ReactElement } from "react";

import { SetupView } from "./views/SetupView";
import { SearchView } from "./views/SearchView";
import { SessionsView } from "./views/SessionsView";
import { SettingsView } from "./views/SettingsView";

/** One destination in the persistent sidebar. */
export interface Route {
  /** Stable, used as the React key and as the selected-view discriminant. */
  readonly id: string;
  /** The navigation control's accessible name and the view's heading. */
  readonly label: string;
  readonly render: () => ReactElement;
}

/**
 * Every destination, in sidebar order.
 *
 * A later feature area appends one entry and brings its own view; it
 * does not edit a switch, a union type, or a router table that another
 * area also has to touch. The first entry is what the window opens on.
 */
export const ROUTES: readonly Route[] = [
  { id: "sessions", label: "Sessions", render: () => <SessionsView /> },
  { id: "search", label: "Search", render: () => <SearchView /> },
  { id: "settings", label: "Settings", render: () => <SettingsView /> },
  { id: "setup", label: "Setup", render: () => <SetupView /> },
];

/**
 * The route guided setup lives at.
 *
 * Named rather than looked up by string at each call site, so the shell
 * can open on it and the wizard can leave it without either of them
 * repeating a literal the registry owns.
 */
export const SETUP_ROUTE_ID = "setup";

/**
 * The route the window opens on.
 *
 * @throws if the registry is empty — the window has to open on
 * something, and a blank main region would hide the defect.
 */
export function defaultRoute(): Route {
  const [first] = ROUTES;
  if (first === undefined) {
    throw new Error("the route registry is empty");
  }
  return first;
}
