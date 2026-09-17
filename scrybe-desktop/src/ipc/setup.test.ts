import { describe, expect, it } from "vitest";

import { COMMANDS } from "./client";
import { SETUP_COMMANDS } from "./setup";
import { servicesReturning } from "../testing/services";

describe("setup ipc surface", () => {
  it("test_every_setup_command_appears_in_the_list_checked_against_the_host_grants", () => {
    // `client.test.ts` compares COMMANDS against the capability files.
    // This is the other half: a setup command that never reached that
    // list would be granted by the host and unchecked by that test.
    for (const name of SETUP_COMMANDS) {
      expect(COMMANDS).toContain(name);
    }
  });

  it("test_reading_diagnostics_and_repairing_are_different_service_calls", () => {
    // The mutation boundary as the frontend sees it. A view that could
    // only reach repair through the same call it reads with would make
    // "opening diagnostics mutates nothing" unenforceable above Rust.
    const scrybe = servicesReturning();

    expect(scrybe.diagnosticsReport).not.toBe(scrybe.applyRecovery);
    expect(SETUP_COMMANDS).toContain("diagnostics_report");
    expect(SETUP_COMMANDS).toContain("apply_recovery");
  });

  it("test_reading_a_model_offer_and_installing_it_are_different_service_calls", () => {
    const scrybe = servicesReturning();

    expect(scrybe.modelOffer).not.toBe(scrybe.installModel);
    expect(SETUP_COMMANDS).toContain("model_offer");
    expect(SETUP_COMMANDS).toContain("install_model");
  });
});
