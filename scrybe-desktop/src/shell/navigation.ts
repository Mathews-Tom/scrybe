import { createContext, use } from "react";

/** Moves the shell to the route with the given identity. */
export type Navigate = (routeId: string) => void;

/**
 * How a view asks the shell to go somewhere else.
 *
 * A context rather than a prop threaded through every view: a view that
 * never navigates should not have to accept and forward a function it
 * does not use, and the shell owns the single place navigation happens.
 * The default throws rather than doing nothing, because a view rendered
 * outside the shell asking to navigate is a wiring mistake, and
 * silently ignoring it would present as a button that does nothing.
 */
const NavigationContext = createContext<Navigate>(() => {
  throw new Error("no navigation is available outside the application shell");
});

export const NavigationProvider = NavigationContext;

export function useNavigate(): Navigate {
  return use(NavigationContext);
}
