import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it } from "vitest";

import { AppShell } from "./AppShell";

describe("AppShell", () => {
  it("test_shell_initial_render_shows_sessions_as_the_current_view", () => {
    render(<AppShell />);

    expect(
      screen.getByRole("button", { name: "Sessions", current: "page" }),
    ).toBeDefined();
    expect(screen.getByRole("heading", { name: "Sessions" })).toBeDefined();
  });

  it("test_shell_every_navigation_control_has_an_accessible_name", () => {
    render(<AppShell />);

    const names = screen
      .getAllByRole("button")
      .map((control) => control.textContent);

    expect(names).toEqual(["Sessions", "Search", "Settings"]);
  });

  it("test_shell_activating_search_replaces_the_main_region", async () => {
    const user = userEvent.setup();
    render(<AppShell />);

    await user.click(screen.getByRole("button", { name: "Search" }));

    expect(screen.getByRole("heading", { name: "Search" })).toBeDefined();
    expect(screen.queryByRole("heading", { name: "Sessions" })).toBeNull();
    expect(
      screen.getByRole("button", { name: "Search", current: "page" }),
    ).toBeDefined();
  });

  it("test_shell_navigation_is_reachable_and_operable_by_keyboard", async () => {
    const user = userEvent.setup();
    render(<AppShell />);

    await user.tab();
    await user.tab();
    await user.tab();
    await user.keyboard("{Enter}");

    expect(screen.getByRole("heading", { name: "Settings" })).toBeDefined();
  });

  it("test_shell_exposes_the_local_only_status_indicator", () => {
    render(<AppShell />);

    expect(
      screen.getByText("Local only — no account, no cloud"),
    ).toBeDefined();
  });
});
