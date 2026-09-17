import { useSyncExternalStore } from "react";

/**
 * Names the next query this window makes.
 *
 * A cancellation token does not serialize and `invoke` has no abort, so
 * a query is abandoned by name: the view mints one of these, passes it
 * with the query, and hands the same string to `cancelQuery` when it
 * gives up on the answer. Monotonic rather than random because the
 * ordering is the useful part — a larger number is a later query.
 */
let minted = 0;
export function nextRequestId(): string {
  minted += 1;
  return `q${minted.toString()}`;
}

/**
 * The one storage-root watcher this window has.
 *
 * Ordinary files on disk are the source of truth, and anything may
 * write to them: the command-line tool, a recording this application
 * started, or the reader moving a folder in Finder. Nothing tells the
 * window when that happened. What the window does know is when it was
 * given the reader's attention back, which is the moment a stale list
 * starts being visible, so that is when every library view re-reads.
 *
 * One module-level subscription serves every view rather than one per
 * view: raising a window emits `focus` and `visibilitychange` together,
 * and a per-view listener would turn one return-to-the-window into as
 * many rescans as there are mounted views. The two events are coalesced
 * into a single revision bump on the next microtask, so the burst that
 * raising a window produces costs exactly one re-read.
 *
 * Deliberately not a filesystem watcher. The repository already decides
 * staleness from a fingerprint of the root, which sees an artifact
 * appear or disappear on every filesystem and without a watcher's
 * per-platform behaviour; this only says when to ask it again.
 */
const subscribers = new Set<() => void>();
let revision = 0;
let coalescing = false;

function invalidate(): void {
  if (coalescing) {
    return;
  }
  coalescing = true;
  queueMicrotask(() => {
    coalescing = false;
    revision += 1;
    for (const notify of subscribers) {
      notify();
    }
  });
}

/**
 * A window becoming hidden changes nothing the reader can see, and
 * re-reading the root for a window nobody is looking at is work with no
 * observer.
 */
function onVisibilityChange(): void {
  if (document.visibilityState === "visible") {
    invalidate();
  }
}

/**
 * Calls `notify` whenever the storage root may have changed, until the
 * returned function is called.
 *
 * Exported so the coalescing above can be observed for what it is. Read
 * through React it cannot be: `useSyncExternalStore` hands two bumps in
 * one turn to one render, so a view would look correctly coalesced even
 * with the coalescing removed.
 */
export function onStorageRootChanged(notify: () => void): () => void {
  if (subscribers.size === 0) {
    window.addEventListener("focus", invalidate);
    document.addEventListener("visibilitychange", onVisibilityChange);
  }
  subscribers.add(notify);
  return () => {
    subscribers.delete(notify);
    if (subscribers.size === 0) {
      window.removeEventListener("focus", invalidate);
      document.removeEventListener("visibilitychange", onVisibilityChange);
    }
  };
}

/**
 * A number that changes whenever the storage root may have changed
 * under this window.
 *
 * Views key their reads on it, so a changed revision re-runs the read
 * through the same path a first render takes.
 */
export function useStorageRootRevision(): number {
  return useSyncExternalStore(onStorageRootChanged, () => revision);
}
