import { act, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";

import type { RecordingStatus, RecordingTransition } from "../generated/bindings";
import { commandFailure, idle, servicesReturning, settings } from "../testing/services";
import { renderWith } from "../testing/render";
import { AppShell } from "./AppShell";
import { ROUTES, defaultRoute } from "./routes";

const updater = vi.hoisted(() => ({
  check: vi.fn(),
  relaunch: vi.fn(),
}));

vi.mock("@tauri-apps/plugin-updater", () => ({ check: updater.check }));
vi.mock("@tauri-apps/plugin-process", () => ({ relaunch: updater.relaunch }));

describe("AppShell", () => {
  beforeEach(() => {
    updater.check.mockReset();
    updater.relaunch.mockReset();
  });

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

  it("test_shell_marks_settings_after_an_explicit_authenticated_update_check", async () => {
    const user = userEvent.setup();
    updater.check.mockResolvedValue({
      version: "2.3.0",
      download: vi.fn(),
      install: vi.fn(),
    });

    await renderWith(<AppShell />);

    expect(updater.check).not.toHaveBeenCalled();
    expect(screen.getByRole("button", { name: "Settings" })).toBeDefined();
    await user.click(screen.getByRole("button", { name: "Settings" }));
    await user.click(await screen.findByRole("button", { name: "Check for updates" }));

    expect(
      await screen.findByRole("button", { name: "Settings — Update available" }),
    ).toBeDefined();
    expect(
      screen
        .getAllByRole("navigation", { name: "Primary" })
        .flatMap((nav) => [...nav.querySelectorAll("button")]),
    ).toHaveLength(ROUTES.length);
  });

  it("test_shell_clears_the_settings_annotation_when_a_recheck_is_current_or_fails", async () => {
    const user = userEvent.setup();
    updater.check
      .mockResolvedValueOnce({
        version: "2.3.0",
        download: vi.fn(),
        install: vi.fn(),
      })
      .mockResolvedValueOnce(null)
      .mockRejectedValueOnce(new Error("release feed unavailable"));

    await renderWith(<AppShell />);
    await user.click(screen.getByRole("button", { name: "Settings" }));
    await user.click(await screen.findByRole("button", { name: "Check for updates" }));
    await screen.findByRole("button", { name: "Settings — Update available" });

    await user.click(screen.getByRole("button", { name: "Check for updates" }));
    await screen.findByText("This installation is current.");
    expect(screen.getByRole("button", { name: "Settings" })).toBeDefined();

    await user.click(screen.getByRole("button", { name: "Check for updates" }));
    await screen.findByText("The update check failed: release feed unavailable");
    expect(screen.getByRole("button", { name: "Settings" })).toBeDefined();
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

  /**
   * The regression this guards: `acknowledge_recording` used to be
   * called only from `RecordingView`, which is mounted by one route
   * among several. A recording driven from the tray, a hotkey, or the
   * signal bridge while the reader is on a different route — here,
   * the shell's own default, `Sessions` — has no `RecordingView`
   * mounted to acknowledge it, and `RecordingState::accepts_start`
   * would refuse every surface a second start until one did. This
   * test never renders `RecordingView` at all.
   */
  it("test_shell_acknowledges_a_terminal_recording_even_when_the_record_route_was_never_opened", async () => {
    let deliver: ((transition: RecordingTransition) => void) | undefined;
    const acknowledgeRecording = vi
      .fn<() => Promise<RecordingStatus>>()
      .mockResolvedValue(idle());
    const recordingStatus = vi
      .fn<() => Promise<RecordingStatus>>()
      .mockResolvedValue(idle({ state: "completed", elapsed_ms: 4_000 }));

    await renderWith(
      <AppShell />,
      servicesReturning({
        recordingStatus,
        acknowledgeRecording,
        onRecordingTransition: (onTransition) => {
          deliver = onTransition;
          return Promise.resolve(() => undefined);
        },
      }),
    );

    // Confirms the premise: the reader is on Sessions, not Record.
    expect(screen.getByRole("heading", { level: 1, name: "Sessions" })).toBeDefined();

    act(() => {
      deliver?.({
        schema_version: 1,
        sequence: 1,
        from: "saving",
        to: "completed",
        elapsed_ms: 4_000,
        failure_summary: null,
      });
    });

    await waitFor(() => {
      expect(acknowledgeRecording).toHaveBeenCalledTimes(1);
    });
  });
});
