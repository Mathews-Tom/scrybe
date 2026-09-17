// Copyright 2026 Mathews Tom
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//     https://www.apache.org/licenses/LICENSE-2.0

//! The `scrybe-audio://` scheme, and the one artifact it serves.
//!
//! An `<audio>` element needs a URL, and there is no URL a `WebView` can
//! be given that names a file under the storage root without also
//! naming a way to reach files that are not under it. Tauri's own asset
//! protocol is exactly that shape — a path the frontend composes — so
//! it is not used here. This scheme carries an opaque session identity
//! and a fixed artifact name instead:
//!
//! ```text
//! scrybe-audio://localhost/<session-id>/playback
//! ```
//!
//! Three properties are structural rather than conventional:
//!
//! - **The identity is parsed, not validated.** The first path segment
//!   becomes a [`SessionRef`] or the request is refused. Separators,
//!   traversal, drive-relative and home-relative forms are rejected at
//!   construction, so no `Path` is ever built from a string this module
//!   inspected by hand. That is what keeps the two platforms honest:
//!   Unix resolves `..` through the filesystem and Windows normalizes
//!   it lexically, and a check written against one is not a check on
//!   the other.
//! - **The artifact is named here, not by the caller.** The only path
//!   this scheme answers to ends in `playback`, and the only file it
//!   ever opens is `playback.opus`. A request for `transcript.md` is
//!   not a permission failure, it is a path this scheme has no meaning
//!   for.
//! - **The session is resolved before the file is.** The repository
//!   answers whether the session exists and whether its playback
//!   artifact does, so a URL is refused for the same reasons the detail
//!   view withholds the player.
//!
//! The identity is carried in the path rather than in the URL's host
//! because a host is not case-preserving in every URL parser and a
//! session folder name carries an upper-case ULID. A host that arrived
//! lower-cased would name no session at all.

use std::io::{Read as _, Seek as _, SeekFrom};
use std::path::Path;

use scrybe_application::sessions::SessionDetail;
use scrybe_application::{ApplicationError, ErrorCode, SessionRef};
use tauri::http::{Request, Response, StatusCode};
use tauri::{Manager as _, UriSchemeContext};

use crate::contract::CommandFailure;
use crate::state::Desktop;

/// The scheme this module registers.
pub const SCHEME: &str = "scrybe-audio";

/// The only path suffix the scheme answers to.
const ARTIFACT: &str = "playback";

/// The file that suffix names, and the only file ever opened here.
const ARTIFACT_FILE: &str = "playback.opus";

/// Opus in an Ogg container, which is what `pipeline::merge` writes.
const CONTENT_TYPE: &str = "audio/ogg";

/// Serves one request on the scheme.
///
/// Registered as the protocol handler itself rather than wrapped, so a
/// test that drives this function drives what the application
/// registered. The two arguments arrive by value because that is the
/// shape Tauri's handler signature has, consumed or not.
#[allow(clippy::needless_pass_by_value)]
pub fn serve(
    context: UriSchemeContext<'_, tauri::Wry>,
    request: Request<Vec<u8>>,
) -> Response<Vec<u8>> {
    let app = context.app_handle();
    let desktop = app.state::<Desktop>();
    let response = respond(&desktop, &request);
    crate::note!(
        app,
        "playback-served",
        &format!(
            "{} {} {}",
            request.uri().path(),
            response.status().as_u16(),
            response.body().len()
        )
    );
    response
}

/// Everything [`serve`] does except record that it did it.
///
/// Separated only so a test can drive it: a `UriSchemeContext` is
/// constructed by the runtime and cannot be built outside it, and a
/// test that stopped at [`identity`] would prove the path helper right
/// without proving the handler consults it.
pub fn respond(desktop: &Desktop, request: &Request<Vec<u8>>) -> Response<Vec<u8>> {
    let id = match identity(request.uri().path()) {
        Ok(id) => id,
        Err(failure) => return refuse(StatusCode::NOT_FOUND, &failure),
    };
    let detail = match desktop.application().sessions().get_session(&id) {
        Ok(detail) => detail,
        Err(error) => return refuse(StatusCode::NOT_FOUND, &CommandFailure::from(error)),
    };
    if let Err((status, failure)) = playable(&detail) {
        return refuse(status, &failure);
    }
    let path = desktop
        .application()
        .sessions()
        .root()
        .resolve(&detail.id)
        .join(ARTIFACT_FILE);
    read(&path, request)
}

/// The session the path names, if the path is one this scheme answers
/// to at all.
///
/// Exactly two segments, the second exactly [`ARTIFACT`]. Anything
/// else — a third segment, a different artifact name, a traversal
/// component, an absolute form — has no meaning on this scheme and is
/// refused before a session is looked up, let alone a file opened.
fn identity(path: &str) -> Result<SessionRef, CommandFailure> {
    // One leading separator is stripped, not every leading separator.
    // `//etc/playback` is three segments with an empty first one, and
    // collapsing it to two would have this scheme answer to a path the
    // caller did not write.
    let Some(rest) = path.strip_prefix('/') else {
        return Err(unknown(path));
    };
    let mut segments = rest.split('/');
    let (Some(first), Some(second), None) = (segments.next(), segments.next(), segments.next())
    else {
        return Err(unknown(path));
    };
    if second != ARTIFACT {
        return Err(unknown(path));
    }
    SessionRef::parse(first).map_err(Into::into)
}

fn unknown(path: &str) -> CommandFailure {
    ApplicationError::new(
        ErrorCode::SessionNotFound,
        format!("{path} is not a path this scheme serves"),
    )
    .into()
}

/// Whether this session's playback artifact may be served.
///
/// The same answer the detail view renders its player from, read off
/// the same field, so a URL cannot be playable while the affordance
/// that produced it says otherwise.
fn playable(detail: &SessionDetail) -> Result<(), (StatusCode, CommandFailure)> {
    if detail.artifacts.playback {
        return Ok(());
    }
    let (status, code, message) = if detail.state.is_complete() {
        (
            StatusCode::NOT_FOUND,
            ErrorCode::NotApplicable,
            format!("session {} has no playback audio", detail.id),
        )
    } else {
        (
            StatusCode::CONFLICT,
            ErrorCode::NotApplicable,
            format!("session {} has not finished recording", detail.id),
        )
    };
    Err((status, ApplicationError::new(code, message).into()))
}

/// A refusal carrying the same payload a failed command carries, so the
/// two surfaces describe a failure the same way.
fn refuse(status: StatusCode, failure: &CommandFailure) -> Response<Vec<u8>> {
    let body = serde_json::to_vec(failure).unwrap_or_default();
    build(status)
        .header("Content-Type", "application/json")
        .body(body)
        .unwrap_or_else(|_| empty(StatusCode::INTERNAL_SERVER_ERROR))
}

fn build(status: StatusCode) -> tauri::http::response::Builder {
    Response::builder()
        .status(status)
        // The scheme is reachable from the application's own origin and
        // from nowhere else, so no other origin is named.
        .header("Access-Control-Allow-Origin", "null")
        .header("Cache-Control", "no-store")
}

fn empty(status: StatusCode) -> Response<Vec<u8>> {
    let mut response = Response::new(Vec::new());
    *response.status_mut() = status;
    response
}

fn read(path: &Path, request: &Request<Vec<u8>>) -> Response<Vec<u8>> {
    let mut file = match std::fs::File::open(path) {
        Ok(file) => file,
        Err(source) => {
            return refuse(
                StatusCode::NOT_FOUND,
                &CommandFailure::from(
                    ApplicationError::new(
                        ErrorCode::StorageUnavailable,
                        "the playback artifact could not be opened".to_string(),
                    )
                    .with_source(source),
                ),
            )
        }
    };
    let Ok(total) = file.metadata().map(|meta| meta.len()) else {
        return empty(StatusCode::INTERNAL_SERVER_ERROR);
    };
    let header = request
        .headers()
        .get("Range")
        .and_then(|value| value.to_str().ok());

    match range(header, total) {
        Wanted::Whole => whole(&mut file, total),
        Wanted::Slice { start, end } => slice(&mut file, start, end, total),
        Wanted::Unsatisfiable => build(StatusCode::RANGE_NOT_SATISFIABLE)
            .header("Content-Range", format!("bytes */{total}"))
            .body(Vec::new())
            .unwrap_or_else(|_| empty(StatusCode::RANGE_NOT_SATISFIABLE)),
    }
}

fn whole(file: &mut std::fs::File, total: u64) -> Response<Vec<u8>> {
    let mut body = Vec::new();
    if file.read_to_end(&mut body).is_err() {
        return empty(StatusCode::INTERNAL_SERVER_ERROR);
    }
    build(StatusCode::OK)
        .header("Content-Type", CONTENT_TYPE)
        .header("Accept-Ranges", "bytes")
        .header("Content-Length", total.to_string())
        .body(body)
        .unwrap_or_else(|_| empty(StatusCode::INTERNAL_SERVER_ERROR))
}

/// `start` and `end` are inclusive, and both are already known to be
/// inside the file.
fn slice(file: &mut std::fs::File, start: u64, end: u64, total: u64) -> Response<Vec<u8>> {
    let length = end - start + 1;
    let Ok(length) = usize::try_from(length) else {
        return empty(StatusCode::INTERNAL_SERVER_ERROR);
    };
    let mut body = vec![0_u8; length];
    if file.seek(SeekFrom::Start(start)).is_err() || file.read_exact(&mut body).is_err() {
        return empty(StatusCode::INTERNAL_SERVER_ERROR);
    }
    build(StatusCode::PARTIAL_CONTENT)
        .header("Content-Type", CONTENT_TYPE)
        .header("Accept-Ranges", "bytes")
        .header("Content-Length", length.to_string())
        .header("Content-Range", format!("bytes {start}-{end}/{total}"))
        .body(body)
        .unwrap_or_else(|_| empty(StatusCode::INTERNAL_SERVER_ERROR))
}

/// What a `Range` header asks for.
#[derive(Debug, Eq, PartialEq)]
enum Wanted {
    /// The whole representation: either no range was asked for, or one
    /// was asked for in a form this server does not honour, which RFC
    /// 9110 says to ignore rather than refuse.
    Whole,
    /// One inclusive byte range, already clamped inside the file.
    Slice { start: u64, end: u64 },
    /// A range that is syntactically fine and cannot be satisfied.
    Unsatisfiable,
}

/// Parses a `Range` header against a representation of `total` bytes.
///
/// Written out rather than taken from a helper because Tauri supplies
/// none, and the shape of the answer is the whole point: an invalid
/// header is ignored, an unsatisfiable one is refused, and the two are
/// not the same thing. RFC 9110 §14.2 says a recipient that cannot
/// understand a `Range` header must ignore it — a player sending
/// something this does not parse gets the whole file and works, rather
/// than a 416 and silence. §14.1.1 makes a `last-pos` below `first-pos`
/// invalid, and §14.1.2 makes a suffix length of zero unsatisfiable.
///
/// Multiple ranges are ignored rather than refused: `multipart/
/// byteranges` is not implemented here, and §14.2 permits a server that
/// does not support it to answer with the whole representation.
fn range(header: Option<&str>, total: u64) -> Wanted {
    let Some(header) = header else {
        return Wanted::Whole;
    };
    let Some(spec) = header.trim().strip_prefix("bytes=") else {
        return Wanted::Whole;
    };
    let spec = spec.trim();
    if spec.contains(',') {
        return Wanted::Whole;
    }
    let Some((first, last)) = spec.split_once('-') else {
        return Wanted::Whole;
    };
    let (first, last) = (first.trim(), last.trim());

    if first.is_empty() {
        // A suffix range: the last `last` bytes.
        let Ok(length) = last.parse::<u64>() else {
            return Wanted::Whole;
        };
        if length == 0 || total == 0 {
            return Wanted::Unsatisfiable;
        }
        let start = total.saturating_sub(length);
        return Wanted::Slice {
            start,
            end: total - 1,
        };
    }

    let Ok(start) = first.parse::<u64>() else {
        return Wanted::Whole;
    };
    if start >= total {
        return Wanted::Unsatisfiable;
    }
    if last.is_empty() {
        return Wanted::Slice {
            start,
            end: total - 1,
        };
    }
    let Ok(end) = last.parse::<u64>() else {
        return Wanted::Whole;
    };
    if end < start {
        return Wanted::Unsatisfiable;
    }
    Wanted::Slice {
        start,
        end: end.min(total - 1),
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    const TOTAL: u64 = 100;

    #[test]
    fn test_no_range_asks_for_the_whole_representation() {
        assert_eq!(range(None, TOTAL), Wanted::Whole);
    }

    #[test]
    fn test_a_closed_range_is_the_bytes_it_names() {
        assert_eq!(
            range(Some("bytes=10-19"), TOTAL),
            Wanted::Slice { start: 10, end: 19 }
        );
    }

    #[test]
    fn test_an_open_ended_range_runs_to_the_last_byte() {
        assert_eq!(
            range(Some("bytes=90-"), TOTAL),
            Wanted::Slice { start: 90, end: 99 }
        );
    }

    #[test]
    fn test_a_suffix_range_is_the_last_bytes_of_the_representation() {
        assert_eq!(
            range(Some("bytes=-10"), TOTAL),
            Wanted::Slice { start: 90, end: 99 }
        );
    }

    /// A suffix longer than the representation is the representation,
    /// not a refusal: RFC 9110 §14.1.2 says so explicitly.
    #[test]
    fn test_a_suffix_longer_than_the_file_is_the_whole_file() {
        assert_eq!(
            range(Some("bytes=-500"), TOTAL),
            Wanted::Slice { start: 0, end: 99 }
        );
    }

    #[test]
    fn test_a_zero_length_suffix_cannot_be_satisfied() {
        assert_eq!(range(Some("bytes=-0"), TOTAL), Wanted::Unsatisfiable);
    }

    #[test]
    fn test_a_range_that_ends_before_it_starts_cannot_be_satisfied() {
        assert_eq!(range(Some("bytes=50-40"), TOTAL), Wanted::Unsatisfiable);
    }

    #[test]
    fn test_a_range_starting_past_the_end_cannot_be_satisfied() {
        assert_eq!(range(Some("bytes=100-"), TOTAL), Wanted::Unsatisfiable);
        assert_eq!(range(Some("bytes=250-300"), TOTAL), Wanted::Unsatisfiable);
    }

    #[test]
    fn test_a_range_that_ends_past_the_end_stops_at_the_last_byte() {
        assert_eq!(
            range(Some("bytes=95-500"), TOTAL),
            Wanted::Slice { start: 95, end: 99 }
        );
    }

    /// Ignored rather than refused. A player that sends a header this
    /// does not parse gets the whole file and works; a 416 would leave
    /// it silent for a header the specification says to disregard.
    #[test]
    fn test_a_header_this_server_cannot_parse_is_ignored() {
        for header in [
            "chunks=0-10",
            "bytes=",
            "bytes=abc-def",
            "bytes=10-xyz",
            "bytes 0-10",
            "0-10",
        ] {
            assert_eq!(range(Some(header), TOTAL), Wanted::Whole, "{header}");
        }
    }

    /// `multipart/byteranges` is not implemented, and §14.2 permits
    /// answering with the whole representation rather than refusing.
    #[test]
    fn test_a_request_for_several_ranges_is_answered_with_the_whole_file() {
        assert_eq!(range(Some("bytes=0-9,20-29"), TOTAL), Wanted::Whole);
    }

    #[test]
    fn test_an_empty_representation_satisfies_no_range() {
        assert_eq!(range(Some("bytes=0-"), 0), Wanted::Unsatisfiable);
        assert_eq!(range(Some("bytes=-1"), 0), Wanted::Unsatisfiable);
    }

    #[test]
    fn test_the_only_path_this_scheme_answers_to_names_a_session_and_playback() {
        assert_eq!(
            identity("/2026-04-29-1430-acme-01HXYZ/playback")
                .unwrap()
                .as_str(),
            "2026-04-29-1430-acme-01HXYZ"
        );
    }

    #[test]
    fn test_no_other_artifact_is_reachable_on_this_scheme() {
        for path in [
            "/2026-04-29-1430-acme-01HXYZ/transcript.md",
            "/2026-04-29-1430-acme-01HXYZ/notes.md",
            "/2026-04-29-1430-acme-01HXYZ/audio.opus",
            "/2026-04-29-1430-acme-01HXYZ/meta.toml",
            "/2026-04-29-1430-acme-01HXYZ/playback/extra",
            "/2026-04-29-1430-acme-01HXYZ",
            "/playback",
            "/",
        ] {
            assert!(identity(path).is_err(), "{path}");
        }
    }

    #[test]
    fn test_a_traversal_never_becomes_an_identity() {
        for path in [
            "/../playback",
            "/../../etc/playback",
            "/2026-04-29-1430-acme-01HXYZ/../other/playback",
            "//etc/playback",
            "/~/playback",
            "/C:/playback",
        ] {
            assert!(identity(path).is_err(), "{path}");
        }
    }
}
