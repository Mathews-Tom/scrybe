// Copyright 2026 Mathews Tom
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//     https://www.apache.org/licenses/LICENSE-2.0

//! The capability files and the registered commands describe one
//! surface.
//!
//! Tauri rejects an invoke that no capability grants, so a command
//! missing from the capability files is unreachable and a grant with no
//! command behind it is a stale widening of the trust boundary. Neither
//! shows up as a compile error, so both are asserted here.
//!
//! Every assertion runs over the same capabilities Tauri itself loads:
//! the whole `capabilities/**/*` tree rather than its top directory,
//! every format this audit can read rather than one, and the inline
//! list in every configuration file Tauri reads — the base one and the
//! `tauri.<platform>.conf.json` merged over it — which replaces the
//! directory when it is present. A rule applied to a subset of the
//! policy is a rule the next capability can be written just outside of.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use scrybe_desktop::commands::COMMANDS;

/// The capability formats this test reads.
///
/// `tauri-utils` also reads `json5`, but only with its `config-json5`
/// feature, which this host's `tauri-build` dependency does not enable;
/// parsing it here would add six crates to a dependency graph for a
/// format the build currently ignores. A `.json5` capability file is
/// therefore rejected outright by [`capability_documents`] rather than
/// skipped, so it can never sit in the policy directory unexamined —
/// the same outcome the `check:capabilities` gate reaches by reading
/// it.
const CAPABILITY_EXTENSIONS: &[&str] = &["json", "toml"];

/// Tauri compiles `windows` and `webviews` as glob patterns, so each of
/// these widens a capability past the literal label it appears to name.
const GLOB_METACHARACTERS: &[char] = &['*', '?', '[', ']'];

/// The configuration files `tauri_utils::config::parse::read_from`
/// reads.
///
/// It loads one base file and then, when the file for the target
/// platform is present, merges it in with `json_patch::merge` — RFC
/// 7396 JSON Merge Patch, which replaces an array outright rather than
/// concatenating it. `app.security.capabilities` declared in
/// `tauri.macos.conf.json` would therefore replace the inline list and,
/// being non-empty, the capability directory with it. Reading only
/// `tauri.conf.json` left that policy unaudited while the module
/// documentation above claimed otherwise.
///
/// Every recognised name is read rather than the base plus the one
/// platform in use: the union cannot miss the set that wins the merge,
/// and a platform file added for a future target is audited the day it
/// lands. The `json5` and `Tauri.toml` names are behind the
/// `config-json5` and `config-toml` features, and are covered for the
/// same reason [`CAPABILITY_EXTENSIONS`] covers TOML.
const CONFIG_FILES: &[&str] = &[
    "tauri.conf.json",
    "tauri.conf.json5",
    "Tauri.toml",
    "tauri.macos.conf.json",
    "tauri.macos.conf.json5",
    "Tauri.macos.toml",
    "tauri.windows.conf.json",
    "tauri.windows.conf.json5",
    "Tauri.windows.toml",
    "tauri.linux.conf.json",
    "tauri.linux.conf.json5",
    "Tauri.linux.toml",
    "tauri.android.conf.json",
    "tauri.android.conf.json5",
    "Tauri.android.toml",
    "tauri.ios.conf.json",
    "tauri.ios.conf.json5",
    "Tauri.ios.toml",
];

/// Tauri skips the files directly inside this folder when collecting
/// capabilities: it holds the JSON schemas it generates for editor
/// completion, not policy. It skips those files only, not the subtree
/// — see [`capability_files`].
const SCHEMA_FOLDER: &str = "schemas";

fn host_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).to_path_buf()
}

/// Every capability file under `capabilities/`, at any depth.
///
/// Tauri's glob descends, so a capability one directory deeper is
/// loaded exactly like a sibling one. A file in a format this test
/// cannot read is a failure rather than a skip: an unreadable file in
/// the policy directory is an unaudited one.
///
/// `schemas` is skipped exactly as `parse_capabilities` skips it and
/// no more widely. That filter is
/// `p.parent().file_name() != "schemas"`, which drops a file sitting
/// directly in a `schemas` directory while the glob still descends
/// through it. Refusing to descend was strictly wider, and the gap was
/// a bypass rather than a narrowing: `capabilities/schemas/nested/`
/// has the parent `nested`, so Tauri loads what is in it and this
/// audit used to see none of it.
fn capability_files() -> Vec<PathBuf> {
    let mut found = Vec::new();
    let mut pending = vec![host_root().join("capabilities")];
    while let Some(directory) = pending.pop() {
        let in_schema_folder = directory
            .file_name()
            .is_some_and(|name| name == SCHEMA_FOLDER);
        for entry in std::fs::read_dir(&directory).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                pending.push(path);
                continue;
            }
            if in_schema_folder {
                continue;
            }
            let extension = path
                .extension()
                .map(|extension| extension.to_string_lossy().into_owned())
                .unwrap_or_default();
            assert!(
                CAPABILITY_EXTENSIONS.contains(&extension.as_str()),
                "{}: a capability file this audit cannot read is an unaudited one; \
                 readable formats are {CAPABILITY_EXTENSIONS:?}",
                path.display(),
            );
            found.push(path);
        }
    }
    found.sort();
    found
}

/// Every capability Tauri would apply, from the files and from the
/// configuration.
///
/// A capability file holds one capability, a bare list, or an object
/// carrying a `capabilities` list, and `tauri.conf.json` may declare
/// capabilities inline under `app.security.capabilities` — where a
/// non-empty list replaces the directory rather than adding to it.
fn capabilities() -> Vec<(String, serde_json::Value)> {
    let mut found = Vec::new();
    for path in capability_files() {
        let text = std::fs::read_to_string(&path).unwrap();
        let document: serde_json::Value = match path.extension().unwrap().to_string_lossy().as_ref()
        {
            "toml" => toml::from_str(&text).unwrap(),
            _ => serde_json::from_str(&text).unwrap(),
        };
        let where_ = path.display().to_string();
        match document {
            serde_json::Value::Array(list) => {
                found.extend(list.into_iter().map(|entry| (where_.clone(), entry)));
            }
            serde_json::Value::Object(ref object) if object.contains_key("capabilities") => {
                let list = object["capabilities"].as_array().unwrap().clone();
                found.extend(list.into_iter().map(|entry| (where_.clone(), entry)));
            }
            other => found.push((where_, other)),
        }
    }

    let mut configs_read = 0_usize;
    for name in CONFIG_FILES {
        let path = host_root().join(name);
        if !path.exists() {
            continue;
        }
        configs_read += 1;
        let text = std::fs::read_to_string(&path).unwrap();
        let extension = path
            .extension()
            .map(|extension| extension.to_string_lossy().into_owned())
            .unwrap_or_default();
        let config: serde_json::Value = if extension == "toml" {
            toml::from_str(&text).unwrap()
        } else {
            // A `.json5` configuration is rejected rather than skipped,
            // for the reason [`CAPABILITY_EXTENSIONS`] gives: a file
            // this audit cannot read is one it has not audited.
            assert!(
                extension == "json",
                "{name}: a configuration file this audit cannot read is an unaudited one",
            );
            serde_json::from_str(&text).unwrap()
        };
        let Some(inlined) = config["app"]["security"]["capabilities"].as_array() else {
            continue;
        };
        // A string entry references a capability file, already
        // collected above. Anything else is declared only here.
        found.extend(
            inlined
                .iter()
                .filter(|entry| !entry.is_string())
                .enumerate()
                .map(|(index, entry)| {
                    (
                        format!("{name} app.security.capabilities[{index}]"),
                        entry.clone(),
                    )
                }),
        );
    }
    assert!(
        configs_read > 0,
        "no Tauri configuration file was found to audit; expected one of {CONFIG_FILES:?}",
    );

    assert!(!found.is_empty(), "no capability was found to audit");
    found
}

/// Every `allow-<command>` grant across every capability.
fn granted_commands() -> BTreeSet<String> {
    let mut granted = BTreeSet::new();
    for (where_, capability) in capabilities() {
        let permissions = capability["permissions"]
            .as_array()
            .unwrap_or_else(|| panic!("{where_}: a capability without a `permissions` array"));
        for permission in permissions {
            let permission = permission.as_str().unwrap();
            if let Some(command) = permission.strip_prefix("allow-") {
                granted.insert(command.replace('-', "_"));
            }
        }
    }
    granted
}

#[test]
fn test_every_registered_command_is_granted_to_exactly_one_window() {
    let registered: BTreeSet<String> = COMMANDS.iter().map(|name| (*name).to_owned()).collect();

    assert_eq!(
        granted_commands(),
        registered,
        "\nthe capability files and `commands::COMMANDS` disagree. A command \
         the files do not grant is rejected at the IPC boundary; a grant with \
         no command behind it widens the trust boundary for nothing.\n",
    );
}

#[test]
fn test_the_access_control_manifest_declares_the_same_commands_the_host_registers() {
    // `build.rs` names the commands Tauri generates `allow-`/`deny-`
    // permissions from. A build script runs before the crate it builds,
    // so it cannot import `commands::COMMANDS`; the two lists are
    // compared here instead. A command missing from `build.rs` has no
    // permission, so no capability can grant it and every invoke of it
    // is rejected.
    let build_script = std::fs::read_to_string(host_root().join("build.rs")).unwrap();
    let declaration = build_script
        .split_once("const COMMANDS: &[&str] = &[")
        .and_then(|(_, rest)| rest.split_once("];"))
        .map(|(body, _)| body)
        .expect("build.rs no longer declares a COMMANDS array");

    let declared: BTreeSet<String> = declaration
        .split('"')
        .skip(1)
        .step_by(2)
        .map(str::to_owned)
        .collect();
    let registered: BTreeSet<String> = COMMANDS.iter().map(|name| (*name).to_owned()).collect();

    assert_eq!(declared, registered);
}

#[test]
fn test_no_capability_applies_to_an_unnamed_window_or_webview() {
    // Tauri grants a command when EITHER the webview patterns or the
    // window patterns match, and compiles both as globs. Reading only
    // `windows`, and only for emptiness, left `"webviews": ["*"]` and
    // `"windows": ["mai?"]` both reading as scoped to one window and
    // both applying to every webview in the process.
    for (where_, capability) in capabilities() {
        let mut named = 0_usize;
        for field in ["windows", "webviews"] {
            let Some(labels) = capability[field].as_array() else {
                continue;
            };
            for label in labels {
                let label = label.as_str().unwrap_or_else(|| {
                    panic!("{where_}: `{field}` must hold literal labels, found {label}")
                });
                assert!(
                    !label.contains(GLOB_METACHARACTERS),
                    "{where_}: `{field}` label `{label}` is a glob pattern, not the literal \
                     label it reads as; Tauri applies the capability to everything it matches",
                );
                named += 1;
            }
        }
        assert!(
            named > 0,
            "{where_}: a capability naming neither a window nor a webview grants nothing, \
             and one naming them only by pattern grants more than it says",
        );
    }
}
