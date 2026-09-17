// Copyright 2026 Mathews Tom
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//     https://www.apache.org/licenses/LICENSE-2.0

// Audits the host's capability policy.
//
// The Rust side proves the capability files and the registered command
// list describe the same surface. This proves the surface is one the
// trust boundary allows at all: no capability may reach a shell, a
// filesystem, the network, the process table, or an arbitrary path, no
// capability may apply to an unnamed window or webview, and no broad
// Tauri plugin may be a dependency.
//
// It audits every capability Tauri itself would load, because a rule
// that covers only some of them is opt-out for whoever writes the one
// it misses. Tauri collects capabilities with the glob
// `capabilities/**/*`, accepts three file formats, and additionally
// honours capabilities declared inline in `tauri.conf.json` under
// `app.security.capabilities` — which, when present, replace the
// directory rather than adding to it.
//
// Run: pnpm --dir scrybe-desktop run check:capabilities
//
// An optional argument names a different host root, so the rules below
// can be tested against a tree that deliberately breaks them.

import { existsSync, readFileSync, readdirSync, statSync } from "node:fs";
import { dirname, extname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

import JSON5 from "json5";
import { parse as parseToml } from "smol-toml";

const desktopRoot = dirname(dirname(fileURLToPath(import.meta.url)));
const hostRoot =
  process.argv[2] === undefined ? join(desktopRoot, "src-tauri") : resolve(process.argv[2]);
const capabilityDir = join(hostRoot, "capabilities");
const permissionDir = join(hostRoot, "permissions");
const configPath = join(hostRoot, "tauri.conf.json");

// A permission whose namespace is one of these hands the WebView a
// general-purpose capability rather than a named application use case.
// Adding one is a trust-boundary change and must be argued in review,
// not slipped in as a dependency.
const FORBIDDEN_NAMESPACES = [
  "shell",
  "fs",
  "http",
  "process",
  "os",
  "dialog",
  "clipboard-manager",
  "global-shortcut",
  "updater",
  "upload",
  "websocket",
  "deep-link",
  "store",
  "sql",
  "stronghold",
  "core:path",
];

// Core permissions that are narrow enough to grant but still worth
// naming, so an unexpected one fails rather than passes quietly.
const ALLOWED_CORE_PERMISSIONS = ["core:event:allow-listen", "core:event:allow-unlisten"];

// Tauri plugins this application is allowed to depend on. Empty: every
// capability it needs is a command it defines itself.
const ALLOWED_PLUGINS = [];

// The formats `tauri_utils::acl::capability::CapabilityFile::load`
// parses. `toml` is unconditional; `json5` is behind the `config-json5`
// feature, which this host does not enable today and which is one
// manifest edit away from being enabled. Auditing a format the build
// does not currently load costs nothing; skipping one it does load is
// how a rule becomes optional.
/** @type {Map<string, (text: string) => unknown>} */
const CAPABILITY_PARSERS = new Map([
  [".json", (text) => /** @type {unknown} */ (JSON.parse(text))],
  [".json5", (text) => /** @type {unknown} */ (JSON5.parse(text))],
  [".toml", (text) => /** @type {unknown} */ (parseToml(text))],
]);

// `tauri_utils::acl::build::PERMISSION_FILE_EXTENSIONS`. A permission
// file has no JSON5 form even when that feature is on.
/** @type {Map<string, (text: string) => unknown>} */
const PERMISSION_PARSERS = new Map([
  [".json", (text) => /** @type {unknown} */ (JSON.parse(text))],
  [".toml", (text) => /** @type {unknown} */ (parseToml(text))],
]);

// Tauri skips this folder when collecting capabilities: it holds the
// JSON schemas it generates for editor completion, not policy.
const SCHEMA_FOLDER = "schemas";

// Both label lists are compiled to `glob::Pattern`, so each of these
// widens a capability past the literal label it appears to name. `*`
// alone was the only one checked before, which left `mai?` and `mai[n]`
// reading as literals to a reviewer and as wildcards to Tauri.
const GLOB_METACHARACTERS = ["*", "?", "[", "]"];

/**
 * @typedef {{ identifier?: unknown, windows?: unknown, webviews?: unknown, permissions?: unknown, remote?: unknown }} Capability
 * @typedef {{ where: string, expected: string, observed: string }} Finding
 */

/** @type {Finding[]} */
const failures = [];

/**
 * @param {string} where
 * @param {string} expected
 * @param {string} observed
 */
function fail(where, expected, observed) {
  failures.push({ where, expected, observed });
}

/**
 * A list, with its elements still `unknown`.
 *
 * `Array.isArray` narrows an `unknown` to `any[]`, which would make
 * every element of every policy file `any` from that point on. Keeping
 * them `unknown` is what forces each one to be checked before it is
 * used, which is the whole job of this file.
 *
 * @param {unknown} value
 * @returns {unknown[] | undefined}
 */
function asList(value) {
  return Array.isArray(value) ? /** @type {unknown[]} */ (value) : undefined;
}

/**
 * Every file under `directory`, at any depth, relative to it.
 *
 * Tauri's glob is `capabilities/**` + `/*`, so a capability one
 * directory deeper is loaded exactly like a sibling one. A
 * non-recursive listing made that the cheapest way to put a capability
 * outside every rule below.
 *
 * @param {string} directory
 * @param {string} prefix
 * @returns {string[]}
 */
function walk(directory, prefix = "") {
  /** @type {string[]} */
  const found = [];
  for (const entry of readdirSync(directory).sort()) {
    const absolute = join(directory, entry);
    const relative = prefix === "" ? entry : `${prefix}/${entry}`;
    if (statSync(absolute).isDirectory()) {
      if (entry !== SCHEMA_FOLDER) {
        found.push(...walk(absolute, relative));
      }
    } else {
      found.push(relative);
    }
  }
  return found;
}

/**
 * Parses one policy file, failing on a format this audit cannot read.
 *
 * Skipping an unrecognised file would reintroduce the hole this walk
 * closes: whether Tauri loads that format or not, an audit that cannot
 * read a file under `capabilities/` has not audited it, and saying so
 * is the only honest outcome.
 *
 * @param {string} where
 * @param {string} path
 * @param {Map<string, (text: string) => unknown>} parsers
 * @returns {unknown}
 */
function parsePolicyFile(where, path, parsers) {
  const parse = parsers.get(extname(path).toLowerCase());
  if (parse === undefined) {
    fail(where, `a file this audit can read [${[...parsers.keys()].join(", ")}]`, path);
    return undefined;
  }
  return parse(readFileSync(path, "utf8"));
}

/**
 * The capabilities one file declares.
 *
 * `CapabilityFile` is an untagged enum: one capability, a bare list, or
 * an object carrying a `capabilities` list. Tauri loads all three, so
 * all three are audited.
 *
 * @param {string} where
 * @param {unknown} parsed
 * @returns {Capability[]}
 */
function capabilitiesIn(where, parsed) {
  const list = asList(parsed);
  if (list !== undefined) {
    return list.map((entry) => /** @type {Capability} */ (entry));
  }
  if (typeof parsed !== "object" || parsed === null) {
    fail(where, "a capability object, a list, or a `capabilities` list", JSON.stringify(parsed));
    return [];
  }
  const named = asList(/** @type {{ capabilities?: unknown }} */ (parsed).capabilities);
  if (named !== undefined) {
    return named.map((entry) => /** @type {Capability} */ (entry));
  }
  return [/** @type {Capability} */ (parsed)];
}

/**
 * Every hand-written permission set, by identifier.
 *
 * A set names other permissions, so auditing the literal strings in a
 * capability file without expanding sets lets one indirection reach
 * something the rules below forbid outright.
 *
 * @returns {Map<string, string[]>}
 */
function permissionSets() {
  /** @type {Map<string, string[]>} */
  const sets = new Map();
  if (!existsSync(permissionDir)) {
    return sets;
  }
  for (const name of walk(permissionDir)) {
    const where = `permissions/${name}`;
    const parsed = parsePolicyFile(where, join(permissionDir, name), PERMISSION_PARSERS);
    if (typeof parsed !== "object" || parsed === null) {
      continue;
    }
    const declared = /** @type {{ set?: unknown, default?: unknown }} */ (parsed);
    const entries = [...(asList(declared.set) ?? [])];
    // The application's own default set is referenced by the bare
    // identifier `default` and carries no `identifier` field itself.
    if (typeof declared.default === "object" && declared.default !== null) {
      entries.push({
        identifier: "default",
        .../** @type {Record<string, unknown>} */ (declared.default),
      });
    }
    for (const entry of entries) {
      if (typeof entry !== "object" || entry === null) {
        fail(where, "a permission set object", JSON.stringify(entry));
        continue;
      }
      const { identifier, permissions } =
        /** @type {{ identifier?: unknown, permissions?: unknown }} */ (entry);
      const members = asList(permissions);
      if (typeof identifier !== "string" || members === undefined) {
        fail(where, "a set with an `identifier` and a `permissions` array", JSON.stringify(entry));
        continue;
      }
      sets.set(
        identifier,
        members.map((permission) =>
          typeof permission === "string" ? permission : JSON.stringify(permission),
        ),
      );
    }
  }
  return sets;
}

/**
 * Resolves a permission to the permissions it actually grants.
 *
 * Each leaf carries the chain that reached it, so a finding names the
 * set a reviewer has to go and read rather than only its result.
 *
 * @param {string} identifier
 * @param {Map<string, string[]>} sets
 * @param {string[]} chain
 * @returns {{ permission: string, via: string[] }[]}
 */
function expandPermission(identifier, sets, chain = []) {
  const members = sets.get(identifier);
  if (members === undefined) {
    return [{ permission: identifier, via: chain }];
  }
  if (chain.includes(identifier)) {
    // Tauri rejects a cyclic set at build time; reporting it rather
    // than recursing keeps this audit from hanging on one.
    fail(
      `permissions set \`${identifier}\``,
      "a set that does not contain itself",
      [...chain, identifier].join(" -> "),
    );
    return [];
  }
  return members.flatMap((member) => expandPermission(member, sets, [...chain, identifier]));
}

/**
 * Audits one list of window or webview labels.
 *
 * Tauri grants a command when EITHER list matches, and compiles both as
 * glob patterns, so a wildcard in either applies the capability
 * everywhere no matter how literal the other list looks.
 *
 * @param {string} where
 * @param {string} field
 * @param {unknown} labels
 * @returns {boolean} whether the list named at least one literal label
 */
function auditLabels(where, field, labels) {
  if (labels === undefined) {
    return false;
  }
  const list = asList(labels);
  if (list === undefined) {
    fail(`${where} ${field}`, `a \`${field}\` list`, JSON.stringify(labels));
    return false;
  }
  let named = false;
  for (const label of list) {
    if (typeof label !== "string") {
      fail(`${where} ${field}`, "a literal label", JSON.stringify(label));
      continue;
    }
    const wildcard = GLOB_METACHARACTERS.find((character) => label.includes(character));
    if (wildcard === undefined) {
      named = true;
    } else {
      fail(
        `${where} ${field}`,
        `a literal label, with no glob metacharacter from [${GLOB_METACHARACTERS.join(" ")}]`,
        label,
      );
    }
  }
  return named;
}

/**
 * @param {string} where
 * @param {Capability} capability
 * @param {Map<string, string[]>} sets
 */
function auditCapability(where, capability, sets) {
  const namedWindow = auditLabels(where, "windows", capability.windows);
  const namedWebview = auditLabels(where, "webviews", capability.webviews);
  if (!namedWindow && !namedWebview) {
    fail(
      where,
      "at least one literal `windows` or `webviews` label",
      `windows: ${JSON.stringify(capability.windows)}, webviews: ${JSON.stringify(capability.webviews)}`,
    );
  }

  if (capability.remote !== undefined) {
    fail(where, "no `remote` grant", JSON.stringify(capability.remote));
  }

  const permissions = asList(capability.permissions);
  if (permissions === undefined) {
    fail(where, "a `permissions` array", JSON.stringify(capability.permissions));
    return;
  }

  for (const declared of permissions) {
    if (typeof declared !== "string") {
      fail(`${where} permissions`, "a string permission", JSON.stringify(declared));
      continue;
    }
    for (const { permission, via } of expandPermission(declared, sets)) {
      const reached =
        via.length === 0 ? `${where} permissions` : `${where} permissions, via ${via.join(" -> ")}`;
      if (permission.includes("*")) {
        fail(reached, `no wildcard in ${permission}`, permission);
      }
      const forbidden = FORBIDDEN_NAMESPACES.find(
        (namespace) => permission === namespace || permission.startsWith(`${namespace}:`),
      );
      if (forbidden !== undefined) {
        fail(reached, `no \`${forbidden}\` capability`, permission);
      }
      if (permission.startsWith("core:") && !ALLOWED_CORE_PERMISSIONS.includes(permission)) {
        fail(
          reached,
          `a core permission from [${ALLOWED_CORE_PERMISSIONS.join(", ")}]`,
          permission,
        );
      }
    }
  }
}

const sets = permissionSets();

/** @type {{ where: string, capability: Capability }[]} */
const audited = [];

const capabilityFiles = existsSync(capabilityDir) ? walk(capabilityDir) : [];
for (const name of capabilityFiles) {
  const where = `capabilities/${name}`;
  const parsed = parsePolicyFile(where, join(capabilityDir, name), CAPABILITY_PARSERS);
  if (parsed !== undefined) {
    for (const capability of capabilitiesIn(where, parsed)) {
      audited.push({ where, capability });
    }
  }
}

// `get_capabilities` uses the inline list in place of the directory
// when it is non-empty, keeping only the file capabilities it names by
// reference. An inline entry therefore does not merely add to the
// reviewed set, it can replace it, and reading only the directory left
// it entirely unaudited.
const hostConfig = /** @type {unknown} */ (JSON.parse(readFileSync(configPath, "utf8")));
const inlined = /** @type {{ app?: { security?: { capabilities?: unknown } } }} */ (hostConfig).app
  ?.security?.capabilities;
if (inlined !== undefined) {
  const entries = asList(inlined);
  if (entries !== undefined) {
    for (const [index, entry] of entries.entries()) {
      // A string is a reference to a capability file, which the walk
      // above already audited. Anything else is declared here and
      // nowhere else.
      if (typeof entry !== "string") {
        audited.push({
          where: `tauri.conf.json app.security.capabilities[${index.toString()}]`,
          capability: /** @type {Capability} */ (entry),
        });
      }
    }
  } else {
    fail(
      "tauri.conf.json app.security.capabilities",
      "a list of capabilities",
      JSON.stringify(inlined),
    );
  }
}

if (audited.length === 0) {
  fail("capabilities/", "at least one capability", "none");
}

for (const { where, capability } of audited) {
  auditCapability(where, capability, sets);
}

const manifest = readFileSync(join(hostRoot, "Cargo.toml"), "utf8");
for (const [, plugin] of manifest.matchAll(/^(tauri-plugin-[a-z0-9-]+)/gm)) {
  if (!ALLOWED_PLUGINS.includes(plugin)) {
    fail("src-tauri/Cargo.toml", `a plugin from [${ALLOWED_PLUGINS.join(", ")}]`, plugin);
  }
}

if (failures.length > 0) {
  console.error(`capability audit FAILED — ${failures.length.toString()} findings:`);
  for (const { where, expected, observed } of failures) {
    console.error(`  ${where}`);
    console.error(`    expected: ${expected}`);
    console.error(`    observed: ${observed}`);
  }
  process.exit(1);
}

console.log(
  `capability audit: ok — ${audited.length.toString()} capability/capabilities audited ` +
    `from ${capabilityFiles.length.toString()} file(s) under capabilities/ and from ` +
    `tauri.conf.json, ${sets.size.toString()} permission set(s) expanded, ` +
    "no shell, filesystem, network, process, or path grant, " +
    "no wildcard window or webview label, no broad plugin",
);
