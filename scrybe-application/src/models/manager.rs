// Copyright 2026 Mathews Tom
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//     https://www.apache.org/licenses/LICENSE-2.0

//! Acquiring a model, and everything that must be true before a byte
//! of it is requested.
//!
//! The order is the contract. [`ModelManager::plan`] reads the
//! catalog, looks at the destination, and measures the filesystem; it
//! opens no connection, so a user can be shown the source, revision,
//! licence, exact size, digest, destination, and disk requirement
//! without anything having been fetched. [`ModelManager::install`]
//! then refuses outright unless it is handed a [`ModelConfirmation`]
//! naming that same model and that same digest — and it refuses
//! *before* it asks the source for anything, which is what makes the
//! zero-request property testable rather than merely intended.
//!
//! After that the sequence is: free space, unique `.partial`, stream
//! with cancellation checked between chunks, exact size, exact digest,
//! rename. Every failure and every cancellation leaves the partial
//! where it is and the destination untouched, so nothing half-written
//! is ever reachable under the name a runtime would load.
//!
//! Two refusals are deliberate and not fallbacks. An installed
//! artifact that already matches the manifest is reported `Ready`
//! without a request — a valid model is never re-fetched. An installed
//! file that does *not* match is reported and left alone — replacing
//! it is the user's decision, and a manager that silently overwrote it
//! would be indistinguishable from one that lost their data.

use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use scrybe_core::storage::full_fsync;
use sha2::{Digest, Sha256};

use crate::cancellation::CancellationToken;
use crate::error::{ApplicationError, ErrorCode};
use crate::models::contract::{
    DownloadProgress, InstallReport, ModelConfirmation, ModelFailure, ModelPlan, ModelState,
    StaleModelPartial,
};
use crate::models::manifest::{self, ModelManifest};
use crate::models::source::{ArtifactChunks, ModelSource, UnavailableSource};
use crate::models::space;
use crate::Result;

/// The suffix an in-flight download carries. A file under this name is
/// never loaded by a runtime and never promoted without verification.
pub const PARTIAL_SUFFIX: &str = ".partial";

/// How often a caller's progress callback is invoked, in bytes. A
/// callback per chunk would be a callback per few kilobytes, which is
/// a frontend event storm for a half-gigabyte artifact.
const PROGRESS_STRIDE_BYTES: u64 = 4 * 1024 * 1024;

/// Installs and reports on managed models under one directory.
///
/// The directory is handed in rather than resolved here, exactly as
/// `ConfigService` takes its file and `DiagnosticsService` takes its
/// root. The composition root decides where models live; a disposable
/// run therefore redirects them by pointing the configuration
/// somewhere disposable, with no environment variable of its own.
pub struct ModelManager {
    models_dir: PathBuf,
    source: Arc<dyn ModelSource>,
    /// Counts `.partial` files this process has created, so two
    /// concurrent attempts at the same model cannot pick the same
    /// name even within one second.
    attempts: AtomicU64,
}

impl ModelManager {
    /// A manager over `models_dir`, using this build's transport.
    #[must_use]
    pub fn new(models_dir: impl Into<PathBuf>) -> Self {
        Self::with_source(models_dir, default_source())
    }

    /// A manager over `models_dir`, reading artifacts from `source`.
    #[must_use]
    pub fn with_source(models_dir: impl Into<PathBuf>, source: Arc<dyn ModelSource>) -> Self {
        Self {
            models_dir: models_dir.into(),
            source,
            attempts: AtomicU64::new(0),
        }
    }

    /// Where verified artifacts are promoted to.
    #[must_use]
    pub fn models_dir(&self) -> &Path {
        &self.models_dir
    }

    /// Every model the catalog offers, with the state of each.
    ///
    /// # Errors
    ///
    /// As [`Self::plan`].
    pub fn catalog(&self) -> Result<Vec<ModelPlan>> {
        manifest::catalog()?
            .iter()
            .map(|entry| self.plan_for(entry))
            .collect()
    }

    /// What acquiring `id` would involve, and what is already there.
    ///
    /// Reads the catalog, the destination, and the filesystem. Opens
    /// nothing: this is the call whose output a confirmation prompt is
    /// built from, so it must be safe to make before the user has
    /// agreed to anything.
    ///
    /// # Errors
    ///
    /// [`ErrorCode::ModelUnknown`] for an identity the catalog does not
    /// carry, and [`ErrorCode::ModelManifestInvalid`] when the catalog
    /// itself is unusable.
    pub fn plan(&self, id: &str) -> Result<ModelPlan> {
        self.plan_for(manifest::find(id)?)
    }

    /// What acquiring `manifest` would involve, for a caller that
    /// already holds one — from [`Self::catalog`], or in a test that
    /// describes an artifact small enough to actually produce.
    ///
    /// # Errors
    ///
    /// [`ErrorCode::ModelManifestInvalid`] when `manifest` is not one
    /// the manager can act on.
    pub fn plan_for(&self, manifest: &ModelManifest) -> Result<ModelPlan> {
        manifest.validate()?;
        let destination = manifest.destination_under(&self.models_dir);
        let required = manifest.required_bytes();
        let available = space::available_bytes(&self.models_dir).or_else(|| {
            // A models directory that does not exist yet has no
            // statistics of its own; the filesystem it would be
            // created on does.
            self.models_dir.parent().and_then(space::available_bytes)
        });
        Ok(ModelPlan {
            id: manifest.id.clone(),
            source_url: manifest.source_url.clone(),
            source_revision: manifest.source_revision.clone(),
            license: manifest.license.clone(),
            size_bytes: manifest.size_bytes,
            sha256: manifest.sha256.clone(),
            runtime: manifest.runtime.clone(),
            destination: manifest.destination.clone(),
            destination_path: destination.display().to_string(),
            required_bytes: required,
            available_bytes: available,
            // An unknown figure does not block: refusing every install
            // on a platform that will not answer would be a worse
            // failure than letting the write itself report a full disk.
            sufficient_space: available.is_none_or(|bytes| bytes >= required),
            state: self.installed_state(manifest)?,
        })
    }

    /// Every `.partial` left under the models directory.
    ///
    /// Reported, never removed. A partial may be the tail of a
    /// download the user still wants, and deleting one is a mutation
    /// this call has no mandate for.
    ///
    /// # Errors
    ///
    /// [`ErrorCode::ModelStorageUnavailable`] when the directory exists
    /// but cannot be enumerated. A directory that does not exist yet
    /// holds no partials and is not an error.
    pub fn stale_partials(&self) -> Result<Vec<StaleModelPartial>> {
        if !self.models_dir.is_dir() {
            return Ok(Vec::new());
        }
        let entries = std::fs::read_dir(&self.models_dir).map_err(|source| {
            ApplicationError::new(
                ErrorCode::ModelStorageUnavailable,
                format!(
                    "the models directory {} could not be read",
                    self.models_dir.display()
                ),
            )
            .with_source(source)
        })?;
        let mut partials: Vec<StaleModelPartial> = entries
            .flatten()
            .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_file()))
            .filter_map(|entry| {
                let name = entry.file_name().to_str()?.to_string();
                if !name.ends_with(PARTIAL_SUFFIX) {
                    return None;
                }
                Some(StaleModelPartial {
                    name,
                    bytes: entry.metadata().map(|meta| meta.len()).unwrap_or_default(),
                })
            })
            .collect();
        partials.sort_by(|left, right| left.name.cmp(&right.name));
        Ok(partials)
    }

    /// Acquires `id`, having been told the user agreed to it.
    ///
    /// `progress` is called as bytes arrive, at a coarse stride.
    /// `cancel` is checked between chunks; a cancelled download leaves
    /// its `.partial` in place and promotes nothing.
    ///
    /// # Errors
    ///
    /// [`ErrorCode::ModelConfirmationRequired`] when `confirmation`
    /// does not name this model and the digest the catalog offers —
    /// raised before the source is opened. [`ErrorCode::ModelUnknown`],
    /// [`ErrorCode::ModelManifestInvalid`], and
    /// [`ErrorCode::ModelStorageUnavailable`] as elsewhere. Everything
    /// else — a short artifact, a wrong digest, a transport failure, a
    /// full disk, a cancellation — is an outcome in the returned
    /// [`InstallReport`], not an error, because each of them is a
    /// state the user is shown and can act on.
    pub async fn install(
        &self,
        id: &str,
        confirmation: &ModelConfirmation,
        cancel: &CancellationToken,
        progress: &(dyn Fn(DownloadProgress) + Send + Sync),
    ) -> Result<InstallReport> {
        self.install_manifest(manifest::find(id)?, confirmation, cancel, progress)
            .await
    }

    /// Acquires `manifest`, for a caller that already holds one.
    ///
    /// # Errors
    ///
    /// As [`Self::install`].
    pub async fn install_manifest(
        &self,
        manifest: &ModelManifest,
        confirmation: &ModelConfirmation,
        cancel: &CancellationToken,
        progress: &(dyn Fn(DownloadProgress) + Send + Sync),
    ) -> Result<InstallReport> {
        manifest.validate()?;

        // The gate. Everything above this line reads checked-in data;
        // nothing below it may run for a request the user did not
        // agree to. `tests/model_acquisition.rs` asserts the source
        // records zero opens when this refuses.
        let names_this_model = confirmation.id == manifest.id;
        let acknowledges_this_artifact = confirmation.acknowledged_sha256 == manifest.sha256;
        if !(names_this_model && acknowledges_this_artifact) {
            return Err(ApplicationError::new(
                ErrorCode::ModelConfirmationRequired,
                format!(
                    "model {:?} has not been confirmed for the artifact on offer",
                    manifest.id
                ),
            ));
        }

        match self.installed_state(manifest)? {
            ModelState::Ready => {
                return Ok(InstallReport {
                    id: manifest.id.clone(),
                    state: ModelState::Ready,
                    promoted: false,
                })
            }
            state @ ModelState::Failed { .. } => {
                return Ok(InstallReport {
                    id: manifest.id.clone(),
                    state,
                    promoted: false,
                })
            }
            _ => {}
        }

        self.ensure_directory()?;

        let required = manifest.required_bytes();
        if let Some(available) = space::available_bytes(&self.models_dir) {
            if available < required {
                return Ok(failed(
                    manifest,
                    ModelFailure::InsufficientSpace {
                        required,
                        available,
                    },
                ));
            }
        }

        if cancel.is_cancelled() {
            return Ok(InstallReport {
                id: manifest.id.clone(),
                state: ModelState::Cancelled,
                promoted: false,
            });
        }

        self.fetch_and_promote(manifest, cancel, progress).await
    }

    // -- internals ---------------------------------------------------

    /// What the destination already holds, hashing it when it is the
    /// right length to be the artifact.
    fn installed_state(&self, manifest: &ModelManifest) -> Result<ModelState> {
        let destination = manifest.destination_under(&self.models_dir);
        let Ok(metadata) = std::fs::metadata(&destination) else {
            return Ok(ModelState::Available);
        };
        if !metadata.is_file() {
            return Ok(ModelState::Failed {
                reason: ModelFailure::InstalledArtifactUnrecognized {
                    summary: format!("{} exists and is not a file", destination.display()),
                },
            });
        }
        if metadata.len() != manifest.size_bytes {
            return Ok(ModelState::Failed {
                reason: ModelFailure::InstalledArtifactUnrecognized {
                    summary: format!(
                        "{} is {} bytes where the catalog describes {}",
                        destination.display(),
                        metadata.len(),
                        manifest.size_bytes
                    ),
                },
            });
        }
        let observed = digest_of(&destination)?;
        if observed == manifest.sha256 {
            return Ok(ModelState::Ready);
        }
        Ok(ModelState::Failed {
            reason: ModelFailure::InstalledArtifactUnrecognized {
                summary: format!(
                    "{} is the right length but hashes to {observed}",
                    destination.display()
                ),
            },
        })
    }

    fn ensure_directory(&self) -> Result<()> {
        std::fs::create_dir_all(&self.models_dir).map_err(|source| {
            ApplicationError::new(
                ErrorCode::ModelStorageUnavailable,
                format!(
                    "the models directory {} could not be created",
                    self.models_dir.display()
                ),
            )
            .with_source(source)
        })
    }

    /// A `.partial` name no concurrent attempt can collide with.
    fn partial_path(&self, manifest: &ModelManifest) -> PathBuf {
        let attempt = self.attempts.fetch_add(1, Ordering::SeqCst);
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |since| since.as_nanos());
        self.models_dir.join(format!(
            "{}.{}-{attempt}-{stamp}{PARTIAL_SUFFIX}",
            manifest.destination,
            std::process::id(),
        ))
    }

    async fn fetch_and_promote(
        &self,
        manifest: &ModelManifest,
        cancel: &CancellationToken,
        progress: &(dyn Fn(DownloadProgress) + Send + Sync),
    ) -> Result<InstallReport> {
        let partial = self.partial_path(manifest);
        let mut chunks = match self.source.open(&manifest.source_url).await {
            Ok(chunks) => chunks,
            Err(error) => {
                return Ok(failed(
                    manifest,
                    ModelFailure::Transport {
                        summary: error.message().to_string(),
                    },
                ))
            }
        };

        let mut file = match std::fs::File::create(&partial) {
            Ok(file) => file,
            Err(source) => {
                return Ok(failed(
                    manifest,
                    ModelFailure::Storage {
                        summary: format!("{} could not be created: {source}", partial.display()),
                    },
                ))
            }
        };

        Ok(
            match stream_into(
                manifest,
                chunks.as_mut(),
                &mut file,
                &partial,
                cancel,
                progress,
            )
            .await
            {
                Streamed::Complete { digest } => {
                    verify_and_promote(manifest, &partial, &digest, &file)
                }
                Streamed::Stopped(report) => report,
            },
        )
    }
}

/// Commits the verified partial under the destination name.
fn verify_and_promote(
    manifest: &ModelManifest,
    partial: &Path,
    digest: &str,
    file: &std::fs::File,
) -> InstallReport {
    if digest != manifest.sha256 {
        return failed(
            manifest,
            ModelFailure::DigestMismatch {
                expected: manifest.sha256.clone(),
                observed: digest.to_string(),
            },
        );
    }

    // Durable before it is reachable. A rename that outran the data
    // would leave a file under the name a runtime loads whose tail had
    // never reached the disk.
    if let Err(source) = full_fsync(file) {
        return failed(
            manifest,
            ModelFailure::Storage {
                summary: format!("{} could not be committed: {source}", partial.display()),
            },
        );
    }

    let destination =
        manifest.destination_under(partial.parent().unwrap_or_else(|| Path::new(".")));
    if let Err(source) = std::fs::rename(partial, &destination) {
        return failed(
            manifest,
            ModelFailure::Storage {
                summary: format!(
                    "{} could not be promoted to {}: {source}",
                    partial.display(),
                    destination.display()
                ),
            },
        );
    }

    InstallReport {
        id: manifest.id.clone(),
        state: ModelState::Ready,
        promoted: true,
    }
}

/// How streaming ended.
enum Streamed {
    /// Every byte arrived and the count matched. Carries the digest.
    Complete { digest: String },
    /// It did not get that far; the report says why.
    Stopped(InstallReport),
}

/// Writes the artifact into `file`, hashing as it goes.
///
/// Cancellation is checked between chunks, and an artifact longer than
/// the manifest is stopped as soon as it overruns rather than after
/// the rest of it has been paid for. Every early return leaves the
/// partial exactly where it is: nothing here promotes anything.
async fn stream_into(
    manifest: &ModelManifest,
    chunks: &mut dyn ArtifactChunks,
    file: &mut std::fs::File,
    partial: &Path,
    cancel: &CancellationToken,
    progress: &(dyn Fn(DownloadProgress) + Send + Sync),
) -> Streamed {
    let mut hasher = Sha256::new();
    let mut received: u64 = 0;
    let mut announced: u64 = 0;
    loop {
        if cancel.is_cancelled() {
            // The partial stays. It is reported by `stale_partials`,
            // and removing it is the user's call, not a cleanup this
            // path performs behind them.
            return Streamed::Stopped(InstallReport {
                id: manifest.id.clone(),
                state: ModelState::Cancelled,
                promoted: false,
            });
        }
        let chunk = match chunks.next_chunk().await {
            Ok(Some(chunk)) => chunk,
            Ok(None) => break,
            Err(error) => {
                return Streamed::Stopped(failed(
                    manifest,
                    ModelFailure::Transport {
                        summary: error.message().to_string(),
                    },
                ))
            }
        };
        // An artifact longer than the manifest is stopped as it
        // overruns rather than after the rest of it: the size is
        // already known to be wrong.
        received = received.saturating_add(chunk.len() as u64);
        if received > manifest.size_bytes {
            return Streamed::Stopped(failed(
                manifest,
                ModelFailure::SizeMismatch {
                    expected: manifest.size_bytes,
                    observed: received,
                },
            ));
        }
        hasher.update(&chunk);
        if let Err(source) = file.write_all(&chunk) {
            return Streamed::Stopped(failed(
                manifest,
                ModelFailure::Storage {
                    summary: format!("{} could not be written: {source}", partial.display()),
                },
            ));
        }
        if received - announced >= PROGRESS_STRIDE_BYTES {
            announced = received;
            progress(DownloadProgress {
                received_bytes: received,
                total_bytes: manifest.size_bytes,
            });
        }
    }
    progress(DownloadProgress {
        received_bytes: received,
        total_bytes: manifest.size_bytes,
    });

    if received != manifest.size_bytes {
        return Streamed::Stopped(failed(
            manifest,
            ModelFailure::SizeMismatch {
                expected: manifest.size_bytes,
                observed: received,
            },
        ));
    }
    if let Err(source) = file.flush() {
        return Streamed::Stopped(failed(
            manifest,
            ModelFailure::Storage {
                summary: format!("{} could not be flushed: {source}", partial.display()),
            },
        ));
    }
    Streamed::Complete {
        digest: format!("{:x}", hasher.finalize()),
    }
}

fn failed(manifest: &ModelManifest, reason: ModelFailure) -> InstallReport {
    InstallReport {
        id: manifest.id.clone(),
        state: ModelState::Failed { reason },
        promoted: false,
    }
}

fn default_source() -> Arc<dyn ModelSource> {
    #[cfg(feature = "model-download")]
    {
        match crate::models::source::HttpModelSource::new() {
            Ok(source) => Arc::new(source),
            // A TLS stack that will not initialise is a build whose
            // transport is unavailable, which is a state the manager
            // already models. Reporting it at the first `install`
            // keeps the constructor infallible for every caller that
            // never downloads anything.
            Err(_) => Arc::new(UnavailableSource),
        }
    }
    #[cfg(not(feature = "model-download"))]
    {
        Arc::new(UnavailableSource)
    }
}

/// The SHA-256 of a file, lowercase hexadecimal.
fn digest_of(path: &Path) -> Result<String> {
    let mut file = std::fs::File::open(path).map_err(|source| {
        ApplicationError::new(
            ErrorCode::ModelStorageUnavailable,
            format!("{} could not be read", path.display()),
        )
        .with_source(source)
    })?;
    let mut hasher = Sha256::new();
    std::io::copy(&mut file, &mut hasher).map_err(|source| {
        ApplicationError::new(
            ErrorCode::ModelStorageUnavailable,
            format!("{} could not be hashed", path.display()),
        )
        .with_source(source)
    })?;
    Ok(format!("{:x}", hasher.finalize()))
}
