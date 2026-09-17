import { useCallback, useState } from "react";

import { useScrybe } from "../ipc/ScrybeProvider";
import { useQuery, type Query } from "../shell/useQuery";
import type { ReadinessReport } from "../generated/bindings";

/**
 * Readiness, and a way to ask again.
 *
 * Setup changes the thing it is reporting on — a model is installed, a
 * permission is granted in System Settings, a field is saved — so a
 * read that happened once on mount would show a stale answer for the
 * rest of the session. The counter is what a step increments after
 * doing something, and it is the query key, so asking again is one
 * call rather than a manual refetch protocol.
 *
 * Reading readiness performs no mutation: it is a projection of the
 * read-only diagnosis, which is why a step may ask as often as it
 * likes.
 */
export function useReadiness(): {
  readiness: Query<ReadinessReport>;
  recheck: () => void;
} {
  const scrybe = useScrybe();
  const [generation, setGeneration] = useState(0);
  const readiness = useQuery(() => scrybe.readinessReport(), `readiness:${String(generation)}`);
  const recheck = useCallback(() => {
    setGeneration((previous) => previous + 1);
  }, []);
  return { readiness, recheck };
}
