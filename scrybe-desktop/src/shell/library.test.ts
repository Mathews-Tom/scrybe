import { afterEach, describe, expect, it, vi } from "vitest";

import { nextRequestId, onStorageRootChanged } from "./library";

/** Lets the microtask the watcher publishes on run. */
function published(): Promise<void> {
  return Promise.resolve();
}

function hide(state: "visible" | "hidden"): void {
  Object.defineProperty(document, "visibilityState", {
    value: state,
    configurable: true,
  });
}

afterEach(() => {
  hide("visible");
  vi.restoreAllMocks();
});

describe("the storage-root watcher", () => {
  it("test_one_return_to_the_window_is_published_once_however_many_events_it_raises", async () => {
    // Raising a window emits `focus` and `visibilitychange` together.
    // Uncoalesced, each return to the window would rescan the whole
    // storage root once per event.
    let notified = 0;
    const stop = onStorageRootChanged(() => {
      notified += 1;
    });

    window.dispatchEvent(new Event("focus"));
    document.dispatchEvent(new Event("visibilitychange"));
    await published();
    stop();

    expect(notified).toBe(1);
  });

  it("test_a_window_being_hidden_publishes_nothing", async () => {
    // Re-reading the root for a window nobody is looking at is work
    // with no observer, and `visibilitychange` fires on the way out as
    // well as on the way in.
    let notified = 0;
    const stop = onStorageRootChanged(() => {
      notified += 1;
    });

    hide("hidden");
    document.dispatchEvent(new Event("visibilitychange"));
    await published();
    stop();

    expect(notified).toBe(0);
  });

  it("test_every_subscriber_shares_one_pair_of_listeners", () => {
    // One watcher, however many views are mounted. A listener per view
    // would turn one return to the window into as many registrations,
    // and leave the "exactly one watcher" claim untrue the moment a
    // second library view existed.
    const onWindow = vi.spyOn(window, "addEventListener");
    const onDocument = vi.spyOn(document, "addEventListener");

    const first = onStorageRootChanged(() => undefined);
    const second = onStorageRootChanged(() => undefined);

    expect(onWindow.mock.calls.filter(([event]) => event === "focus")).toHaveLength(1);
    expect(
      onDocument.mock.calls.filter(([event]) => event === "visibilitychange"),
    ).toHaveLength(1);

    first();
    second();
  });

  it("test_the_last_subscriber_leaving_takes_the_listeners_with_it", async () => {
    let notified = 0;
    const stop = onStorageRootChanged(() => {
      notified += 1;
    });

    stop();
    window.dispatchEvent(new Event("focus"));
    await published();

    expect(notified).toBe(0);
  });
});

describe("query identities", () => {
  it("test_no_two_queries_are_given_the_same_name", () => {
    // The identifier is the only handle a view has on a query already
    // in flight, so a repeat would cancel the wrong one.
    const names = [nextRequestId(), nextRequestId(), nextRequestId()];

    expect(new Set(names).size).toBe(names.length);
  });
});
