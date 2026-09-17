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
// capability may apply to an unnamed window, and no broad Tauri plugin
// may be a dependency.
//
// Run: pnpm --dir scrybe-desktop run check:capabilities

import { readFileSync, readdirSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const desktopRoot = dirname(dirname(fileURLToPath(import.meta.url)));
const hostRoot = join(desktopRoot, "src-tauri");
const capabilityDir = join(hostRoot, "capabilities");

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
const ALLOWED_CORE_PERMISSIONS = [
  "core:event:allow-listen",
  "core:event:allow-unlisten",
];

// Tauri plugins this application is allowed to depend on. Empty: every
// capability it needs is a command it defines itself.
const ALLOWED_PLUGINS = [];

/**
 * @typedef {{ where: string, expected: string, observed: string }} Finding
 * @typedef {{ windows?: unknown, permissions?: unknown, remote?: unknown }} Capability
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
 * @param {string} name
 * @param {Capability} capability
 */
function auditCapability(name, capability) {
  const where = `capabilities/${name}`;

  if (!Array.isArray(capability.windows) || capability.windows.length === 0) {
    fail(where, "an explicit non-empty `windows` list", JSON.stringify(capability.windows));
  } else {
    for (const label of capability.windows) {
      if (typeof label !== "string" || label.includes("*")) {
        fail(`${where} windows`, "a literal window label", JSON.stringify(label));
      }
    }
  }

  if (capability.remote !== undefined) {
    fail(where, "no `remote` grant", JSON.stringify(capability.remote));
  }

  if (!Array.isArray(capability.permissions)) {
    fail(where, "a `permissions` array", JSON.stringify(capability.permissions));
    return;
  }

  for (const permission of capability.permissions) {
    if (typeof permission !== "string") {
      fail(`${where} permissions`, "a string permission", JSON.stringify(permission));
      continue;
    }
    if (permission.includes("*")) {
      fail(`${where} permissions`, `no wildcard in ${permission}`, permission);
    }
    const forbidden = FORBIDDEN_NAMESPACES.find(
      (namespace) => permission === namespace || permission.startsWith(`${namespace}:`),
    );
    if (forbidden !== undefined) {
      fail(`${where} permissions`, `no \`${forbidden}\` capability`, permission);
    }
    if (permission.startsWith("core:") && !ALLOWED_CORE_PERMISSIONS.includes(permission)) {
      fail(
        `${where} permissions`,
        `a core permission from [${ALLOWED_CORE_PERMISSIONS.join(", ")}]`,
        permission,
      );
    }
  }
}

const capabilityFiles = readdirSync(capabilityDir).filter((name) => name.endsWith(".json"));
if (capabilityFiles.length === 0) {
  fail("capabilities/", "at least one capability file", "none");
}
/**
 * @param {string} path
 * @returns {Capability}
 */
function readCapability(path) {
  /** @type {unknown} */
  const parsed = JSON.parse(readFileSync(path, "utf8"));
  if (typeof parsed !== "object" || parsed === null) {
    throw new Error(`${path}: a capability file must be a JSON object`);
  }
  return parsed;
}

for (const name of capabilityFiles) {
  auditCapability(name, readCapability(join(capabilityDir, name)));
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
  `capability audit: ok — ${capabilityFiles.length.toString()} capability file(s), ` +
    "no shell, filesystem, network, process, or path grant, no broad plugin",
);
