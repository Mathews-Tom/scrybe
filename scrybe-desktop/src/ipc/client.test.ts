import { readFileSync, readdirSync } from "node:fs";
import { join } from "node:path";
import { describe, expect, it } from "vitest";

import { COMMANDS } from "./client";

const CAPABILITY_DIR = join(import.meta.dirname, "../../src-tauri/capabilities");

function grantedCommands(): string[] {
  const granted = new Set<string>();
  for (const name of readdirSync(CAPABILITY_DIR).filter((f) => f.endsWith(".json"))) {
    const capability: unknown = JSON.parse(
      readFileSync(join(CAPABILITY_DIR, name), "utf8"),
    );
    const permissions = (capability as { permissions: string[] }).permissions;
    for (const permission of permissions) {
      if (permission.startsWith("allow-")) {
        granted.add(permission.slice("allow-".length).replaceAll("-", "_"));
      }
    }
  }
  return [...granted].sort();
}

describe("ipc client", () => {
  it("test_client_invokes_only_commands_the_host_capabilities_grant", () => {
    expect([...COMMANDS].sort()).toEqual(grantedCommands());
  });
});
