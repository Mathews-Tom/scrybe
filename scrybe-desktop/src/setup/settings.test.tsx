import { screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it } from "vitest";

import type { SettingsChange } from "../generated/bindings";
import { SettingsView } from "../shell/views/SettingsView";
import { renderWith } from "../testing/render";
import { commandFailure, servicesReturning } from "../testing/services";
import {
  diagnosticRow,
  diagnostics,
  modelOfferFixture,
  readiness,
  readinessFacet,
  settingsFormFixture,
} from "../testing/setup";

describe("the settings surface", () => {
  it("test_readiness_is_reported_alongside_the_fields_that_change_it", async () => {
    await renderWith(
      <SettingsView />,
      servicesReturning({
        readinessReport: () =>
          Promise.resolve(
            readiness({
              notes: readinessFacet({ state: "blocked", summary: "nothing is answering" }),
            }),
          ),
      }),
    );

    expect(await screen.findByText("Needs attention")).toBeDefined();
    expect(screen.getByText("nothing is answering")).toBeDefined();
  });

  it("test_saving_sends_only_the_fields_that_changed", async () => {
    const user = userEvent.setup();
    const written: SettingsChange[][] = [];
    await renderWith(
      <SettingsView />,
      servicesReturning({
        applySettings: (changes) => {
          written.push(changes);
          return Promise.resolve(settingsFormFixture());
        },
      }),
    );
    const root = await screen.findByLabelText("Storage root");

    await user.clear(root);
    await user.type(root, "/elsewhere/sessions");
    await user.click(screen.getByRole("button", { name: "Save changes" }));

    expect(written).toEqual([[{ field: "storage_root", value: "/elsewhere/sessions" }]]);
  });

  it("test_the_indicator_list_is_written_as_a_list_rather_than_as_one_string", async () => {
    const user = userEvent.setup();
    const written: SettingsChange[][] = [];
    await renderWith(
      <SettingsView />,
      servicesReturning({
        applySettings: (changes) => {
          written.push(changes);
          return Promise.resolve(settingsFormFixture());
        },
      }),
    );
    const indicators = await screen.findByLabelText("Recording indicators");

    await user.clear(indicators);
    await user.type(indicators, "menu-bar-label\nfloating-window");
    await user.click(screen.getByRole("button", { name: "Save changes" }));

    expect(written).toEqual([
      [{ field: "shell_indicators", value: ["menu-bar-label", "floating-window"] }],
    ]);
  });

  it("test_nothing_is_written_until_something_changes", async () => {
    const written: unknown[] = [];
    await renderWith(
      <SettingsView />,
      servicesReturning({
        applySettings: (changes) => {
          written.push(changes);
          return Promise.resolve(settingsFormFixture());
        },
      }),
    );

    const save = await screen.findByRole("button", { name: "Save changes" });

    expect(save.hasAttribute("disabled")).toBe(true);
    expect(written).toEqual([]);
  });

  it("test_a_rejected_change_is_reported_and_the_previous_values_stay_on_screen", async () => {
    const user = userEvent.setup();
    await renderWith(
      <SettingsView />,
      servicesReturning({
        applySettings: () =>
          commandFailure("config_invalid", "the updated configuration is not valid"),
      }),
    );
    const root = await screen.findByLabelText("Storage root");

    await user.clear(root);
    await user.type(root, "/elsewhere");
    await user.click(screen.getByRole("button", { name: "Save changes" }));

    expect(await screen.findByRole("alert")).toBeDefined();
    expect(screen.getByText("the updated configuration is not valid")).toBeDefined();
    expect(screen.getByLabelText("Storage root")).toHaveProperty("value", "/elsewhere");
  });

  it("test_the_surface_says_that_advanced_configuration_survives_a_save", async () => {
    await renderWith(<SettingsView />);

    expect(
      await screen.findByText(/Comments, ordering, and any advanced block/),
    ).toBeDefined();
  });

  it("test_advanced_configuration_opens_through_the_service_rather_than_a_path", async () => {
    const user = userEvent.setup();
    let opened = 0;
    await renderWith(
      <SettingsView />,
      servicesReturning({
        openAdvancedConfiguration: () => {
          opened += 1;
          return Promise.resolve();
        },
      }),
    );

    await user.click(
      await screen.findByRole("button", { name: "Open advanced configuration" }),
    );

    expect(opened).toBe(1);
  });

  it("test_the_surface_collects_no_credential", async () => {
    const { container } = await renderWith(
      <SettingsView />,
      servicesReturning({
        settingsForm: () =>
          Promise.resolve(settingsFormFixture({ hosted_credential_required: true })),
      }),
    );
    await screen.findByLabelText("Storage root");

    // No field that could hold a secret, and none that names one.
    // `hotkey` is a keyboard shortcut and is deliberately not caught by
    // the pattern; a field named `api_key` would be.
    expect(container.querySelector("input[type=password]")).toBeNull();
    for (const input of container.querySelectorAll("input,textarea")) {
      expect(input.id).not.toMatch(/api[_-]?key|secret|token|password|credential/i);
    }
  });

  it("test_model_storage_is_shown_with_the_facts_a_download_would_need", async () => {
    await renderWith(<SettingsView />);

    expect(
      await screen.findByRole("heading", { level: 2, name: "Managed model storage" }),
    ).toBeDefined();
    expect(screen.getByText(modelOfferFixture().destination_path)).toBeDefined();
    expect(screen.getByText(modelOfferFixture().sha256)).toBeDefined();
  });
});

describe("the diagnostics surface", () => {
  it("test_opening_it_reads_and_applies_nothing", async () => {
    const calls: string[] = [];
    await renderWith(
      <SettingsView />,
      servicesReturning({
        diagnosticsReport: () => {
          calls.push("read");
          return Promise.resolve(
            diagnostics([
              diagnosticRow({
                code: "session_lock_stale",
                severity: "warning",
                component: "session",
                summary: "session 2026-04-29-1430-acme-01HXYZ holds a lock whose process is gone",
                recovery_action: {
                  action: "remove_stale_session_lock",
                  id: "2026-04-29-1430-acme-01HXYZ",
                },
                mutation_required: true,
              }),
            ]),
          );
        },
        applyRecovery: () => {
          calls.push("apply");
          return Promise.resolve({ applied: true, summary: "removed" });
        },
      }),
    );
    await screen.findByText(/holds a lock whose process is gone/);

    expect(calls).toEqual(["read"]);
  });

  it("test_a_repair_runs_only_when_its_button_is_pressed", async () => {
    const user = userEvent.setup();
    const applied: unknown[] = [];
    await renderWith(
      <SettingsView />,
      servicesReturning({
        diagnosticsReport: () =>
          Promise.resolve(
            diagnostics([
              diagnosticRow({
                code: "storage_root_absent",
                severity: "info",
                component: "storage",
                summary: "storage root /configured/sessions does not exist yet",
                recovery_action: { action: "create_storage_root" },
                mutation_required: true,
              }),
            ]),
          ),
        applyRecovery: (action) => {
          applied.push(action);
          return Promise.resolve({ applied: true, summary: "created the storage root" });
        },
      }),
    );
    const repair = await screen.findByRole("button", { name: "Create the storage root" });

    expect(applied).toEqual([]);

    await user.click(repair);

    await waitFor(() => {
      expect(applied).toEqual([{ action: "create_storage_root" }]);
    });
    expect(await screen.findByText("created the storage root")).toBeDefined();
  });

  it("test_a_finding_with_no_recovery_offers_no_button", async () => {
    await renderWith(
      <SettingsView />,
      servicesReturning({
        diagnosticsReport: () =>
          Promise.resolve(
            diagnostics([
              diagnosticRow({
                code: "llm_egress_local",
                severity: "info",
                component: "egress",
                summary: "no egress (local LLM at http://127.0.0.1:11434/v1)",
                recovery_action: null,
              }),
            ]),
          ),
      }),
    );
    await screen.findByText(/no egress/);

    const row = screen.getByText(/no egress/).closest("li");

    expect(row?.querySelector("button")).toBeNull();
  });

  it("test_installing_a_model_is_routed_to_the_confirmation_flow_rather_than_repaired", async () => {
    const applied: unknown[] = [];
    await renderWith(
      <SettingsView />,
      servicesReturning({
        diagnosticsReport: () =>
          Promise.resolve(
            diagnostics([
              diagnosticRow({
                code: "transcription_model_absent",
                severity: "error",
                component: "providers",
                summary: "local transcription is configured but no model is installed",
                recovery_action: {
                  action: "install_transcription_model",
                  id: "whisper-small-en",
                },
                mutation_required: true,
              }),
            ]),
          ),
        applyRecovery: (action) => {
          applied.push(action);
          return Promise.resolve({ applied: true, summary: "" });
        },
      }),
    );
    await screen.findByText(/no model is installed/);

    const row = screen.getByText(/no model is installed/).closest("li");

    // A recovery this application performs elsewhere is a pointer to
    // where, not a button that would reach a repair call carrying no
    // confirmation.
    expect(row?.querySelector("button")).toBeNull();
    expect(screen.getByText("Install it from Setup")).toBeDefined();
    expect(applied).toEqual([]);
  });

  it("test_a_severity_is_carried_by_text_rather_than_by_colour_alone", async () => {
    await renderWith(
      <SettingsView />,
      servicesReturning({
        diagnosticsReport: () =>
          Promise.resolve(
            diagnostics([
              diagnosticRow({ severity: "info", summary: "all well" }),
              diagnosticRow({ severity: "warning", summary: "something is odd" }),
              diagnosticRow({ severity: "error", summary: "something is broken" }),
            ]),
          ),
      }),
    );

    expect(await screen.findByText("Information")).toBeDefined();
    expect(screen.getByText("Warning")).toBeDefined();
    expect(screen.getByText("Problem")).toBeDefined();
  });
});
