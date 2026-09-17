// Copyright 2026 Mathews Tom
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//     https://www.apache.org/licenses/LICENSE-2.0

//! Serializable model-acquisition types.
//!
//! [`ModelPlan`] is the whole of what a user is shown before anything
//! is fetched, and [`ModelConfirmation`] is the only thing that turns
//! that showing into permission. The confirmation carries the digest
//! the user was shown rather than a bare flag, so a caller cannot
//! confirm one artifact and receive another: the manager compares it
//! against the catalog before it opens anything.

use serde::{Deserialize, Serialize};

/// Where one model stands.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "state")]
pub enum ModelState {
    /// Not installed, and nothing in flight.
    Available,
    /// Bytes are arriving.
    Downloading {
        received_bytes: u64,
        total_bytes: u64,
    },
    /// Every byte has arrived; size and digest are being checked.
    Verifying,
    /// Installed, exactly the manifest's bytes.
    Ready,
    /// The user stopped it. Nothing was promoted.
    Cancelled,
    /// It did not complete. Nothing was promoted.
    Failed { reason: ModelFailure },
}

/// Why an acquisition did not produce a usable model.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "failure")]
pub enum ModelFailure {
    /// The bytes that arrived were not the number the manifest states.
    SizeMismatch { expected: u64, observed: u64 },
    /// The bytes that arrived hashed to something else.
    DigestMismatch { expected: String, observed: String },
    /// The filesystem has less room than the install needs.
    InsufficientSpace { required: u64, available: u64 },
    /// The transport failed, or this build has none.
    Transport { summary: String },
    /// The models directory could not be written.
    Storage { summary: String },
    /// A file already occupies the destination and is not the
    /// manifest's artifact. It is left exactly as it is: replacing it
    /// is the user's decision, not the manager's.
    InstalledArtifactUnrecognized { summary: String },
}

/// Everything shown before any model-network access, and the state the
/// destination is already in.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ModelPlan {
    pub id: String,
    pub source_url: String,
    pub source_revision: String,
    pub license: String,
    pub size_bytes: u64,
    pub sha256: String,
    pub runtime: String,
    /// The filename the verified artifact is promoted to.
    pub destination: String,
    /// Where that file is, or would be.
    pub destination_path: String,
    /// The artifact plus the headroom the promotion step needs.
    pub required_bytes: u64,
    /// What the filesystem reports free, or `None` when it will not
    /// say. An unknown figure is reported as unknown rather than as
    /// zero or as plenty.
    pub available_bytes: Option<u64>,
    /// Whether a download may proceed on space grounds.
    pub sufficient_space: bool,
    /// What the destination already holds.
    pub state: ModelState,
}

/// A user's explicit agreement to fetch one specific artifact.
///
/// Constructed from the plan they were shown. The manager compares
/// both fields against the catalog before it opens a connection, so a
/// confirmation for a different model, or one carrying a digest that
/// is not the one on offer, is refused without any network access.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ModelConfirmation {
    pub id: String,
    /// The digest the user was shown in [`ModelPlan::sha256`].
    pub acknowledged_sha256: String,
}

impl ModelConfirmation {
    /// Agreement to fetch `id`, having been shown `acknowledged_sha256`.
    #[must_use]
    pub fn new(id: impl Into<String>, acknowledged_sha256: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            acknowledged_sha256: acknowledged_sha256.into(),
        }
    }

    /// The agreement a caller would build from `plan`.
    #[must_use]
    pub fn from_plan(plan: &ModelPlan) -> Self {
        Self::new(plan.id.clone(), plan.sha256.clone())
    }
}

/// How one acquisition ended.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct InstallReport {
    pub id: String,
    pub state: ModelState,
    /// Whether this call put a verified artifact at the destination.
    /// `false` for a cancellation, a failure, and for a model that was
    /// already installed and already valid.
    pub promoted: bool,
}

impl InstallReport {
    /// Whether the destination now holds the manifest's artifact.
    #[must_use]
    pub fn is_ready(&self) -> bool {
        self.state == ModelState::Ready
    }
}

/// A `.partial` left in the models directory by an earlier attempt.
///
/// Reported, never removed: deleting a partial is a mutation, and a
/// diagnosis that mutated would defeat the point of diagnosing.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct StaleModelPartial {
    pub name: String,
    pub bytes: u64,
}

/// How far a download has got.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct DownloadProgress {
    pub received_bytes: u64,
    pub total_bytes: u64,
}

impl DownloadProgress {
    /// Received over total, clamped to `0.0..=1.0`. `None` when the
    /// total is zero, which the manifest validator already excludes
    /// for a catalog entry but a caller could still construct.
    #[must_use]
    pub fn fraction(self) -> Option<f64> {
        if self.total_bytes == 0 {
            return None;
        }
        #[allow(clippy::cast_precision_loss)]
        Some((self.received_bytes as f64 / self.total_bytes as f64).clamp(0.0, 1.0))
    }
}
