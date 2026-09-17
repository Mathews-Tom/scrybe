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
//! HTTP client at all, which is the build the default gate runs. The
//! desktop host enables `model-download` by default and therefore
//! ships the transport; this crate leaves it off so that the library
//! and command-line graphs the egress baselines read stay clean.
//!
//! [`UnavailableSource`] is what a build without the feature resolves
//! to. It fails at [`ModelSource::open`], which is after the
//! confirmation gate, so a build with no transport still refuses an
//! unconfirmed request for the same reason a build with one does.
//!
//! Every implementation is expected to bound how long a single read may
//! block. The manager checks cancellation between chunks, so a chunk
//! that never arrives is a chunk during which a cancellation cannot be
//! observed and the install cannot resolve at all. That is not a
//! hypothetical: a peer that accepts the connection and then stops
//! sending without closing it — a half-open connection after a dropped
//! network, a stalled edge — parks the read forever unless the
//! transport says otherwise.

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
    use std::time::Duration;

    use super::{ArtifactChunks, ModelSource};
    use crate::error::{ApplicationError, ErrorCode};
    use crate::Result;
    use async_trait::async_trait;

    /// How long the client waits for a connection to be established.
    const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);

    /// How long the client waits between two consecutive reads before
    /// it gives up on the stream.
    ///
    /// Deliberately not a *total* timeout. A half-gigabyte artifact
    /// legitimately takes minutes on a slow line, and a total timeout
    /// would abandon a download that was making steady progress. What
    /// has to be bounded is silence, not duration: a peer that is still
    /// sending resets this on every chunk, and a peer that has stopped
    /// does not.
    const READ_TIMEOUT: Duration = Duration::from_secs(30);

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
            Self::with_timeouts(CONNECT_TIMEOUT, READ_TIMEOUT)
        }

        /// A source whose client gives up after `connect` without a
        /// connection, or after `read` without a byte.
        ///
        /// [`Self::new`] is the production constructor and supplies
        /// figures sized for a half-gigabyte artifact over a real
        /// network. This exists so a test can assert the abandonment
        /// happens at all without waiting those figures out.
        ///
        /// # Errors
        ///
        /// [`ErrorCode::ModelDownloadUnavailable`] when the platform
        /// TLS stack will not initialise.
        pub fn with_timeouts(connect: Duration, read: Duration) -> Result<Self> {
            let client = reqwest::Client::builder()
                .connect_timeout(connect)
                .read_timeout(read)
                .build()
                .map_err(|source| {
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
