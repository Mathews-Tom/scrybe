import { act, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it } from "vitest";

import type { SessionRows } from "../../generated/bindings";
import { renderWith } from "../../testing/render";
import {
  commandFailure,
  page,
  servicesReturning,
  session,
} from "../../testing/services";
import { settingsFormFixture } from "../../testing/setup";
import { SearchView } from "./SearchView";
import { SessionsView } from "./SessionsView";
import { SettingsView } from "./SettingsView";

/**
 * Lets everything the last interaction started run to completion.
 *
 * The storage-root revision is published on a microtask and read
 * through `useSyncExternalStore`, so a re-read is several turns away
 * from the event that asked for it rather than one.
 */
async function settle(): Promise<void> {
  await act(async () => {
    await new Promise<void>((resolve) => {
      setTimeout(() => {
        resolve();
      }, 0);
    });
  });
}

/** A promise a test resolves when it decides the answer has arrived. */
function deferred(): {
  promise: Promise<SessionRows>;
  settle: (rows: SessionRows) => void;
} {
  let settleWith: (rows: SessionRows) => void = () => undefined;
  const promise = new Promise<SessionRows>((resolve) => {
    settleWith = resolve;
  });
  return { promise, settle: settleWith };
}

describe("SessionsView", () => {
  it("test_sessions_lists_each_recorded_session_with_its_state", async () => {
    await renderWith(
      <SessionsView />,
      servicesReturning({
        listSessions: () =>
          Promise.resolve(
            page([
              session({ title: "Quarterly review" }),
              session({ id: "other", title: "Standup", progress: "repairable" }),
            ]),
          ),
      }),
    );

    const rows = screen.getAllByRole("listitem");

    expect(rows).toHaveLength(2);
    expect(rows[0]?.textContent).toContain("Quarterly review");
    expect(rows[1]?.textContent).toContain("Needs repair");
  });

  it("test_sessions_beyond_one_page_says_how_many_there_are_rather_than_showing_a_page", async () => {
    // The view asks for twenty and Rust answers with the total, so
    // twenty-one sessions used to render exactly like twenty. With no
    // paging control to go looking with, the count is the only thing
    // that separates a complete list from a truncated one.
    const rows = Array.from({ length: 20 }, (_unused, index) =>
      session({ id: `session-${index.toString()}`, title: `Session ${index.toString()}` }),
    );
    await renderWith(
      <SessionsView />,
      servicesReturning({
        listSessions: () => Promise.resolve(page(rows, { total: 21, has_more: true })),
      }),
    );

    expect(screen.getAllByRole("listitem")).toHaveLength(20);
    expect(screen.getByText("Showing the first 20 of 21.")).toBeDefined();
    expect(screen.getByRole("list").getAttribute("aria-describedby")).toBe(
      screen.getByText("Showing the first 20 of 21.").id,
    );
  });

  it("test_sessions_that_fit_in_one_page_say_nothing_about_a_count", async () => {
    // The other half of the rule: a complete list must not read as a
    // truncated one either.
    await renderWith(
      <SessionsView />,
      servicesReturning({
        listSessions: () => Promise.resolve(page([session({ title: "Quarterly review" })])),
      }),
    );

    expect(screen.queryByText(/Showing the first/)).toBeNull();
    expect(screen.getByRole("list").getAttribute("aria-describedby")).toBeNull();
  });

  it("test_sessions_with_nothing_recorded_says_so_rather_than_showing_a_blank_region", async () => {
    await renderWith(<SessionsView />);

    expect(
      screen.getByText("Nothing has been recorded into this storage root yet."),
    ).toBeDefined();
  });

  it("test_sessions_surfaces_an_unreadable_storage_root_as_an_alert", async () => {
    await renderWith(
      <SessionsView />,
      servicesReturning({
        listSessions: () =>
          commandFailure(
            "storage_root_missing",
            "the configured storage root does not exist",
          ),
      }),
    );

    expect(screen.getByRole("alert").textContent).toBe(
      "the configured storage root does not exist",
    );
    expect(screen.queryByRole("list")).toBeNull();
  });

  it("test_sessions_are_grouped_under_the_day_each_one_was_recorded", async () => {
    await renderWith(
      <SessionsView />,
      servicesReturning({
        listSessions: () =>
          Promise.resolve(
            page([
              session({
                id: "later",
                title: "Later meeting",
                started_at: "2026-04-29T14:30:00Z",
              }),
              session({
                id: "earlier",
                title: "Earlier meeting",
                started_at: "2026-04-28T09:00:00Z",
              }),
            ]),
          ),
      }),
    );

    const days = screen.getAllByRole("heading", { level: 2 });
    const [later, earlier] = screen.getAllByRole("list");
    if (later === undefined || earlier === undefined) {
      throw new Error("a grouped list renders one list per day");
    }

    expect(days).toHaveLength(2);
    expect(screen.getAllByRole("list")).toHaveLength(2);
    expect(within(later).getByText("Later meeting")).toBeDefined();
    expect(within(earlier).getByText("Earlier meeting")).toBeDefined();
  });

  it("test_a_session_that_never_wrote_a_start_time_is_filed_rather_than_dropped", async () => {
    // The sessions with no metadata are the ones a reader is most
    // likely to be looking for, so they cannot be the ones grouping
    // loses.
    await renderWith(
      <SessionsView />,
      servicesReturning({
        listSessions: () =>
          Promise.resolve(
            page([session({ id: "bare", title: null, started_at: null })]),
          ),
      }),
    );

    expect(screen.getByRole("heading", { level: 2 }).textContent).toBe("Undated");
    expect(screen.getByText("bare")).toBeDefined();
  });

  it("test_an_unfinished_and_a_repairable_session_are_told_apart_from_a_complete_one", async () => {
    await renderWith(
      <SessionsView />,
      servicesReturning({
        listSessions: () =>
          Promise.resolve(
            page([
              session({ id: "one", progress: "complete" }),
              session({ id: "two", progress: "unfinished" }),
              session({ id: "three", progress: "repairable" }),
            ]),
          ),
      }),
    );

    const rows = screen.getAllByRole("listitem");

    expect(rows.map((row) => row.getAttribute("data-progress"))).toEqual([
      "complete",
      "unfinished",
      "repairable",
    ]);
    expect(rows[1]?.textContent).toContain("Unfinished");
    expect(rows[2]?.textContent).toContain("Needs repair");
  });

  it("test_returning_to_the_window_re_reads_the_storage_root", async () => {
    // Ordinary files on disk are the source of truth and anything may
    // write to them, so a list that only ever read once would go stale
    // the first time a recording finished in another window.
    let reads = 0;
    await renderWith(
      <SessionsView />,
      servicesReturning({
        listSessions: () => {
          reads += 1;
          return Promise.resolve(page([session()]));
        },
      }),
    );
    expect(reads).toBe(1);

    window.dispatchEvent(new Event("focus"));
    await settle();

    expect(reads).toBe(2);
  });

});

describe("SearchView", () => {
  it("test_search_field_has_a_label_and_is_operable_by_keyboard", async () => {
    const user = userEvent.setup();
    await renderWith(
      <SearchView />,
      servicesReturning({
        searchSessions: (_requestId, query) =>
          Promise.resolve(page(query === "review" ? [session()] : [])),
      }),
    );

    await user.tab();
    await user.keyboard("review");
    await user.keyboard("{Enter}");

    expect(screen.getByRole("searchbox", { name: "Search transcripts and notes" })).toBe(
      document.activeElement,
    );
    expect(await screen.findByText("Quarterly review")).toBeDefined();
  });

  it("test_search_with_no_match_names_the_query_it_found_nothing_for", async () => {
    const user = userEvent.setup();
    await renderWith(<SearchView />);

    await user.type(
      screen.getByRole("searchbox", { name: "Search transcripts and notes" }),
      "budget",
    );
    await user.click(screen.getByRole("button", { name: "Search" }));

    expect(await screen.findByText("Nothing matches “budget”.")).toBeDefined();
  });

  it("test_a_second_search_abandons_the_first_one_by_name", async () => {
    // `CancellationToken` does not serialize and `invoke` has no abort,
    // so the only handle the frontend has on a search already running
    // is the identifier it minted for it.
    const user = userEvent.setup();
    const issued: string[] = [];
    const cancelled: string[] = [];
    const answers = [deferred(), deferred()];
    await renderWith(
      <SearchView />,
      servicesReturning({
        searchSessions: (requestId) => {
          issued.push(requestId);
          return answers[issued.length - 1]?.promise ?? Promise.resolve(page([]));
        },
        cancelQuery: (requestId) => {
          cancelled.push(requestId);
          return Promise.resolve(true);
        },
      }),
    );
    const field = screen.getByRole("searchbox", { name: "Search transcripts and notes" });

    await user.type(field, "first");
    await user.click(screen.getByRole("button", { name: "Search" }));
    await user.clear(field);
    await user.type(field, "second");
    await user.click(screen.getByRole("button", { name: "Search" }));

    expect(issued).toHaveLength(2);
    expect(cancelled).toEqual([issued[0]]);
  });

  it("test_a_slow_answer_cannot_replace_the_answer_to_a_newer_search", async () => {
    // The whole point of naming a query: a search abandoned mid-flight
    // may still finish, and the row it would render is the answer to a
    // question the reader has already moved on from.
    const user = userEvent.setup();
    const answers = [deferred(), deferred()];
    let issued = 0;
    await renderWith(
      <SearchView />,
      servicesReturning({
        searchSessions: () => {
          issued += 1;
          return answers[issued - 1]?.promise ?? Promise.resolve(page([]));
        },
      }),
    );
    const field = screen.getByRole("searchbox", { name: "Search transcripts and notes" });

    await user.type(field, "first");
    await user.click(screen.getByRole("button", { name: "Search" }));
    await user.clear(field);
    await user.type(field, "second");
    await user.click(screen.getByRole("button", { name: "Search" }));

    answers[1]?.settle(page([session({ id: "newer", title: "Newer answer" })]));
    await settle();
    answers[0]?.settle(page([session({ id: "older", title: "Older answer" })]));
    await settle();

    expect(screen.getByText("Newer answer")).toBeDefined();
    expect(screen.queryByText("Older answer")).toBeNull();
  });
});

// The settings surface is editable now, so most of what it does is
// asserted in `src/setup/settings.test.tsx` beside the panels that do
// it. What stays here is what every view in this file is checked for:
// that the view renders the state Rust returned rather than a
// placeholder.
describe("SettingsView", () => {
  it("test_settings_shows_where_storage_and_configuration_live", async () => {
    await renderWith(<SettingsView />);

    expect(screen.getByLabelText("Storage root")).toHaveProperty(
      "value",
      "/configured/sessions",
    );
    expect(screen.getByText(/\/configured\/config\.toml/)).toBeDefined();
  });

  it("test_settings_reports_the_service_layers_configuration_warnings", async () => {
    await renderWith(
      <SettingsView />,
      servicesReturning({
        settingsForm: () =>
          Promise.resolve(
            settingsFormFixture({
              warnings: [
                { severity: "warning", message: "the storage root does not exist" },
              ],
            }),
          ),
      }),
    );

    expect(
      screen.getByText("Warning: the storage root does not exist"),
    ).toBeDefined();
  });
});
