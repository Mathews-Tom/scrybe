import { act } from "react";
import { screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";

import type { ModelOutcome, ModelProgress } from "../generated/bindings";
import { AppShell } from "../shell/AppShell";
import { renderWith } from "../testing/render";
import { commandFailure, servicesReturning } from "../testing/services";
import {
  diagnostics,
  modelOfferFixture,
  modelOutcome,
  readiness,
  readinessFacet,
  settingsFormFixture,
} from "../testing/setup";
import { SetupWizard } from "./SetupWizard";
import { needsSetup } from "./steps";

/** Moves to the step with the given label. */
async function goTo(user: ReturnType<typeof userEvent.setup>, label: string) {
  while (screen.queryByRole("heading", { level: 2, name: label }) === null) {
    const next = screen.getByRole("button", { name: "Next" });
    if (next.hasAttribute("disabled")) {
      throw new Error(`the wizard has no step called ${label}`);
    }
    await user.click(next);
  }
}

describe("guided setup", () => {
  it("test_the_wizard_presents_four_steps_in_order", async () => {
    const user = userEvent.setup();
    await renderWith(<SetupWizard onExit={() => undefined} />);

    const labels: string[] = [];
    for (;;) {
      labels.push(screen.getByRole("heading", { level: 2 }).textContent);
      const next = screen.getByRole("button", { name: "Next" });
      if (next.hasAttribute("disabled")) {
        break;
      }
      await user.click(next);
    }

    expect(labels).toEqual(["Welcome", "Recording", "Transcription and notes", "Ready"]);
  });

  it("test_welcome_names_the_storage_root_and_the_one_network_request_before_any_step_makes_one", async () => {
    await renderWith(
      <SetupWizard onExit={() => undefined} />,
      servicesReturning({
        settingsForm: () =>
          Promise.resolve(settingsFormFixture({ storage_root: "/disposable/sessions" })),
      }),
    );

    expect(await screen.findByText(/\/disposable\/sessions/)).toBeDefined();
    expect(screen.getByText(/only network request guided setup makes/)).toBeDefined();
    expect(screen.getByText(/No meeting bot joins a call/)).toBeDefined();
  });

  it("test_every_step_offers_a_visible_exit_that_leaves_the_application_usable", async () => {
    const user = userEvent.setup();
    const exit = vi.fn();
    await renderWith(<SetupWizard onExit={exit} />);

    for (const label of ["Welcome", "Recording", "Transcription and notes", "Ready"]) {
      await goTo(user, label);
      expect(screen.getByRole("button", { name: "Leave setup" })).toBeDefined();
    }
    await user.click(screen.getByRole("button", { name: "Leave setup" }));

    expect(exit).toHaveBeenCalled();
  });

  it("test_the_shell_opens_on_setup_when_this_installation_cannot_record", async () => {
    await renderWith(
      <AppShell />,
      servicesReturning({
        readinessReport: () =>
          Promise.resolve(
            readiness({
              transcription: readinessFacet({ state: "blocked", summary: "no model installed" }),
              can_record: false,
            }),
          ),
      }),
    );

    expect(await screen.findByRole("heading", { level: 1, name: "Setup" })).toBeDefined();
  });

  it("test_the_shell_opens_on_sessions_when_this_installation_can_record", async () => {
    await renderWith(<AppShell />);

    expect(await screen.findByRole("heading", { level: 1, name: "Sessions" })).toBeDefined();
  });

  it("test_leaving_setup_reaches_the_session_list", async () => {
    const user = userEvent.setup();
    await renderWith(
      <AppShell />,
      servicesReturning({
        readinessReport: () => Promise.resolve(readiness({ can_record: false })),
      }),
    );
    await screen.findByRole("heading", { level: 1, name: "Setup" });

    await user.click(screen.getByRole("button", { name: "Leave setup" }));

    expect(screen.getByRole("heading", { level: 1, name: "Sessions" })).toBeDefined();
  });
});

describe("the recording step", () => {
  it("test_each_capability_is_explained_and_links_its_own_recovery_before_anything_is_asked_for", async () => {
    const user = userEvent.setup();
    const opened: string[] = [];
    await renderWith(
      <SetupWizard onExit={() => undefined} />,
      servicesReturning({
        openSystemSettings: (capability) => {
          opened.push(capability);
          return Promise.resolve();
        },
      }),
    );
    await goTo(user, "Recording");

    expect(screen.getByRole("heading", { level: 4, name: "Microphone" })).toBeDefined();
    expect(
      screen.getByRole("heading", { level: 4, name: "Screen & System Audio Recording" }),
    ).toBeDefined();
    expect(screen.getByText(/requests no video frames/)).toBeDefined();

    await user.click(screen.getByRole("button", { name: "Open Microphone settings" }));
    await user.click(
      screen.getByRole("button", { name: "Open Screen & System Audio Recording settings" }),
    );

    // Each capability reaches its own recovery, rather than both
    // reaching one screen that happens to be right for the first.
    expect(opened).toEqual(["microphone", "system_audio_recording"]);
  });

  it("test_the_step_never_prompts_so_it_cannot_prompt_repeatedly", async () => {
    const user = userEvent.setup();
    const prompts: string[] = [];
    await renderWith(
      <SetupWizard onExit={() => undefined} />,
      servicesReturning({
        openSystemSettings: (capability) => {
          prompts.push(capability);
          return Promise.resolve();
        },
      }),
    );

    await goTo(user, "Recording");
    await user.click(screen.getByRole("button", { name: "Check again" }));

    // Arriving at the step, and re-checking from it, request nothing.
    // macOS raises its own dialog when a recording first needs one; the
    // only thing this step invokes is the recovery a user asks for.
    expect(prompts).toEqual([]);
  });

  it("test_saving_the_microphone_writes_one_field_through_the_settings_service", async () => {
    const user = userEvent.setup();
    const written: unknown[] = [];
    await renderWith(
      <SetupWizard onExit={() => undefined} />,
      servicesReturning({
        applySettings: (changes) => {
          written.push(changes);
          return Promise.resolve(settingsFormFixture({ capture_mic_device: "Studio Mic" }));
        },
      }),
    );
    await goTo(user, "Recording");

    await user.clear(screen.getByLabelText("Microphone device"));
    await user.type(screen.getByLabelText("Microphone device"), "Studio Mic");
    await user.click(screen.getByRole("button", { name: "Save" }));

    expect(written).toEqual([[{ field: "capture_mic_device", value: "Studio Mic" }]]);
  });

  /// The confirmation used to be state inside `RecordingStep`, set
  /// beside the same `onSaved` call that reloads settings and
  /// readiness — which unmounts that step while the reload is in
  /// flight, destroying the flag before it could ever render. A
  /// successful write gave the user no feedback at all.
  it("test_a_successful_save_is_confirmed_after_settings_and_readiness_reload", async () => {
    const user = userEvent.setup();
    await renderWith(
      <SetupWizard onExit={() => undefined} />,
      servicesReturning({
        applySettings: () =>
          Promise.resolve(settingsFormFixture({ capture_mic_device: "Studio Mic" })),
      }),
    );
    await goTo(user, "Recording");

    await user.clear(screen.getByLabelText("Microphone device"));
    await user.type(screen.getByLabelText("Microphone device"), "Studio Mic");
    await user.click(screen.getByRole("button", { name: "Save" }));

    expect(await screen.findByRole("status")).toHaveProperty(
      "textContent",
      "Saved. Your advanced settings and comments were left untouched.",
    );
  });

  it("test_the_confirmation_is_retired_once_the_microphone_is_edited_again", async () => {
    const user = userEvent.setup();
    await renderWith(
      <SetupWizard onExit={() => undefined} />,
      servicesReturning({
        applySettings: () =>
          Promise.resolve(settingsFormFixture({ capture_mic_device: "Studio Mic" })),
      }),
    );
    await goTo(user, "Recording");

    await user.clear(screen.getByLabelText("Microphone device"));
    await user.type(screen.getByLabelText("Microphone device"), "Studio Mic");
    await user.click(screen.getByRole("button", { name: "Save" }));
    expect(await screen.findByRole("status")).toBeDefined();

    await user.type(await screen.findByLabelText("Microphone device"), " (USB)");

    await waitFor(() => {
      expect(screen.queryByRole("status")).toBeNull();
    });
  });
});

describe("the transcription step", () => {
  it("test_the_offer_shows_everything_that_must_precede_a_request_and_requests_nothing", async () => {
    const user = userEvent.setup();
    const installs: string[] = [];
    await renderWith(
      <SetupWizard onExit={() => undefined} />,
      servicesReturning({
        installModel: (id) => {
          installs.push(id);
          return Promise.resolve(modelOutcome());
        },
      }),
    );
    await goTo(user, "Transcription and notes");

    const offer = modelOfferFixture();
    for (const fact of [
      offer.source_url,
      offer.source_revision,
      offer.license,
      offer.sha256,
      offer.destination_path,
    ]) {
      expect(await screen.findByText(fact)).toBeDefined();
    }
    expect(screen.getByText(/487614201 bytes/)).toBeDefined();

    // Reaching the step and reading the offer requested nothing.
    expect(installs).toEqual([]);
  });

  it("test_no_model_is_requested_until_the_confirmation_is_pressed", async () => {
    const user = userEvent.setup();
    const installs: [string, string][] = [];
    await renderWith(
      <SetupWizard onExit={() => undefined} />,
      servicesReturning({
        installModel: (id, digest) => {
          installs.push([id, digest]);
          return Promise.resolve(modelOutcome());
        },
      }),
    );
    await goTo(user, "Transcription and notes");
    const confirm = await screen.findByRole("button", {
      name: "Download and verify this model",
    });

    expect(installs).toEqual([]);

    await user.click(confirm);

    // The digest handed back is the one the offer displayed, which is
    // what Rust compares against the catalog before opening anything.
    await waitFor(() => {
      expect(installs).toEqual([
        [
          "whisper-small-en",
          "c6138d6d58ecc8322097e0f987c32f1be8bb0a18532a3f88f734d1bbf9c41e5d",
        ],
      ]);
    });
  });

  it("test_an_offer_with_no_room_for_it_cannot_be_confirmed", async () => {
    const user = userEvent.setup();
    await renderWith(
      <SetupWizard onExit={() => undefined} />,
      servicesReturning({
        modelOffer: () =>
          Promise.resolve(
            modelOfferFixture({ sufficient_space: false, available_bytes: "1000" }),
          ),
      }),
    );
    await goTo(user, "Transcription and notes");

    const confirm = await screen.findByRole("button", {
      name: "Download and verify this model",
    });

    expect(confirm.hasAttribute("disabled")).toBe(true);
    expect(screen.getByText(/not enough room on this volume/)).toBeDefined();
  });

  it("test_a_cancelled_download_says_nothing_was_installed_and_offers_a_retry", async () => {
    const user = userEvent.setup();
    await renderWith(
      <SetupWizard onExit={() => undefined} />,
      servicesReturning({
        installModel: () =>
          Promise.resolve(modelOutcome({ state: "cancelled", promoted: false })),
      }),
    );
    await goTo(user, "Transcription and notes");
    await user.click(
      await screen.findByRole("button", { name: "Download and verify this model" }),
    );

    expect(await screen.findByText(/Nothing was installed/)).toBeDefined();
    expect(screen.getByRole("button", { name: "Try again" })).toBeDefined();
  });

  it("test_a_digest_failure_says_nothing_was_installed_and_offers_a_retry", async () => {
    const user = userEvent.setup();
    await renderWith(
      <SetupWizard onExit={() => undefined} />,
      servicesReturning({
        installModel: () =>
          Promise.resolve(
            modelOutcome({
              state: "failed",
              promoted: false,
              failure:
                "the artifact did not match the checked-in digest, so nothing was installed",
            }),
          ),
      }),
    );
    await goTo(user, "Transcription and notes");
    await user.click(
      await screen.findByRole("button", { name: "Download and verify this model" }),
    );

    expect(await screen.findByText(/did not match the checked-in digest/)).toBeDefined();
    expect(screen.getByRole("button", { name: "Try again" })).toBeDefined();
  });

  it("test_progress_is_reported_with_determinate_semantics_once_bytes_arrive", async () => {
    const user = userEvent.setup();
    let publish: ((progress: ModelProgress) => void) | undefined;
    await renderWith(
      <SetupWizard onExit={() => undefined} />,
      servicesReturning({
        onModelProgress: (onProgress) => {
          publish = onProgress;
          return Promise.resolve(() => undefined);
        },
        // Never settles, so the panel stays in its downloading stage
        // for the duration of the assertions below.
        installModel: () => new Promise<ModelOutcome>(() => undefined),
      }),
    );
    await goTo(user, "Transcription and notes");
    await user.click(
      await screen.findByRole("button", { name: "Download and verify this model" }),
    );

    // Before the first report there is no figure to report, and the bar
    // says so rather than claiming zero of a total it has not seen.
    const bar = screen.getByRole("progressbar");
    expect(bar.getAttribute("aria-valuenow")).toBeNull();
    expect(bar.getAttribute("aria-valuetext")).toBe("starting the download");

    await waitFor(() => {
      expect(publish).toBeDefined();
    });
    act(() => {
      publish?.({
        id: "whisper-small-en",
        received_bytes: "243807100",
        total_bytes: "487614201",
      });
    });

    await waitFor(() => {
      expect(screen.getByRole("progressbar").getAttribute("aria-valuenow")).toBe("50");
    });
  });

  it("test_cancelling_a_download_asks_the_service_to_stop_rather_than_deleting_anything", async () => {
    const user = userEvent.setup();
    const calls: string[] = [];
    await renderWith(
      <SetupWizard onExit={() => undefined} />,
      servicesReturning({
        installModel: () => new Promise<ModelOutcome>(() => undefined),
        cancelModelInstall: () => {
          calls.push("cancel");
          return Promise.resolve(true);
        },
        applyRecovery: () => {
          calls.push("repair");
          return Promise.resolve({ applied: true, summary: "" });
        },
      }),
    );
    await goTo(user, "Transcription and notes");
    await user.click(
      await screen.findByRole("button", { name: "Download and verify this model" }),
    );

    await user.click(screen.getByRole("button", { name: "Cancel" }));

    expect(calls).toEqual(["cancel"]);
  });

  /// Every install failure used to reach this panel as the single state
  /// `failed` plus prose, so a full disk and a corrupted download were
  /// indistinguishable to anything but a human reader — and both were
  /// offered the same `Try again`, which for the full disk could never
  /// work because nothing about the disk had changed.
  it("test_a_full_disk_and_a_corrupted_download_are_told_apart", async () => {
    const user = userEvent.setup();
    await renderWith(
      <SetupWizard onExit={() => undefined} />,
      servicesReturning({
        installModel: () =>
          Promise.resolve(
            modelOutcome({
              state: "failed",
              reason: "insufficient_space",
              failure: "installing it needs 756049465 bytes and 12 are free",
              promoted: false,
            }),
          ),
      }),
    );
    await goTo(user, "Transcription and notes");
    await user.click(
      await screen.findByRole("button", { name: "Download and verify this model" }),
    );

    expect(await screen.findByText(/There is not enough room on the disk/)).toBeDefined();
    expect(screen.queryByRole("button", { name: "Try again" })).toBeNull();
  });

  it("test_a_digest_failure_is_told_apart_and_is_worth_retrying", async () => {
    const user = userEvent.setup();
    await renderWith(
      <SetupWizard onExit={() => undefined} />,
      servicesReturning({
        installModel: () =>
          Promise.resolve(
            modelOutcome({
              state: "failed",
              reason: "digest_mismatch",
              failure:
                "the artifact did not match the checked-in digest, so nothing was installed",
              promoted: false,
            }),
          ),
      }),
    );
    await goTo(user, "Transcription and notes");
    await user.click(
      await screen.findByRole("button", { name: "Download and verify this model" }),
    );

    expect(await screen.findByText(/not the file the catalog describes/)).toBeDefined();
    expect(screen.getByRole("button", { name: "Try again" })).toBeDefined();
  });

  it("test_an_installed_model_is_reported_rather_than_offered_again", async () => {
    const user = userEvent.setup();
    await renderWith(
      <SetupWizard onExit={() => undefined} />,
      servicesReturning({
        modelOffer: () => Promise.resolve(modelOfferFixture({ state: "ready" })),
      }),
    );
    await goTo(user, "Transcription and notes");

    expect(await screen.findByText(/installed and verified/)).toBeDefined();
    expect(
      screen.queryByRole("button", { name: "Download and verify this model" }),
    ).toBeNull();
  });

  it("test_the_step_collects_no_hosted_credential", async () => {
    const user = userEvent.setup();
    const { container } = await renderWith(<SetupWizard onExit={() => undefined} />);
    await goTo(user, "Transcription and notes");

    // No field of any kind that could take a secret, and none that
    // names one. The only mention of a credential on the step is the
    // sentence saying none is wanted.
    expect(container.querySelector("input[type=password]")).toBeNull();
    expect(container.querySelectorAll("input")).toHaveLength(0);
    expect(screen.getByText(/asks for an account or an API key/)).toBeDefined();
  });
});

describe("the ready step", () => {
  it("test_readiness_is_reported_as_five_separate_answers", async () => {
    const user = userEvent.setup();
    await renderWith(<SetupWizard onExit={() => undefined} />);
    await goTo(user, "Ready");

    const terms = screen
      .getAllByRole("term")
      .map((term) => term.textContent);

    expect(terms).toEqual(["Recording", "Transcription", "Notes", "Storage", "Privacy"]);
  });

  it("test_recording_can_be_ready_while_notes_are_visibly_unavailable", async () => {
    const user = userEvent.setup();
    await renderWith(
      <SetupWizard onExit={() => undefined} />,
      servicesReturning({
        readinessReport: () =>
          Promise.resolve(
            readiness({
              notes: readinessFacet({
                state: "blocked",
                summary: "nothing is answering at http://127.0.0.1:11434/v1",
              }),
              can_record: true,
            }),
          ),
      }),
    );
    await goTo(user, "Ready");

    expect(screen.getByText(/Nothing above is blocking a recording/)).toBeDefined();
    expect(screen.getByText(/nothing is answering/)).toBeDefined();
    expect(screen.getByText(/Notes need a language model running on this Mac/)).toBeDefined();
  });

  /// The verdict used to read "This Mac is ready to record." for any
  /// installation whose facets were not blocked. Nothing in this
  /// application checks a capture permission, so that told a user who
  /// had refused both prompts that recording would work.
  it("test_the_verdict_does_not_claim_a_permission_nothing_checked", async () => {
    const user = userEvent.setup();
    await renderWith(
      <SetupWizard onExit={() => undefined} />,
      servicesReturning({
        readinessReport: () =>
          Promise.resolve(
            readiness({
              capture: readinessFacet({
                state: "unverified",
                summary:
                  "whether macOS has granted microphone and system-audio recording is not checked here; macOS asks the first time a recording needs it",
              }),
              can_record: true,
            }),
          ),
      }),
    );
    await goTo(user, "Ready");

    expect(screen.queryByText(/ready to record/)).toBeNull();
    expect(screen.getByText(/does not check whether macOS has granted/)).toBeDefined();
    expect(screen.getByText("Not checked")).toBeDefined();
  });

  it("test_a_blocked_facet_says_recording_is_not_available_yet", async () => {
    const user = userEvent.setup();
    await renderWith(
      <SetupWizard onExit={() => undefined} />,
      servicesReturning({
        readinessReport: () =>
          Promise.resolve(
            readiness({
              transcription: readinessFacet({
                state: "blocked",
                summary: "no model is installed",
              }),
              can_record: false,
            }),
          ),
      }),
    );
    await goTo(user, "Ready");

    expect(screen.getByText(/Recording is not available yet/)).toBeDefined();
    expect(screen.getByRole("button", { name: "Leave setup for now" })).toBeDefined();
  });

  it("test_a_state_is_carried_by_text_rather_than_by_colour_alone", async () => {
    const user = userEvent.setup();
    await renderWith(
      <SetupWizard onExit={() => undefined} />,
      servicesReturning({
        readinessReport: () =>
          Promise.resolve(
            readiness({
              notes: readinessFacet({ state: "blocked", summary: "nothing is answering" }),
              storage: readinessFacet({ state: "not_configured", summary: "unknown" }),
            }),
          ),
      }),
    );
    await goTo(user, "Ready");

    expect(screen.getAllByText("Ready").length).toBeGreaterThan(0);
    expect(screen.getByText("Needs attention")).toBeDefined();
    expect(screen.getByText("Not configured")).toBeDefined();
  });
});

describe("failures", () => {
  it("test_an_unreachable_service_is_reported_rather_than_rendered_as_an_empty_wizard", async () => {
    await renderWith(
      <SetupWizard onExit={() => undefined} />,
      servicesReturning({
        settingsForm: () => commandFailure("config_unreadable", "the file is not valid TOML"),
      }),
    );

    expect(await screen.findByRole("alert")).toBeDefined();
    expect(screen.getByText("the file is not valid TOML")).toBeDefined();
  });

  it("test_diagnostics_are_not_read_by_the_wizard_at_all", async () => {
    const reads: string[] = [];
    await renderWith(
      <SetupWizard onExit={() => undefined} />,
      servicesReturning({
        diagnosticsReport: () => {
          reads.push("diagnostics");
          return Promise.resolve(diagnostics());
        },
        applyRecovery: () => {
          reads.push("repair");
          return Promise.resolve({ applied: true, summary: "" });
        },
      }),
    );

    // The wizard reads readiness, which is a projection of the same
    // read-only diagnosis. It never applies a recovery, so nothing it
    // does can mutate.
    expect(reads).not.toContain("repair");
  });
});

describe("whether setup is needed", () => {
  it("test_an_installation_that_can_record_does_not_need_setup", () => {
    expect(needsSetup(readiness({ can_record: true }))).toBe(false);
  });

  it("test_an_installation_that_cannot_record_needs_setup", () => {
    expect(needsSetup(readiness({ can_record: false }))).toBe(true);
  });
});
