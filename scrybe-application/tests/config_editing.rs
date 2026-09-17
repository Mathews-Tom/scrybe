// Copyright 2026 Mathews Tom
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//     https://www.apache.org/licenses/LICENSE-2.0

//! Configuration shapes the editor refuses rather than rewrites.
//!
//! Both live outside `src/` because each needs a real file on disk to
//! assert that a refused edit left it byte-identical, which is the
//! property that matters: `ConfigService::apply` validates only the
//! candidate document, never the one it parsed, so a mangling the
//! editor performed silently would be accepted and written.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use pretty_assertions::assert_eq;
use scrybe_application::config::{ConfigField, ConfigUpdate};
use scrybe_application::{ConfigService, ErrorCode};

fn refuse(body: &str) -> (ErrorCode, String, String) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    std::fs::write(&path, body).unwrap();

    let error = ConfigService::new(&path)
        .apply(&ConfigUpdate::new().set(ConfigField::StorageRoot, "~/moved"))
        .unwrap_err();

    let after = std::fs::read_to_string(&path).unwrap();
    (error.code(), error.to_string(), after)
}

#[test]
fn test_editing_a_key_that_holds_a_sub_table_is_refused() {
    // `existing.as_value()` is `None` for an `Item::Table`, so the
    // decor-restore branch was skipped and the assignment replaced the
    // whole block: this document rendered back as `[storage]\nroot=
    // "~/moved"` with `nested`, the comment, and the spacing gone.
    let body = "[storage]\n# why the root is split out\n[storage.root]\nnested = 1\n";

    let (code, message, after) = refuse(body);

    assert_eq!(code, ErrorCode::ConfigInvalid);
    assert!(message.contains("root"), "the key is not named: {message}");
    assert_eq!(after, body);
}

#[test]
fn test_editing_a_table_written_with_dotted_keys_is_refused() {
    // `toml_edit` 0.20.2 carries the parent key's decor into every
    // dotted path it encodes, so the comment is re-emitted above each
    // dotted sibling. That reproduces on an untouched parse-then-render,
    // so the cause is upstream — but `apply` is what would write the
    // duplicated text back over the user's file.
    let body = "# storage settings\nstorage.root = \"~/scrybe\"\nstorage.audio_format = \"opus\"\n";

    let (code, message, after) = refuse(body);

    assert_eq!(code, ErrorCode::ConfigInvalid);
    assert!(
        message.contains("dotted"),
        "the refusal does not name the cause: {message}"
    );
    assert_eq!(after, body);
}
