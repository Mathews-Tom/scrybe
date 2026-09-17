// Copyright 2026 Mathews Tom
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//     https://www.apache.org/licenses/LICENSE-2.0

//! The checked-in model catalog, and what makes an entry usable.
//!
//! The catalog is data, not code: `models.toml` sits beside the
//! manifest and is compiled in with `include_str!`. That keeps the
//! evidence a reviewer has to check — a URL, a revision, a licence, a
//! byte count, a digest — in one file they can read without following
//! a Rust expression, and it keeps [`validate`] honest, because the
//! validator and the data it judges are not the same text.
//!
//! Validation is deliberately strict about two things a plausible-
//! looking entry gets wrong. A `resolve/main` URL names whatever a
//! branch points at today, so no checked-in digest can describe it;
//! the source URL must therefore carry the pinned revision. And a
//! destination is a filename, never a path — a catalog entry cannot be
//! made to write outside the directory the manager was handed.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use serde::{Deserialize, Serialize};

use crate::error::{ApplicationError, ErrorCode};
use crate::Result;

/// The catalog as it is checked in.
const CATALOG_SOURCE: &str = include_str!("../../models.toml");

/// How much room past the artifact itself a download is required to
/// have.
///
/// The verified file and its `.partial` coexist for the moment between
/// the last write and the rename, and a filesystem with nothing left
/// over is a filesystem that fails somewhere less legible than a
/// preflight.
pub const FREE_SPACE_HEADROOM_BYTES: u64 = 256 * 1024 * 1024;

/// One model the application can install, described completely enough
/// to be verified.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ModelManifest {
    /// Stable identity a caller names the model by.
    pub id: String,
    /// The immutable, revision-pinned artifact URL.
    pub source_url: String,
    /// The upstream revision `source_url` is pinned to.
    pub source_revision: String,
    /// The artifact's licence, as the upstream repository states it.
    pub license: String,
    /// The artifact's exact size. A byte more or fewer is a failure.
    pub size_bytes: u64,
    /// The artifact's SHA-256, lowercase hexadecimal.
    pub sha256: String,
    /// The runtime that loads the artifact.
    pub runtime: String,
    /// The filename the verified artifact is promoted to.
    pub destination: String,
}

#[derive(Deserialize)]
struct Catalog {
    model: Vec<ModelManifest>,
}

impl ModelManifest {
    /// Where the verified artifact belongs, under `models_dir`.
    ///
    /// [`Self::validate`] has already refused any destination carrying
    /// a separator, a parent reference, or a root, so the join is a
    /// direct child of `models_dir`.
    #[must_use]
    pub fn destination_under(&self, models_dir: &Path) -> PathBuf {
        models_dir.join(&self.destination)
    }

    /// Free space the install needs, including the headroom the rename
    /// step depends on.
    #[must_use]
    pub const fn required_bytes(&self) -> u64 {
        self.size_bytes.saturating_add(FREE_SPACE_HEADROOM_BYTES)
    }

    /// Whether every field is one the manager can act on.
    ///
    /// This is the *actionability* half: an identity to name it by, a
    /// licence and runtime to show, a non-zero size and a well-formed
    /// digest to verify against, a parseable URL to read, and a
    /// destination that is a filename rather than a path. Integrity is
    /// the digest's job, so nothing here has an opinion about the
    /// transport — a caller holding a manifest it built itself, such
    /// as a test against a loopback fixture, is judged on whether the
    /// manager can act on it and nothing else.
    ///
    /// # Errors
    ///
    /// [`ErrorCode::ModelManifestInvalid`] naming the first rule the
    /// entry broke.
    pub fn validate(&self) -> Result<()> {
        if self.id.trim().is_empty() {
            return Err(self.reject("has no identity"));
        }
        if self.license.trim().is_empty() {
            return Err(self.reject("names no licence"));
        }
        if self.runtime.trim().is_empty() {
            return Err(self.reject("names no runtime"));
        }
        if self.size_bytes == 0 {
            return Err(self.reject("declares a zero byte size"));
        }
        if !is_lowercase_hex(&self.sha256, 64) {
            return Err(self.reject("does not carry a lowercase hexadecimal SHA-256"));
        }
        if url::Url::parse(&self.source_url).is_err() {
            return Err(self.reject("does not carry a parseable source URL"));
        }
        if !is_plain_filename(&self.destination) {
            return Err(self.reject("has a destination that is not a plain filename"));
        }
        Ok(())
    }

    /// Whether the entry is one this application is willing to ship.
    ///
    /// The *provenance* half, and the stricter one. A checked-in entry
    /// is a claim about where an artifact comes from, so it must be
    /// served over HTTPS and must name a revision its URL is actually
    /// pinned to. Neither rule protects the bytes — the digest does
    /// that — and both protect the reviewer, who otherwise has no way
    /// to tell a pinned URL from one that will serve something else
    /// tomorrow.
    ///
    /// # Errors
    ///
    /// [`ErrorCode::ModelManifestInvalid`] naming the first rule the
    /// entry broke, including every rule [`Self::validate`] applies.
    pub fn validate_as_catalog_entry(&self) -> Result<()> {
        self.validate()?;
        if !is_lowercase_hex(&self.source_revision, 40) {
            return Err(self.reject("does not carry a 40-character source revision"));
        }
        let url = url::Url::parse(&self.source_url)
            .map_err(|_| self.reject("does not carry a parseable source URL"))?;
        if url.scheme() != "https" {
            return Err(self.reject("is served over something other than HTTPS"));
        }
        // A URL that does not carry the revision is a mutable URL
        // wearing a pinned one's clothes, and no checked-in digest can
        // describe what it will serve tomorrow.
        if !self.source_url.contains(&self.source_revision) {
            return Err(
                self.reject("has a source URL that is not pinned to its own source revision")
            );
        }
        Ok(())
    }

    fn reject(&self, what: &str) -> ApplicationError {
        ApplicationError::new(
            ErrorCode::ModelManifestInvalid,
            format!("model {:?}: {what}", self.id),
        )
    }
}

/// Every model in the checked-in catalog.
///
/// # Errors
///
/// [`ErrorCode::ModelManifestInvalid`] when the catalog does not parse,
/// carries a duplicate identity or destination, or holds an entry
/// [`ModelManifest::validate`] refuses.
pub fn catalog() -> Result<&'static [ModelManifest]> {
    static PARSED: OnceLock<std::result::Result<Vec<ModelManifest>, String>> = OnceLock::new();
    match PARSED.get_or_init(|| parse(CATALOG_SOURCE).map_err(|error| error.message().to_string()))
    {
        Ok(models) => Ok(models.as_slice()),
        Err(message) => Err(ApplicationError::new(
            ErrorCode::ModelManifestInvalid,
            message.clone(),
        )),
    }
}

/// The catalog entry `id` names.
///
/// # Errors
///
/// [`ErrorCode::ModelManifestInvalid`] as [`catalog`], or
/// [`ErrorCode::ModelUnknown`] when no entry carries `id`.
pub fn find(id: &str) -> Result<&'static ModelManifest> {
    catalog()?
        .iter()
        .find(|manifest| manifest.id == id)
        .ok_or_else(|| {
            ApplicationError::new(
                ErrorCode::ModelUnknown,
                format!("no model in the catalog is named {id:?}"),
            )
        })
}

fn parse(source: &str) -> Result<Vec<ModelManifest>> {
    let catalog: Catalog = toml::from_str(source).map_err(|source| {
        ApplicationError::new(
            ErrorCode::ModelManifestInvalid,
            "the checked-in model catalog is not well-formed TOML",
        )
        .with_source(source)
    })?;
    if catalog.model.is_empty() {
        return Err(ApplicationError::new(
            ErrorCode::ModelManifestInvalid,
            "the checked-in model catalog is empty",
        ));
    }
    for (index, manifest) in catalog.model.iter().enumerate() {
        manifest.validate_as_catalog_entry()?;
        if catalog.model[..index]
            .iter()
            .any(|earlier| earlier.id == manifest.id)
        {
            return Err(ApplicationError::new(
                ErrorCode::ModelManifestInvalid,
                format!("the catalog names {:?} more than once", manifest.id),
            ));
        }
        if catalog.model[..index]
            .iter()
            .any(|earlier| earlier.destination == manifest.destination)
        {
            return Err(ApplicationError::new(
                ErrorCode::ModelManifestInvalid,
                format!("two catalog entries promote to {:?}", manifest.destination),
            ));
        }
    }
    Ok(catalog.model)
}

fn is_lowercase_hex(value: &str, length: usize) -> bool {
    value.len() == length
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn is_plain_filename(value: &str) -> bool {
    !value.is_empty()
        && value != "."
        && value != ".."
        && !value.contains('/')
        && !value.contains('\\')
        && !value.contains('\0')
        && Path::new(value).components().count() == 1
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    /// The one rule that is about the catalog as a whole rather than
    /// about one entry, so it is the one rule `tests/model_catalog.rs`
    /// cannot reach through the public surface.
    #[test]
    fn test_a_duplicate_identity_fails_the_whole_catalog() {
        let duplicated = format!("{CATALOG_SOURCE}\n{CATALOG_SOURCE}");

        assert_eq!(
            parse(&duplicated).unwrap_err().code(),
            ErrorCode::ModelManifestInvalid
        );
    }
}
