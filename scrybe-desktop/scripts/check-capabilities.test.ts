import { spawnSync } from "node:child_process";
import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { afterEach, describe, expect, it } from "vitest";

// The capability audit is a gate, so it has to be able to fail. Each
// case below writes a host root that breaks exactly one of its rules
// and asserts the audit rejects it; the first case writes one that
// breaks none, so a later failure is attributable to the mutation
// rather than to the fixture.
//
// The real capability files are audited by the gate itself
// (`pnpm run check:capabilities`) and by `tests/capability_audit.rs`.

const AUDIT = join(import.meta.dirname, "check-capabilities.mjs");

/** A capability the audit accepts, for cases that need one. */
const SOUND_CAPABILITY = {
  identifier: "sessions",
  description: "Reading the sessions under the configured storage root.",
  windows: ["main"],
  permissions: ["allow-list-sessions"],
};

/** @returns the audit's exit code and everything it printed */
function audit(hostRoot: string): { status: number; output: string } {
  const result = spawnSync(process.execPath, [AUDIT, hostRoot], { encoding: "utf8" });
  return { status: result.status ?? -1, output: `${result.stdout}${result.stderr}` };
}

const roots: string[] = [];

interface HostRoot {
  /** Files to write, as paths relative to the host root. */
  files?: Record<string, string>;
  /** Capabilities to inline in `tauri.conf.json`. */
  inlineCapabilities?: unknown[];
}

function hostRoot({ files = {}, inlineCapabilities }: HostRoot): string {
  const root = mkdtempSync(join(tmpdir(), "scrybe-cap-"));
  roots.push(root);

  const security: Record<string, unknown> = { csp: "default-src 'self'" };
  if (inlineCapabilities !== undefined) {
    security.capabilities = inlineCapabilities;
  }
  const written: Record<string, string> = {
    "Cargo.toml": '[dependencies]\ntauri = "2"\n',
    "tauri.conf.json": JSON.stringify({
      app: { windows: [{ label: "main" }], security },
    }),
    ...files,
  };
  for (const [relative, contents] of Object.entries(written)) {
    const path = join(root, relative);
    mkdirSync(dirname(path), { recursive: true });
    writeFileSync(path, contents);
  }
  return root;
}

afterEach(() => {
  for (const root of roots.splice(0)) {
    rmSync(root, { recursive: true, force: true });
  }
});

describe("capability audit", () => {
  it("test_a_host_breaking_no_rule_passes", () => {
    const { status, output } = audit(
      hostRoot({ files: { "capabilities/sessions.json": JSON.stringify(SOUND_CAPABILITY) } }),
    );

    expect(output).toContain("capability audit: ok");
    expect(status).toBe(0);
  });

  it("test_a_capability_one_directory_deeper_is_audited_rather_than_skipped", () => {
    // Tauri's glob is `capabilities/**/*`, so this file is loaded
    // exactly like a sibling one. A non-recursive listing made putting
    // a capability here the cheapest way out of every rule.
    const { status, output } = audit(
      hostRoot({
        files: {
          "capabilities/sessions.json": JSON.stringify(SOUND_CAPABILITY),
          "capabilities/nested/probe.json": JSON.stringify({
            identifier: "probe",
            windows: ["*"],
            remote: { urls: ["https://example.invalid"] },
            permissions: ["core:default", "shell:allow-execute"],
          }),
        },
      }),
    );

    expect(output).toContain("capabilities/nested/probe.json");
    expect(output).toContain("no `remote` grant");
    expect(output).toContain("no `shell` capability");
    expect(output).toContain("a literal label, with no glob metacharacter");
    expect(status).toBe(1);
  });

  it("test_a_toml_capability_is_audited_rather_than_skipped", () => {
    // `tauri-utils` reads TOML capabilities unconditionally, so an
    // extension filter of `.json` alone was an opt-out.
    const { status, output } = audit(
      hostRoot({
        files: {
          "capabilities/probe.toml": [
            'identifier = "probe"',
            'windows = ["main"]',
            'permissions = ["shell:allow-execute"]',
          ].join("\n"),
        },
      }),
    );

    expect(output).toContain("capabilities/probe.toml");
    expect(output).toContain("no `shell` capability");
    expect(status).toBe(1);
  });

  it("test_a_capability_inlined_in_the_configuration_is_audited", () => {
    // A non-empty inline list replaces the directory rather than adding
    // to it, so reading only the directory audited a policy the host
    // does not use.
    const { status, output } = audit(
      hostRoot({
        files: { "capabilities/sessions.json": JSON.stringify(SOUND_CAPABILITY) },
        inlineCapabilities: [
          {
            identifier: "probe",
            windows: ["main"],
            permissions: ["fs:allow-read-text-file"],
          },
        ],
      }),
    );

    expect(output).toContain("tauri.conf.json app.security.capabilities[0]");
    expect(output).toContain("no `fs` capability");
    expect(status).toBe(1);
  });

  it("test_a_file_the_audit_cannot_read_fails_rather_than_being_skipped", () => {
    const { status, output } = audit(
      hostRoot({
        files: {
          "capabilities/sessions.json": JSON.stringify(SOUND_CAPABILITY),
          "capabilities/probe.yaml": "identifier: probe\n",
        },
      }),
    );

    expect(output).toContain("capabilities/probe.yaml");
    expect(output).toContain("a file this audit can read");
    expect(status).toBe(1);
  });

  it("test_a_window_label_that_is_a_glob_pattern_is_rejected", () => {
    // `mai?` reads as a literal to a reviewer and matches every
    // four-character window label to Tauri.
    const { status, output } = audit(
      hostRoot({
        files: {
          "capabilities/sessions.json": JSON.stringify({
            ...SOUND_CAPABILITY,
            windows: ["mai?"],
          }),
        },
      }),
    );

    expect(output).toContain("capabilities/sessions.json windows");
    expect(output).toContain("mai?");
    expect(status).toBe(1);
  });

  it("test_a_wildcard_webview_label_is_rejected_the_same_as_a_window_label", () => {
    // Tauri grants when EITHER list matches, so a literal `windows`
    // beside a wildcard `webviews` is not scoped at all.
    const { status, output } = audit(
      hostRoot({
        files: {
          "capabilities/sessions.json": JSON.stringify({
            ...SOUND_CAPABILITY,
            webviews: ["*"],
          }),
        },
      }),
    );

    expect(output).toContain("capabilities/sessions.json webviews");
    expect(status).toBe(1);
  });

  it("test_a_capability_naming_neither_a_window_nor_a_webview_is_rejected", () => {
    const { status, output } = audit(
      hostRoot({
        files: {
          "capabilities/sessions.json": JSON.stringify({
            identifier: "sessions",
            permissions: ["allow-list-sessions"],
          }),
        },
      }),
    );

    expect(output).toContain("at least one literal `windows` or `webviews` label");
    expect(status).toBe(1);
  });

  it("test_a_permission_set_reaching_a_forbidden_namespace_is_rejected", () => {
    // The set is named `allow-…`, so every rule applied to the literal
    // string in the capability file passes. What it grants is a
    // namespace the boundary forbids, which is only visible after the
    // set is expanded.
    const { status, output } = audit(
      hostRoot({
        files: {
          "capabilities/sessions.json": JSON.stringify({
            ...SOUND_CAPABILITY,
            permissions: ["allow-session-tools"],
          }),
          "permissions/session-tools.toml": [
            "[[set]]",
            'identifier = "allow-session-tools"',
            'description = "A set that reaches a forbidden namespace indirectly."',
            'permissions = ["core:path:default"]',
          ].join("\n"),
        },
      }),
    );

    expect(output).toContain("via allow-session-tools");
    expect(output).toContain("no `core:path` capability");
    expect(status).toBe(1);
  });

  it("test_a_permission_set_that_contains_itself_is_reported_rather_than_looped_on", () => {
    const { status, output } = audit(
      hostRoot({
        files: {
          "capabilities/sessions.json": JSON.stringify({
            ...SOUND_CAPABILITY,
            permissions: ["allow-session-tools"],
          }),
          "permissions/session-tools.toml": [
            "[[set]]",
            'identifier = "allow-session-tools"',
            'description = "A set that contains itself."',
            'permissions = ["allow-session-tools"]',
          ].join("\n"),
        },
      }),
    );

    expect(output).toContain("a set that does not contain itself");
    expect(status).toBe(1);
  });
});
