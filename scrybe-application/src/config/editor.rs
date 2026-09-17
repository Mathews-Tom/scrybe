// Copyright 2026 Mathews Tom
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//     https://www.apache.org/licenses/LICENSE-2.0

//! Structure-preserving edits to a configuration document.
//!
//! A user's `config.toml` is a hand-written file. It carries comments
//! explaining why a provider was chosen, advanced blocks no settings
//! screen exposes, and an ordering its author picked. Round-tripping it
//! through `serde` would destroy all three: `Config` is a strict schema
//! that knows nothing about comments and serializes fields in
//! declaration order.
//!
//! This module edits the document instead of rewriting it. Only the
//! keys an update names are touched; each touched key keeps the
//! whitespace and comments that surrounded its old value. Everything
//! else — including blocks this layer has no concept of — is rendered
//! back byte-for-byte.

use toml_edit::{Document, Item, Table};

use crate::config::contract::{ConfigField, ConfigUpdate, ConfigValue};
use crate::error::{ApplicationError, ErrorCode};
use crate::Result;

/// Applies `update` to `text`, returning the edited document.
///
/// Performs no validation of the result beyond TOML well-formedness and
/// per-field value kinds; whether the document is a valid complete
/// configuration is the caller's check, made before anything is
/// written.
///
/// # Errors
///
/// [`ErrorCode::ConfigUnreadable`] if `text` is not well-formed TOML,
/// [`ErrorCode::ConfigInvalid`] if a field's value has the wrong kind
/// or the field's table is occupied by something that is not a table.
pub fn apply(text: &str, update: &ConfigUpdate) -> Result<String> {
    let mut document: Document = text.parse().map_err(|source: toml_edit::TomlError| {
        ApplicationError::new(
            ErrorCode::ConfigUnreadable,
            "the existing configuration file is not well-formed TOML",
        )
        .with_source(source)
    })?;

    for (field, value) in update.entries() {
        check_kind(field, value)?;
        let table = table_mut(&mut document, field.table())?;
        set_preserving_decor(table, field.key(), to_item(value));
    }

    Ok(document.to_string())
}

fn check_kind(field: ConfigField, value: &ConfigValue) -> Result<()> {
    if value.kind() == field.kind() {
        return Ok(());
    }
    Err(ApplicationError::new(
        ErrorCode::ConfigInvalid,
        format!(
            "[{}].{} expects a {:?} value",
            field.table(),
            field.key(),
            field.kind()
        ),
    ))
}

fn table_mut<'doc>(document: &'doc mut Document, name: &str) -> Result<&'doc mut Table> {
    let entry = document
        .as_table_mut()
        .entry(name)
        .or_insert_with(|| Item::Table(Table::new()));
    let table = entry.as_table_mut().ok_or_else(|| {
        ApplicationError::new(
            ErrorCode::ConfigInvalid,
            format!("[{name}] is not a table in the existing configuration file"),
        )
    })?;
    // A table created here is implicit until something renders it, and
    // an implicit table with keys would be written as dotted keys at
    // the top level rather than as the `[table]` header the schema
    // documents.
    table.set_implicit(false);
    Ok(table)
}

/// Replaces `key`'s value while keeping the whitespace and comments
/// that surrounded the old one.
fn set_preserving_decor(table: &mut Table, key: &str, replacement: Item) {
    let decor = table
        .get(key)
        .and_then(Item::as_value)
        .map(|value| value.decor().clone());
    table.insert(key, replacement);
    if let Some(decor) = decor {
        if let Some(value) = table.get_mut(key).and_then(Item::as_value_mut) {
            *value.decor_mut() = decor;
        }
    }
}

fn to_item(value: &ConfigValue) -> Item {
    match value {
        ConfigValue::Boolean(flag) => toml_edit::value(*flag),
        ConfigValue::Integer(number) => toml_edit::value(*number),
        ConfigValue::Text(text) => toml_edit::value(text.as_str()),
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use crate::config::contract::ConfigValueKind;
    use pretty_assertions::assert_eq;

    const HAND_WRITTEN: &str = r#"schema_version = 1

# Everything lives under one root so a backup tool has one target.
[storage]
root = "~/scrybe"   # keep meetings on the internal disk
audio_format = "opus"

[llm]
provider = "openai-compat"
base_url = "http://127.0.0.1:11434/v1"
model = "qwen3:8b"
# Named environment variable, never the credential itself.
api_key_env = "SCRYBE_LLM_KEY"

# Advanced: this layer has no concept of webhooks.
[hooks.webhook]
url = "http://127.0.0.1:9000/scrybe"
timeout_ms = 3000
"#;

    #[test]
    fn test_editing_one_key_preserves_every_comment_in_the_document() {
        let edited = apply(
            HAND_WRITTEN,
            &ConfigUpdate::new().set(ConfigField::LlmModel, "llama3.3"),
        )
        .unwrap();

        assert!(edited.contains("# Everything lives under one root"));
        assert!(edited.contains("# keep meetings on the internal disk"));
        assert!(edited.contains("# Named environment variable, never the credential itself."));
        assert!(edited.contains("# Advanced: this layer has no concept of webhooks."));
    }

    #[test]
    fn test_editing_one_key_preserves_a_block_this_layer_does_not_model() {
        let edited = apply(
            HAND_WRITTEN,
            &ConfigUpdate::new().set(ConfigField::LlmModel, "llama3.3"),
        )
        .unwrap();

        assert!(edited.contains("[hooks.webhook]"));
        assert!(edited.contains("url = \"http://127.0.0.1:9000/scrybe\""));
        assert!(edited.contains("timeout_ms = 3000"));
    }

    #[test]
    fn test_editing_changes_only_the_keys_the_update_names() {
        let edited = apply(
            HAND_WRITTEN,
            &ConfigUpdate::new().set(ConfigField::LlmModel, "llama3.3"),
        )
        .unwrap();

        let changed: Vec<&str> = HAND_WRITTEN
            .lines()
            .zip(edited.lines())
            .filter(|(before, after)| before != after)
            .map(|(_, after)| after)
            .collect();

        assert_eq!(changed, vec![r#"model = "llama3.3""#]);
    }

    #[test]
    fn test_an_edited_key_keeps_its_trailing_comment() {
        let edited = apply(
            HAND_WRITTEN,
            &ConfigUpdate::new().set(ConfigField::StorageRoot, "~/meetings"),
        )
        .unwrap();

        assert!(edited.contains(r#"root = "~/meetings"   # keep meetings on the internal disk"#));
    }

    #[test]
    fn test_a_credential_key_is_never_touched_by_any_update() {
        let mut update = ConfigUpdate::new();
        for field in crate::config::EDITABLE_FIELDS {
            update = match field.kind() {
                ConfigValueKind::Text => update.set(field, "changed"),
                ConfigValueKind::Integer => update.set(field, 64_u32),
                ConfigValueKind::Boolean => update.set(field, true),
            };
        }

        let edited = apply(HAND_WRITTEN, &update).unwrap();

        assert!(edited.contains(r#"api_key_env = "SCRYBE_LLM_KEY""#));
        assert_eq!(edited.matches("api_key").count(), 1);
    }

    #[test]
    fn test_setting_a_key_in_an_absent_table_writes_the_documented_header() {
        let edited = apply(
            "schema_version = 1\n",
            &ConfigUpdate::new().set(ConfigField::AgentAccessEnabled, true),
        )
        .unwrap();

        assert!(edited.contains("[agent_access]"));
        assert!(edited.contains("enabled = true"));
    }

    #[test]
    fn test_a_value_of_the_wrong_kind_is_refused_before_anything_is_rendered() {
        let update = ConfigUpdate::new().set(ConfigField::AgentAccessEnabled, "yes");

        let error = apply(HAND_WRITTEN, &update).unwrap_err();

        assert_eq!(error.code(), ErrorCode::ConfigInvalid);
    }

    #[test]
    fn test_a_malformed_document_is_refused_rather_than_rewritten() {
        let error = apply(
            "[storage\nroot = \"~/scrybe\"\n",
            &ConfigUpdate::new().set(ConfigField::StorageRoot, "~/meetings"),
        )
        .unwrap_err();

        assert_eq!(error.code(), ErrorCode::ConfigUnreadable);
    }

    #[test]
    fn test_an_empty_update_returns_the_document_unchanged() {
        let edited = apply(HAND_WRITTEN, &ConfigUpdate::new()).unwrap();

        assert_eq!(edited, HAND_WRITTEN);
    }
}
