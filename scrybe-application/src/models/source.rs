// Copyright 2026 Mathews Tom
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//     https://www.apache.org/licenses/LICENSE-2.0

//! Where a model's bytes come from.
//!
//! The manager's whole job — preflight, confirmation, cancellation,
//! size and digest verification, atomic promotion, refusing to replace
//! an installed artifact — is about what happens to bytes, not about
//! how they arrive. Keeping arrival behind this trait is what lets
//! every one of those properties be tested in a build that carries no
//! HTTP client at all, which is the build the default gate runs and
//! the build the desktop host ships unless `model-download` is on.
//!
//! [`UnavailableSource`] is what a build without the feature resolves
//! to. It fails at [`ModelSource::open`], which is after the
//! confirmation gate, so a build with no transport still refuses an
//! unconfirmed request for the same reason a build with one does.

use async_trait::async_trait;

use crate::error::{ApplicationError, ErrorCode};
use crate::Result;

/// A model artifact being read, one chunk at a time.
#[async_trait]
pub trait ArtifactChunks: Send {
    /// The next chunk, or `None` at the end of the artifact.
    ///
    /// # Errors
    ///
    /// Whatever the underlying transport reports.
    async fn next_chunk(&mut self) -> Result<Option<Vec<u8>>>;
}

/// Somewhere model artifacts can be read from.
#[async_trait]
pub trait ModelSource: Send + Sync {
    /// Opens `url` for reading.
    ///
    /// Called only after the manager has accepted a confirmation, so an
    /// implementation may treat being called at all as permission.
    ///
    /// # Errors
    ///
    /// Whatever the underlying transport reports.
    async fn open(&self, url: &str) -> Result<Box<dyn ArtifactChunks>>;
}

/// The transport a build without `model-download` resolves to.
pub struct UnavailableSource;

#[async_trait]
impl ModelSource for UnavailableSource {
    async fn open(&self, _url: &str) -> Result<Box<dyn ArtifactChunks>> {
        Err(ApplicationError::new(
            ErrorCode::ModelDownloadUnavailable,
            "this build carries no model transport, so no model can be fetched through it",
        ))
    }
}

#[cfg(feature = "model-download")]
pub use http::HttpModelSource;

#[cfg(feature = "model-download")]
mod http {
    use super::{ArtifactChunks, ModelSource};
    use crate::error::{ApplicationError, ErrorCode};
    use crate::Result;
    use async_trait::async_trait;

    /// Reads a model artifact over HTTPS.
    ///
    /// The client is built once and reused. Redirects are followed
    /// because the pinned artifact URL resolves through one, but the
    /// bytes that arrive are judged by the manifest's size and digest
    /// regardless of where they came from, so a redirect cannot
    /// substitute an artifact.
    pub struct HttpModelSource {
        client: reqwest::Client,
    }

    impl HttpModelSource {
        /// A source over a freshly built client.
        ///
        /// # Errors
        ///
        /// [`ErrorCode::ModelDownloadUnavailable`] when the platform
        /// TLS stack will not initialise.
        pub fn new() -> Result<Self> {
            let client = reqwest::Client::builder().build().map_err(|source| {
                ApplicationError::new(
                    ErrorCode::ModelDownloadUnavailable,
                    "the model transport could not be initialised",
                )
                .with_source(source)
            })?;
            Ok(Self { client })
        }
    }

    struct ResponseChunks {
        response: reqwest::Response,
    }

    #[async_trait]
    impl ArtifactChunks for ResponseChunks {
        async fn next_chunk(&mut self) -> Result<Option<Vec<u8>>> {
            let chunk = self.response.chunk().await.map_err(|source| {
                ApplicationError::new(
                    ErrorCode::ModelDownloadUnavailable,
                    "the model artifact stream failed part way through",
                )
                .with_source(source)
            })?;
            Ok(chunk.map(|bytes| bytes.to_vec()))
        }
    }

    #[async_trait]
    impl ModelSource for HttpModelSource {
        async fn open(&self, url: &str) -> Result<Box<dyn ArtifactChunks>> {
            let response = self.client.get(url).send().await.map_err(|source| {
                ApplicationError::new(
                    ErrorCode::ModelDownloadUnavailable,
                    "the model artifact could not be requested",
                )
                .with_source(source)
            })?;
            let status = response.status();
            if !status.is_success() {
                return Err(ApplicationError::new(
                    ErrorCode::ModelDownloadUnavailable,
                    format!("the model artifact source answered {status}"),
                ));
            }
            Ok(Box::new(ResponseChunks { response }))
        }
    }
}
