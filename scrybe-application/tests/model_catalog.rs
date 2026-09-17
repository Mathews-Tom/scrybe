// Copyright 2026 Mathews Tom
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//     https://www.apache.org/licenses/LICENSE-2.0

//! What the checked-in catalog must be, and what it must refuse.
//!
//! Every field in `models.toml` is evidence a reviewer is being asked
//! to trust, so the rules that make an entry shippable are asserted
//! against the real catalog rather than described in a comment beside
//! it. The mutations here — a mutable URL, a traversing destination, a
//! forty-character checksum that is not a SHA-256 — are the ways a
//! plausible-looking entry actually goes wrong.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::path::Path;

use scrybe_application::models::{self, ModelManifest};
use scrybe_application::ErrorCode;

const MODEL_ID: &str = "whisper-small-en";

fn shipped() -> ModelManifest {
    models::find(MODEL_ID).unwrap().clone()
}

#[test]
fn test_the_checked_in_catalog_parses_and_every_entry_is_shippable() {
    let catalog = models::catalog().unwrap();

    assert!(!catalog.is_empty());
    for manifest in catalog {
        manifest.validate_as_catalog_entry().unwrap();
    }
}

#[test]
fn test_a_mutable_source_url_is_not_shippable_though_it_is_still_actionable() {
    let mut manifest = shipped();
    manifest.source_url =
        "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-small.en.bin".into();

    assert_eq!(
        manifest.validate_as_catalog_entry().unwrap_err().code(),
        ErrorCode::ModelManifestInvalid
    );
    // A mutable URL serves bytes the digest can still judge. What it is
    // not is evidence, which is why only the catalog rule refuses it.
    manifest.validate().unwrap();
}

#[test]
fn test_a_plaintext_source_is_not_shippable() {
    let mut manifest = shipped();
    manifest.source_url = manifest.source_url.replacen("https://", "http://", 1);

    assert_eq!(
        manifest.validate_as_catalog_entry().unwrap_err().code(),
        ErrorCode::ModelManifestInvalid
    );
}

#[test]
fn test_a_revision_that_is_not_forty_hexadecimal_characters_is_not_shippable() {
    let mut manifest = shipped();
    manifest.source_revision = "main".into();

    assert_eq!(
        manifest.validate_as_catalog_entry().unwrap_err().code(),
        ErrorCode::ModelManifestInvalid
    );
}

#[test]
fn test_a_destination_that_escapes_the_models_directory_is_refused() {
    for escape in [
        "../ggml-small.en.bin",
        "nested/ggml-small.en.bin",
        "/etc/passwd",
        "..",
        "",
    ] {
        let mut manifest = shipped();
        manifest.destination = escape.into();

        assert_eq!(
            manifest.validate().unwrap_err().code(),
            ErrorCode::ModelManifestInvalid,
            "{escape:?} was accepted as a destination"
        );
    }
}

#[test]
fn test_a_digest_that_is_not_a_lowercase_sha256_is_refused() {
    // The upstream repository's README carries a forty-character legacy
    // download-script checksum for this very file. It is not a SHA-256,
    // and mistaking one for the other is the single most plausible way
    // a wrong digest reaches this catalog.
    for wrong in [
        "1be3a9b2063867b937e64e2ec7483364a79917e1",
        "C6138D6D58ECC8322097E0F987C32F1BE8BB0A18532A3F88F734D1BBF9C41E5D",
        "not a digest",
        "",
    ] {
        let mut manifest = shipped();
        manifest.sha256 = wrong.into();

        assert_eq!(
            manifest.validate().unwrap_err().code(),
            ErrorCode::ModelManifestInvalid,
            "{wrong:?} was accepted as a digest"
        );
    }
}

#[test]
fn test_a_zero_size_entry_is_refused() {
    let mut manifest = shipped();
    manifest.size_bytes = 0;

    assert_eq!(
        manifest.validate().unwrap_err().code(),
        ErrorCode::ModelManifestInvalid
    );
}

#[test]
fn test_an_entry_naming_no_licence_is_refused() {
    let mut manifest = shipped();
    manifest.license = "   ".into();

    assert_eq!(
        manifest.validate().unwrap_err().code(),
        ErrorCode::ModelManifestInvalid
    );
}

#[test]
fn test_an_unknown_identity_is_reported_as_unknown_rather_than_invalid() {
    assert_eq!(
        models::find("no-such-model").unwrap_err().code(),
        ErrorCode::ModelUnknown
    );
}

#[test]
fn test_the_destination_resolves_directly_under_the_models_directory() {
    let resolved = shipped().destination_under(Path::new("/models"));

    assert_eq!(resolved.parent(), Some(Path::new("/models")));
}

#[test]
fn test_the_install_requirement_exceeds_the_artifact_itself() {
    let manifest = shipped();

    assert!(manifest.required_bytes() > manifest.size_bytes);
}
