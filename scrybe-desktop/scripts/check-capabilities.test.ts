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

  it("test_a_capability_below_the_schemas_folder_is_audited_rather_than_skipped", () => {
    // Tauri filters a capability *file* whose immediate parent is named
    // `schemas`; it does not stop the glob descending through one. This
    // audit used to refuse to recurse into `schemas` at all, which was
    // strictly wider, so `capabilities/schemas/nested/probe.json` — a
    // file Tauri loads, because its parent is `nested` — was outside
    // every rule here.
    const { status, output } = audit(
      hostRoot({
        files: {
          "capabilities/sessions.json": JSON.stringify(SOUND_CAPABILITY),
          "capabilities/schemas/nested/probe.json": JSON.stringify({
            identifier: "probe",
            windows: ["*"],
            webviews: ["*"],
            remote: { urls: ["https://example.invalid"] },
            permissions: ["shell:allow-execute"],
          }),
        },
      }),
    );

    expect(output).toContain("capabilities/schemas/nested/probe.json");
    expect(output).toContain("no `remote` grant");
    expect(output).toContain("no `shell` capability");
    expect(output).toContain("a literal label, with no glob metacharacter");
    expect(status).toBe(1);
  });

  it("test_a_capability_directly_inside_the_schemas_folder_is_skipped_as_tauri_skips_it", () => {
    // The other half of the same rule: a file whose immediate parent is
    // `schemas` is a generated editor schema, not policy, and Tauri
    // does not load it. Auditing it would fail the gate on a build
    // artefact.
    const { status, output } = audit(
      hostRoot({
        files: {
          "capabilities/sessions.json": JSON.stringify(SOUND_CAPABILITY),
          "capabilities/schemas/desktop-schema.json": JSON.stringify({
            $schema: "http://json-schema.org/draft-07/schema#",
            title: "not a capability",
          }),
        },
      }),
    );

    expect(output).toContain("capability audit: ok");
    expect(status).toBe(0);
  });

  it("test_a_permission_set_below_the_schemas_folder_is_expanded_rather_than_left_unresolved", () => {
    // `define_permissions` filters permission files the same way, so a
    // set under `permissions/schemas/nested/` is loaded by Tauri. When
    // this expander skipped the whole `schemas` subtree the capability
    // referencing it audited as an unresolved leaf and the grants it
    // actually carried were never seen.
    const { status, output } = audit(
      hostRoot({
        files: {
          "capabilities/sessions.json": JSON.stringify({
            ...SOUND_CAPABILITY,
            permissions: ["allow-session-tools"],
          }),
          "permissions/schemas/nested/session-tools.toml": [
            "[[set]]",
            'identifier = "allow-session-tools"',
            'description = "A set Tauri loads from below the schemas folder."',
            'permissions = ["shell:allow-execute"]',
          ].join("\n"),
        },
      }),
    );

    expect(output).toContain("via allow-session-tools");
    expect(output).toContain("no `shell` capability");
    expect(status).toBe(1);
  });

  it("test_a_permission_in_a_namespace_the_denylist_does_not_name_is_rejected", () => {
    // The forbidden-namespace list can only reject a namespace someone
    // thought to write down. `clipboard-manager` is on it; the rule
    // under test is the allow-list that catches the ones that are not,
    // so a plugin surface cannot arrive through a namespace this audit
    // has never heard of.
    const { status, output } = audit(
      hostRoot({
        files: {
          "capabilities/sessions.json": JSON.stringify({
            ...SOUND_CAPABILITY,
            permissions: ["allow-list-sessions", "notification:allow-notify"],
          }),
        },
      }),
    );

    expect(output).toContain("notification:allow-notify");
    expect(output).toContain("an `allow-<command>` grant");
    expect(status).toBe(1);
  });

  it("test_an_unapproved_command_in_an_allowed_plugin_namespace_is_rejected", () => {
    // Granting the one relaunch operation must not turn `process` into
    // an allowed namespace. A neighbouring command remains outside the
    // exact framework permission allow-list.
    const { status, output } = audit(
      hostRoot({
        files: {
          "capabilities/sessions.json": JSON.stringify({
            ...SOUND_CAPABILITY,
            permissions: ["allow-list-sessions", "process:allow-exit"],
          }),
        },
      }),
    );

    expect(output).toContain("no `process` capability outside the exact allow-list");
    expect(output).toContain("process:allow-exit");
    expect(status).toBe(1);
  });

  it("test_a_capability_inlined_in_a_platform_config_file_is_audited", () => {
    // Tauri merges `tauri.macos.conf.json` over the base configuration
    // as an RFC 7396 patch, which replaces an array outright. A
    // capability list declared there replaces both the inline list and
    // the directory, and reading only `tauri.conf.json` left the policy
    // that actually ships on this application's one target unaudited.
    const { status, output } = audit(
      hostRoot({
        files: {
          "capabilities/sessions.json": JSON.stringify(SOUND_CAPABILITY),
          "tauri.macos.conf.json": JSON.stringify({
            app: {
              security: {
                capabilities: [
                  {
                    identifier: "probe",
                    windows: ["main"],
                    permissions: ["fs:allow-read-text-file"],
                  },
                ],
              },
            },
          }),
        },
      }),
    );

    expect(output).toContain("tauri.macos.conf.json app.security.capabilities[0]");
    expect(output).toContain("no `fs` capability");
    expect(status).toBe(1);
  });

  it("test_a_plugin_spelled_as_a_dependency_table_is_rejected", () => {
    // `[dependencies.tauri-plugin-shell]` begins with `[`, so a scan
    // anchored to a line starting with the crate name walked past it
    // while the summary claimed "no broad plugin" unconditionally.
    const { status, output } = audit(
      hostRoot({
        files: {
          "capabilities/sessions.json": JSON.stringify(SOUND_CAPABILITY),
          "Cargo.toml": '[dependencies]\ntauri = "2"\n\n[dependencies.tauri-plugin-shell]\nversion = "2"\n',
        },
      }),
    );

    expect(output).toContain("src-tauri/Cargo.toml");
    expect(output).toContain("tauri-plugin-shell");
    expect(status).toBe(1);
  });

  it("test_a_plugin_spelled_as_a_renamed_dependency_is_rejected", () => {
    // The key says `shell`; what is depended on is the `package` value.
    const { status, output } = audit(
      hostRoot({
        files: {
          "capabilities/sessions.json": JSON.stringify(SOUND_CAPABILITY),
          "Cargo.toml":
            '[dependencies]\ntauri = "2"\nshell = { package = "tauri-plugin-shell", version = "2" }\n',
        },
      }),
    );

    expect(output).toContain("src-tauri/Cargo.toml");
    expect(output).toContain("tauri-plugin-shell");
    expect(status).toBe(1);
  });

  it("test_a_plugin_under_build_dependencies_is_rejected", () => {
    const { status, output } = audit(
      hostRoot({
        files: {
          "capabilities/sessions.json": JSON.stringify(SOUND_CAPABILITY),
          "Cargo.toml":
            '[dependencies]\ntauri = "2"\n\n[build-dependencies.tauri-plugin-shell]\nversion = "2"\n',
        },
      }),
    );

    expect(output).toContain("src-tauri/Cargo.toml");
    expect(output).toContain("tauri-plugin-shell");
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
