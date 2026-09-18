import { screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";

import { renderWith } from "../../testing/render";
import { idle, servicesReturning } from "../../testing/services";
import { UpdatePanel } from "./UpdatePanel";

const plugins = vi.hoisted(() => ({
  check: vi.fn(),
  relaunch: vi.fn(),
}));

vi.mock("@tauri-apps/plugin-updater", () => ({ check: plugins.check }));
vi.mock("@tauri-apps/plugin-process", () => ({ relaunch: plugins.relaunch }));

describe("UpdatePanel", () => {
  beforeEach(() => {
    plugins.check.mockReset();
    plugins.relaunch.mockReset();
  });

  it("defers installation while a recording is active", async () => {
    const user = userEvent.setup();
    const download = vi.fn().mockResolvedValue(undefined);
    const install = vi.fn().mockResolvedValue(undefined);
    plugins.check.mockResolvedValue({ version: "2.1.0", download, install });
    await renderWith(
      <UpdatePanel />,
      servicesReturning({
        recordingStatus: () => Promise.resolve(idle({ state: "recording" })),
      }),
    );

    await user.click(screen.getByRole("button", { name: "Check for updates" }));
    expect(await screen.findByText("Scrybe 2.1.0 is available.")).toBeDefined();
    await user.click(screen.getByRole("button", { name: "Download update" }));
    expect(await screen.findByRole("button", { name: "Install and relaunch" })).toBeDefined();
    await user.click(screen.getByRole("button", { name: "Install and relaunch" }));

    expect(
      await screen.findByText("Stop the active recording before installing and relaunching Scrybe."),
    ).toBeDefined();
    expect(download).toHaveBeenCalledOnce();
    expect(install).not.toHaveBeenCalled();
    expect(plugins.relaunch).not.toHaveBeenCalled();
  });

  it("installs a downloaded update and relaunches when idle", async () => {
    const user = userEvent.setup();
    const install = vi.fn().mockResolvedValue(undefined);
    plugins.check.mockResolvedValue({
      version: "2.1.0",
      download: vi.fn().mockResolvedValue(undefined),
      install,
    });
    await renderWith(<UpdatePanel />, servicesReturning());

    await user.click(screen.getByRole("button", { name: "Check for updates" }));
    await user.click(await screen.findByRole("button", { name: "Download update" }));
    await user.click(await screen.findByRole("button", { name: "Install and relaunch" }));

    expect(install).toHaveBeenCalledOnce();
    expect(plugins.relaunch).toHaveBeenCalledOnce();
  });
});
