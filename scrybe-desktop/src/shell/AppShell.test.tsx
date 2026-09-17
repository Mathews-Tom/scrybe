import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it } from "vitest";

import { AppShell } from "./AppShell";
import { ROUTES, defaultRoute } from "./routes";

describe("AppShell", () => {
  it("test_shell_opens_on_the_first_registered_route", () => {
    const first = defaultRoute();

    render(<AppShell />);

    expect(
      screen.getByRole("button", { name: first.label, current: "page" }),
    ).toBeDefined();
    expect(screen.getByRole("heading", { name: first.label })).toBeDefined();
  });

  it("test_shell_offers_one_named_navigation_control_per_registered_route", () => {
    render(<AppShell />);

    const names = screen
      .getAllByRole("button")
      .map((control) => control.textContent);

    expect(names).toEqual(ROUTES.map((route) => route.label));
  });

  it("test_shell_activating_any_route_replaces_the_main_region", async () => {
    const user = userEvent.setup();
    render(<AppShell />);

    for (const route of ROUTES) {
      await user.click(screen.getByRole("button", { name: route.label }));

      expect(screen.getByRole("heading", { name: route.label })).toBeDefined();
      expect(
        screen.getByRole("button", { name: route.label, current: "page" }),
      ).toBeDefined();
      expect(screen.getAllByRole("heading")).toHaveLength(1);
    }
  });

  it("test_shell_every_navigation_control_is_tab_reachable_in_sidebar_order", async () => {
    const user = userEvent.setup();
    render(<AppShell />);
    const controls = screen.getAllByRole("button");

    for (const control of controls) {
      await user.tab();
      expect(document.activeElement).toBe(control);
    }
    await user.keyboard("{Enter}");

    const last = ROUTES.at(-1)?.label ?? "";
    expect(screen.getByRole("heading", { name: last })).toBeDefined();
  });

  it("test_shell_exposes_the_local_only_status_indicator", () => {
    render(<AppShell />);

    expect(screen.getByText("Local only — no account, no cloud")).toBeDefined();
  });
});
