import { useEffect, useState } from "react";

/** What a view knows about an in-flight or finished read. */
export type Query<T> =
  | { readonly status: "loading" }
  | { readonly status: "ready"; readonly value: T }
  | { readonly status: "failed"; readonly message: string };

const LOADING = { status: "loading" } as const;

/**
 * Runs `read` when `key` changes and reports the outcome.
 *
 * The key is carried alongside the outcome rather than reset when it
 * changes, so a view renders `loading` for a new key immediately
 * instead of showing the previous key's result for one frame.
 *
 * A failure is surfaced rather than swallowed: a view that quietly
 * rendered an empty list when Rust reported an unreadable storage root
 * would tell the reader their recordings are gone.
 */
export function useQuery<T>(read: () => Promise<T>, key: string): Query<T> {
  const [settled, setSettled] = useState<{ key: string; query: Query<T> }>({
    key,
    query: LOADING,
  });

  useEffect(() => {
    let current = true;
    read().then(
      (value) => {
        if (current) {
          setSettled({ key, query: { status: "ready", value } });
        }
      },
      (error: unknown) => {
        if (current) {
          setSettled({ key, query: { status: "failed", message: describe(error) } });
        }
      },
    );
    return () => {
      current = false;
    };
    // `read` is rebuilt on every render by design; `key` is what
    // actually identifies the request.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [key]);

  return settled.key === key ? settled.query : LOADING;
}

/**
 * A command failure arrives as the payload Rust serialized, which
 * carries a stable code and a message the service layer wrote. Anything
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
