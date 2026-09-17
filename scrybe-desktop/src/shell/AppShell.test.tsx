import { screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it } from "vitest";

import { commandFailure, servicesReturning, settings } from "../testing/services";
import { renderWith } from "../testing/render";
import { AppShell } from "./AppShell";
import { ROUTES, defaultRoute } from "./routes";

describe("AppShell", () => {
  it("test_shell_opens_on_the_first_registered_route", async () => {
    const first = defaultRoute();

    await renderWith(<AppShell />);

    expect(
      screen.getByRole("button", { name: first.label, current: "page" }),
    ).toBeDefined();
    expect(screen.getByRole("heading", { level: 1, name: first.label })).toBeDefined();
  });

  it("test_shell_offers_one_named_navigation_control_per_registered_route", async () => {
    await renderWith(<AppShell />);

    const names = screen
      .getAllByRole("navigation", { name: "Primary" })
      .flatMap((nav) => [...nav.querySelectorAll("button")])
      .map((control) => control.textContent);

    expect(names).toEqual(ROUTES.map((route) => route.label));
  });

  it("test_shell_activating_any_route_replaces_the_main_region", async () => {
    const user = userEvent.setup();
    await renderWith(<AppShell />);

    for (const route of ROUTES) {
      await user.click(screen.getByRole("button", { name: route.label }));

      expect(
        screen.getByRole("heading", { level: 1, name: route.label }),
      ).toBeDefined();
      expect(screen.getAllByRole("heading", { level: 1 })).toHaveLength(1);
    }
  });

  it("test_shell_every_navigation_control_is_tab_reachable_in_sidebar_order", async () => {
    const user = userEvent.setup();
    await renderWith(<AppShell />);
    const controls = ROUTES.map((route) =>
      screen.getByRole("button", { name: route.label }),
    );

    for (const control of controls) {
      await user.tab();
      expect(document.activeElement).toBe(control);
    }
  });

  it("test_shell_reaches_the_last_route_by_keyboard_alone", async () => {
    const user = userEvent.setup();
    await renderWith(<AppShell />);
    const last = ROUTES.at(-1)?.label ?? "";

    for (const route of ROUTES) {
      await user.tab();
      expect(screen.getByRole("button", { name: route.label })).toBe(
        document.activeElement,
      );
    }
    await user.keyboard("{Enter}");

    expect(screen.getByRole("heading", { level: 1, name: last })).toBeDefined();
  });

  it("test_shell_reports_local_only_when_no_hosted_provider_is_configured", async () => {
    await renderWith(<AppShell />);

    expect(
      await screen.findByText("Local only — no account, no cloud"),
    ).toBeDefined();
  });

  it("test_shell_reports_a_hosted_provider_when_one_is_configured", async () => {
    await renderWith(
      <AppShell />,
      servicesReturning({
        settingsSummary: () =>
          Promise.resolve(settings({ hosted_credential_required: true })),
      }),
    );

    expect(await screen.findByText("A hosted provider is configured")).toBeDefined();
  });

  it("test_shell_status_survives_a_configuration_it_cannot_read", async () => {
    await renderWith(
      <AppShell />,
      servicesReturning({
        settingsSummary: () =>
          commandFailure("config_unreadable", "not valid TOML"),
      }),
    );

    await waitFor(() => {
      expect(screen.getByText("Provider configuration unavailable")).toBeDefined();
    });
  });

  it("test_shell_status_indicator_is_text_not_colour_alone", async () => {
    const { container } = await renderWith(<AppShell />);
    await screen.findByText("Local only — no account, no cloud");

    const dot = container.querySelector(".app-shell__status-dot");

    expect(dot?.getAttribute("aria-hidden")).toBe("true");
  });
});
