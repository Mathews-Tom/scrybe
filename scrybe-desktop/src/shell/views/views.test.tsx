import { screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it } from "vitest";

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
});

describe("SearchView", () => {
  it("test_search_field_has_a_label_and_is_operable_by_keyboard", async () => {
    const user = userEvent.setup();
    await renderWith(
      <SearchView />,
      servicesReturning({
        searchSessions: (query) =>
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
