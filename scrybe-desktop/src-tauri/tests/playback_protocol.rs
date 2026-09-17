// Copyright 2026 Mathews Tom
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//     https://www.apache.org/licenses/LICENSE-2.0

//! The `scrybe-audio://` scheme, driven the way the runtime drives it.
//!
//! Every case below builds a real storage root, a real `Desktop` over
//! it, and a real request, and asks the handler the application
//! registered. Nothing here calls the path helper: a helper proved
//! right in isolation says nothing about whether the handler consults
//! it on every path, and the traversal cases exist precisely because
//! that is the gap a previous increment fell into.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::path::Path;

use scrybe_application::StorageRoot;
use scrybe_desktop::playback;
use scrybe_desktop::state::Desktop;
use tauri::http::{Request, Response, StatusCode};

const COMPLETE: &str = "2026-04-29-1430-acme-01HXYZ";
const OTHER: &str = "2026-04-28-0900-standup-01AAAA";
const UNFINISHED: &str = "2026-04-27-1100-abandoned-01BBBB";
const MONO: &str = "2026-04-26-1500-onevoice-01CCCC";

/// Deliberately not a valid Opus stream. Nothing in this path decodes
/// audio; what is asserted is which bytes are served, and a known byte
/// pattern makes an off-by-one in the range arithmetic visible.
fn audio_bytes() -> Vec<u8> {
    (0..=255_u8).cycle().take(1024).collect()
}

fn meta(title: &str) -> String {
    format!(
        "session_id = \"01HXYZ\"\n\
         title = \"{title}\"\n\
         started_at = \"2026-04-29T14:30:00Z\"\n\
         ended_at = \"2026-04-29T15:00:00Z\"\n\
         duration_secs = 1800\n"
    )
}

/// A storage root holding one complete stereo session with playback
/// audio, one complete mono session without it, one unfinished session,
/// and a second complete session to aim a traversal at.
fn root() -> (tempfile::TempDir, Desktop) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path();

    write(&path.join(COMPLETE), "meta.toml", meta("Acme").as_bytes());
    write(&path.join(COMPLETE), "audio.opus", b"");
    write(&path.join(COMPLETE), "playback.opus", &audio_bytes());
    write(&path.join(COMPLETE), "transcript.md", b"# t\nhello\n");

    write(&path.join(OTHER), "meta.toml", meta("Standup").as_bytes());
    write(&path.join(OTHER), "audio.opus", b"");
    write(&path.join(OTHER), "playback.opus", b"other-session-audio");

    write(&path.join(MONO), "meta.toml", meta("One voice").as_bytes());
    write(&path.join(MONO), "audio.opus", b"");

    write(&path.join(UNFINISHED), "audio.opus", b"");
    write(&path.join(UNFINISHED), "playback.opus", &audio_bytes());

    let desktop = Desktop::new(StorageRoot::new(path), path.join("config.toml"));
    (dir, desktop)
}

fn write(folder: &Path, name: &str, body: &[u8]) {
    std::fs::create_dir_all(folder).unwrap();
    std::fs::write(folder.join(name), body).unwrap();
}

fn get(desktop: &Desktop, path: &str, range: Option<&str>) -> Response<Vec<u8>> {
    let mut builder = Request::builder().uri(format!("scrybe-audio://localhost{path}"));
    if let Some(range) = range {
        builder = builder.header("Range", range);
    }
    playback::respond(desktop, &builder.body(Vec::new()).unwrap())
}

fn header(response: &Response<Vec<u8>>, name: &str) -> String {
    response
        .headers()
        .get(name)
        .map(|value| value.to_str().unwrap_or_default().to_owned())
        .unwrap_or_default()
}

#[test]
fn test_a_playable_session_serves_its_whole_playback_artifact() {
    let (_dir, desktop) = root();

    let response = get(&desktop, &format!("/{COMPLETE}/playback"), None);

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(header(&response, "Content-Type"), "audio/ogg");
    assert_eq!(header(&response, "Accept-Ranges"), "bytes");
    assert_eq!(header(&response, "Content-Length"), "1024");
    assert_eq!(response.body(), &audio_bytes());
}

#[test]
fn test_a_byte_range_serves_exactly_the_bytes_it_names() {
    let (_dir, desktop) = root();

    let response = get(
        &desktop,
        &format!("/{COMPLETE}/playback"),
        Some("bytes=10-19"),
    );

    assert_eq!(response.status(), StatusCode::PARTIAL_CONTENT);
    assert_eq!(header(&response, "Content-Range"), "bytes 10-19/1024");
    assert_eq!(header(&response, "Content-Length"), "10");
    assert_eq!(response.body(), &audio_bytes()[10..=19]);
}

#[test]
fn test_a_suffix_range_serves_the_end_of_the_artifact() {
    let (_dir, desktop) = root();

    let response = get(&desktop, &format!("/{COMPLETE}/playback"), Some("bytes=-8"));

    assert_eq!(response.status(), StatusCode::PARTIAL_CONTENT);
    assert_eq!(header(&response, "Content-Range"), "bytes 1016-1023/1024");
    assert_eq!(response.body(), &audio_bytes()[1016..]);
}

#[test]
fn test_an_open_ended_range_serves_to_the_end_of_the_artifact() {
    let (_dir, desktop) = root();

    let response = get(
        &desktop,
        &format!("/{COMPLETE}/playback"),
        Some("bytes=1020-"),
    );

    assert_eq!(response.status(), StatusCode::PARTIAL_CONTENT);
    assert_eq!(header(&response, "Content-Range"), "bytes 1020-1023/1024");
    assert_eq!(response.body(), &audio_bytes()[1020..]);
}

/// A player that seeks past the end, or asks for nothing, gets the one
/// answer that tells it the representation's size — which is what lets
/// it recover rather than stall.
#[test]
fn test_a_range_that_cannot_be_satisfied_is_refused_with_the_artifact_size() {
    let (_dir, desktop) = root();

    for header_value in ["bytes=2000-", "bytes=50-40", "bytes=-0", "bytes=1024-2048"] {
        let response = get(
            &desktop,
            &format!("/{COMPLETE}/playback"),
            Some(header_value),
        );

        assert_eq!(
            response.status(),
            StatusCode::RANGE_NOT_SATISFIABLE,
            "{header_value}"
        );
        assert_eq!(header(&response, "Content-Range"), "bytes */1024");
        assert!(response.body().is_empty());
    }
}

/// Ignored, not refused: a player that sends a header this server
/// cannot parse gets the whole artifact and works.
#[test]
fn test_a_range_header_this_server_cannot_parse_serves_the_whole_artifact() {
    let (_dir, desktop) = root();

    for header_value in ["chunks=0-10", "bytes=abc", "bytes=0-9,20-29"] {
        let response = get(
            &desktop,
            &format!("/{COMPLETE}/playback"),
            Some(header_value),
        );

        assert_eq!(response.status(), StatusCode::OK, "{header_value}");
        assert_eq!(response.body().len(), 1024);
    }
}

#[test]
fn test_a_session_with_no_playback_artifact_is_refused_even_with_a_range() {
    let (_dir, desktop) = root();

    let response = get(&desktop, &format!("/{MONO}/playback"), Some("bytes=0-9"));

    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    assert!(header(&response, "Content-Range").is_empty());
    assert!(String::from_utf8_lossy(response.body()).contains("no playback audio"));
}

#[test]
fn test_an_unfinished_session_is_refused_even_though_the_file_is_there() {
    // The artifact exists on disk. What it does not have is a session
    // whose duration and channel attribution were ever established, so
    // what it holds is not known to be the whole recording.
    let (_dir, desktop) = root();

    let response = get(
        &desktop,
        &format!("/{UNFINISHED}/playback"),
        Some("bytes=0-9"),
    );

    assert_eq!(response.status(), StatusCode::CONFLICT);
    assert!(String::from_utf8_lossy(response.body()).contains("has not finished"));
}

#[test]
fn test_a_session_that_does_not_exist_is_refused() {
    let (_dir, desktop) = root();

    let response = get(&desktop, "/2099-01-01-0000-nothing-01ZZZZ/playback", None);

    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

/// The confinement assertion, against the handler rather than the path
/// helper underneath it. Every one of these names a file that exists in
/// the fixture tree, so a handler that resolved any of them would serve
/// real bytes rather than fail to find anything.
#[test]
fn test_no_path_on_this_scheme_reaches_anything_but_a_playback_artifact() {
    let (_dir, desktop) = root();
    let escapes = [
        format!("/{COMPLETE}/../{OTHER}/playback.opus"),
        format!("/{COMPLETE}/../{OTHER}/playback"),
        format!("/{COMPLETE}/transcript.md"),
        format!("/{COMPLETE}/audio.opus"),
        format!("/{COMPLETE}/meta.toml"),
        format!("/{COMPLETE}"),
        "/../../etc/passwd/playback".to_string(),
        "//etc/passwd/playback".to_string(),
        "/~/playback".to_string(),
        "/C:/Windows/playback".to_string(),
        "/playback".to_string(),
        "/".to_string(),
    ];

    for path in escapes {
        let response = get(&desktop, &path, None);

        assert_eq!(response.status(), StatusCode::NOT_FOUND, "{path}");
        assert!(response.body().len() < 256, "{path} served a document");
        assert_ne!(response.body(), &audio_bytes(), "{path} served the audio");
    }
}

/// The traversal above must not be refused merely because the file it
/// names is missing: the fixture puts a real, different artifact at the
/// far end of it, and this is the assertion that the refusal is about
/// the path rather than about the filesystem.
#[test]
fn test_the_session_a_traversal_aims_at_is_itself_servable_by_its_own_name() {
    let (_dir, desktop) = root();

    let response = get(&desktop, &format!("/{OTHER}/playback"), None);

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.body(), b"other-session-audio");
}
