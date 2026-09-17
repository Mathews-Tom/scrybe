// Copyright 2026 Mathews Tom
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//     https://www.apache.org/licenses/LICENSE-2.0

//! The managed model catalog, and acquiring what it describes.
//!
//! Two things are separated here on purpose. The catalog is checked-in
//! evidence — a URL pinned to a revision, a licence, an exact byte
//! count, a digest — and lives in `models.toml` beside this crate's
//! manifest. The manager is the machinery that refuses to install
//! anything that does not match it.
//!
//! Nothing in this module resolves a platform directory. The manager
//! is handed the directory models live in, exactly as `ConfigService`
//! is handed its file, so a disposable run redirects model storage
//! through the configuration it already writes rather than through an
//! environment variable invented for the purpose.

mod contract;
mod manager;
mod manifest;
mod source;
mod space;

pub use contract::{
    DownloadProgress, InstallReport, ModelConfirmation, ModelFailure, ModelPlan, ModelState,
    StaleModelPartial,
};
pub use manager::{ModelManager, PARTIAL_SUFFIX};
pub use manifest::{catalog, find, ModelManifest, FREE_SPACE_HEADROOM_BYTES};
pub use source::{ArtifactChunks, ModelSource, UnavailableSource};

#[cfg(feature = "model-download")]
pub use source::HttpModelSource;
