// Copyright 2026 Mathews Tom
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//     https://www.apache.org/licenses/LICENSE-2.0

//! The same properties `model_acquisition.rs` asserts against a
//! scripted source, asserted against a real HTTP server.
//!
//! `tests/model_acquisition.rs` proves the manager never asks its
//! source for an unconfirmed artifact. That is the property; this file
//! is the corroboration that the property survives a real transport,
//! by counting what a local fixture server was actually asked for. The
//! server is `wiremock`, the same one `scrybe-core`'s provider tests
//! use, and it binds to loopback: no test here reaches the public
//! artifact host, and the manifest's own URL is never requested.
//!
//! Only compiled when `model-download` is on, because without it there
//! is no HTTP client to exercise.

#![cfg(feature = "model-download")]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::sync::Arc;

use scrybe_application::cancellation::CancellationToken;
use scrybe_application::models::{
    DownloadProgress, HttpModelSource, ModelConfirmation, ModelFailure, ModelManager,
    ModelManifest, ModelState,
};
use scrybe_application::ErrorCode;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

const ARTIFACT: &[u8] = b"a small stand-in for a GGML model file";
const ARTIFACT_PATH: &str = "/repo/resolve/0123456789abcdef0123456789abcdef01234567/model.bin";

fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::Digest as _;
    let mut hasher = sha2::Sha256::new();
    hasher.update(bytes);
    format!("{:x}", hasher.finalize())
}

/// A manifest pointing at `server`, describing `body` exactly.
///
/// The HTTPS requirement the catalog validator enforces is relaxed for
/// the fixture by constructing the manifest directly rather than
/// through the catalog: a loopback fixture server speaks plain HTTP,
/// and the property under test is about request counts, not TLS. The
/// catalog's own scheme requirement is asserted in
/// `tests/model_catalog.rs`.
fn manifest_for(server: &MockServer, body: &[u8]) -> ModelManifest {
    ModelManifest {
        id: "fixture-en".into(),
        source_url: format!("{}{ARTIFACT_PATH}", server.uri()),
        source_revision: "0123456789abcdef0123456789abcdef01234567".into(),
        license: "MIT".into(),
        size_bytes: body.len() as u64,
        sha256: sha256_hex(body),
        runtime: "whisper-rs 0.13 (GGML)".into(),
        destination: "ggml-fixture.en.bin".into(),
    }
}

async fn serving(body: &'static [u8]) -> MockServer {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path(ARTIFACT_PATH))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(body))
        .mount(&server)
        .await;
    server
}

fn manager(dir: &std::path::Path) -> ModelManager {
    ModelManager::with_source(
        dir.join("models"),
        Arc::new(HttpModelSource::new().unwrap()),
    )
}

const fn nothing(_: DownloadProgress) {}

#[tokio::test]
async fn test_the_fixture_server_is_asked_for_nothing_until_the_user_confirms() {
    let server = serving(ARTIFACT).await;
    let dir = tempfile::tempdir().unwrap();
    let manifest = manifest_for(&server, ARTIFACT);
    let manager = manager(dir.path());

    // Reading the plan is what a confirmation prompt is built from.
    let plan = manager.plan_for(&manifest).unwrap();
    assert_eq!(
        server.received_requests().await.unwrap().len(),
        0,
        "building the confirmation prompt reached the artifact source"
    );

    // An unconfirmed install must reach it no further.
    let refused = manager
        .install_manifest(
            &manifest,
            &ModelConfirmation::new(&manifest.id, "a digest the user was never shown"),
            &CancellationToken::new(),
            &nothing,
        )
        .await
        .unwrap_err();
    assert_eq!(refused.code(), ErrorCode::ModelConfirmationRequired);
    assert_eq!(
        server.received_requests().await.unwrap().len(),
        0,
        "an unconfirmed install requested the artifact"
    );

    // Confirming is what turns the prompt into permission.
    let report = manager
        .install_manifest(
            &manifest,
            &ModelConfirmation::from_plan(&plan),
            &CancellationToken::new(),
            &nothing,
        )
        .await
        .unwrap();

    assert_eq!(report.state, ModelState::Ready);
    let requests = server.received_requests().await.unwrap();
    assert_eq!(
        requests.len(),
        1,
        "the confirmed install made {} requests",
        requests.len()
    );
    assert_eq!(requests[0].url.path(), ARTIFACT_PATH);
}

#[tokio::test]
async fn test_a_server_serving_other_bytes_promotes_nothing() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path(ARTIFACT_PATH))
        .respond_with(
            ResponseTemplate::new(200).set_body_bytes(b"not the artifact at all".to_vec()),
        )
        .mount(&server)
        .await;
    let dir = tempfile::tempdir().unwrap();
    let manifest = manifest_for(&server, ARTIFACT);
    let manager = manager(dir.path());

    let report = manager
        .install_manifest(
            &manifest,
            &ModelConfirmation::new(&manifest.id, &manifest.sha256),
            &CancellationToken::new(),
            &nothing,
        )
        .await
        .unwrap();

    assert!(
        matches!(
            report.state,
            ModelState::Failed {
                reason: ModelFailure::SizeMismatch { .. } | ModelFailure::DigestMismatch { .. }
            }
        ),
        "{:?}",
        report.state
    );
    assert!(!dir.path().join("models/ggml-fixture.en.bin").exists());
}

#[tokio::test]
async fn test_an_installed_artifact_is_ready_without_reaching_the_server() {
    let server = serving(ARTIFACT).await;
    let dir = tempfile::tempdir().unwrap();
    let manifest = manifest_for(&server, ARTIFACT);
    std::fs::create_dir_all(dir.path().join("models")).unwrap();
    std::fs::write(dir.path().join("models/ggml-fixture.en.bin"), ARTIFACT).unwrap();

    let report = manager(dir.path())
        .install_manifest(
            &manifest,
            &ModelConfirmation::new(&manifest.id, &manifest.sha256),
            &CancellationToken::new(),
            &nothing,
        )
        .await
        .unwrap();

    assert_eq!(report.state, ModelState::Ready);
    assert!(!report.promoted);
    assert_eq!(
        server.received_requests().await.unwrap().len(),
        0,
        "an already-installed artifact was fetched again"
    );
}

#[tokio::test]
async fn test_a_refusing_server_promotes_nothing() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path(ARTIFACT_PATH))
        .respond_with(ResponseTemplate::new(404))
        .mount(&server)
        .await;
    let dir = tempfile::tempdir().unwrap();
    let manifest = manifest_for(&server, ARTIFACT);

    let report = manager(dir.path())
        .install_manifest(
            &manifest,
            &ModelConfirmation::new(&manifest.id, &manifest.sha256),
            &CancellationToken::new(),
            &nothing,
        )
        .await
        .unwrap();

    assert!(
        matches!(
            report.state,
            ModelState::Failed {
                reason: ModelFailure::Transport { .. }
            }
        ),
        "{:?}",
        report.state
    );
    assert!(!dir.path().join("models/ggml-fixture.en.bin").exists());
}
