// Copyright 2026 Mathews Tom
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//     https://www.apache.org/licenses/LICENSE-2.0

//! Recorded stdio transcript against `scrybe mcp` (M9 verification,
//! `.docs/DEVELOPMENT_PLAN.md` §6 M9, `.docs/EXECUTION_PROMPTS.md` M9).
//!
//! Spawns the real `scrybe` binary with `mcp --root <fixture>`,
//! writes a fixed sequence of newline-delimited JSON-RPC requests
//! covering `initialize`, `tools/list`, `ping`, and a `tools/call` for
//! all five required tools, closes stdin, and asserts on the recorded
//! response transcript. Compiled only under `agent-access` — the
//! subcommand does not exist in a default-feature build.

#![cfg(feature = "agent-access")]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::io::Write;
use std::process::{Command, Stdio};
use std::time::Duration;

use serde_json::Value;

const fn scrybe_bin() -> &'static str {
    env!("CARGO_BIN_EXE_scrybe")
}

/// Writes a fixture storage root with one finished and one unfinished
/// session, matching `scrybe-core::agent_access::reader`'s invariant
/// directly (hand-written TOML rather than going through the
/// production writer, since this integration test crate has no access
/// to `scrybe-core`'s private test helpers).
fn write_fixture_root(root: &std::path::Path) {
    let finished = root.join("2026-01-01-0900-standup-01STANDUPFIXTURE");
    std::fs::create_dir_all(&finished).unwrap();
    std::fs::write(
        finished.join("meta.toml"),
        "session_id = \"01STANDUPFIXTURE\"\n\
         title = \"Standup Fixture\"\n\
         started_at = \"2026-01-01T09:00:00Z\"\n\
         ended_at = \"2026-01-01T09:10:00Z\"\n\
         duration_secs = 600\n",
    )
    .unwrap();
    std::fs::write(
        finished.join("notes.md"),
        "# Notes\n\nShipped the fixture.\n",
    )
    .unwrap();
    std::fs::write(
        finished.join("transcript.md"),
        "**Me** [00:00:01]: good morning\n",
    )
    .unwrap();

    let unfinished_journal = root
        .join("2026-01-02-0900-crashed-01CRASHEDFIXTURE")
        .join("journal");
    std::fs::create_dir_all(&unfinished_journal).unwrap();
    std::fs::write(unfinished_journal.join("mic.f32"), [0_u8; 8]).unwrap();
}

fn write_enabled_config(cfg_dir: &std::path::Path) -> std::path::PathBuf {
    let path = cfg_dir.join("config.toml");
    std::fs::write(&path, "[agent_access]\nenabled = true\n").unwrap();
    path
}

#[test]
fn test_mcp_stdio_transcript_covers_all_five_tools_and_core_methods() {
    let root_dir = tempfile::tempdir().unwrap();
    write_fixture_root(root_dir.path());
    let cfg_dir = tempfile::tempdir().unwrap();
    let config_path = write_enabled_config(cfg_dir.path());

    let mut child = Command::new(scrybe_bin())
        .args(["mcp", "--root", root_dir.path().to_str().unwrap()])
        .env("SCRYBE_CONFIG", &config_path)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("failed to spawn scrybe mcp subprocess");

    let requests = [
        r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}"#,
        r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#,
        r#"{"jsonrpc":"2.0","id":2,"method":"ping"}"#,
        r#"{"jsonrpc":"2.0","id":3,"method":"tools/list"}"#,
        r#"{"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"list_recent_meetings","arguments":{}}}"#,
        r#"{"jsonrpc":"2.0","id":5,"method":"tools/call","params":{"name":"search_meetings","arguments":{"query":"fixture"}}}"#,
        r#"{"jsonrpc":"2.0","id":6,"method":"tools/call","params":{"name":"get_meeting","arguments":{"id":"2026-01-01-0900-standup-01STANDUPFIXTURE"}}}"#,
        r#"{"jsonrpc":"2.0","id":7,"method":"tools/call","params":{"name":"get_meeting","arguments":{"id":"2026-01-02-0900-crashed-01CRASHEDFIXTURE"}}}"#,
        r#"{"jsonrpc":"2.0","id":8,"method":"tools/call","params":{"name":"get_meeting_notes","arguments":{"id":"2026-01-01-0900-standup-01STANDUPFIXTURE"}}}"#,
        r#"{"jsonrpc":"2.0","id":9,"method":"tools/call","params":{"name":"get_meeting_transcript","arguments":{"id":"2026-01-01-0900-standup-01STANDUPFIXTURE"}}}"#,
    ];

    {
        let stdin = child.stdin.as_mut().expect("child stdin");
        for request in requests {
            writeln!(stdin, "{request}").expect("write request line");
        }
    }
    // Dropping the stdin handle closes the pipe, so the child's
    // `next_line()` loop sees EOF and the process exits on its own —
    // no signal, no timeout race.
    child.stdin.take();

    let output = child
        .wait_with_output()
        .expect("failed to wait for scrybe mcp subprocess");
    assert!(
        output.status.success(),
        "scrybe mcp must exit 0 on stdin EOF; stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8(output.stdout).expect("stdout must be UTF-8");
    let responses: Vec<Value> = stdout
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| serde_json::from_str(l).unwrap_or_else(|e| panic!("invalid JSON line {l:?}: {e}")))
        .collect();

    // 9 requests carry an id (the notification does not) => 9 response lines.
    assert_eq!(
        responses.len(),
        9,
        "unexpected response count: {responses:#?}"
    );
    for response in &responses {
        assert_eq!(response["jsonrpc"], "2.0");
    }

    let initialize = &responses[0];
    assert_eq!(initialize["id"], 1);
    assert_eq!(initialize["result"]["schema_version"], 1);
    assert!(initialize["result"]["protocolVersion"].is_string());

    let ping = &responses[1];
    assert_eq!(ping["result"]["schema_version"], 1);

    let tools_list = &responses[2];
    let tool_names: Vec<&str> = tools_list["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap())
        .collect();
    assert_eq!(
        tool_names,
        vec![
            "list_recent_meetings",
            "search_meetings",
            "get_meeting",
            "get_meeting_notes",
            "get_meeting_transcript",
        ]
    );

    let list_recent: Value = serde_json::from_str(
        responses[3]["result"]["content"][0]["text"]
            .as_str()
            .unwrap(),
    )
    .unwrap();
    assert!(list_recent
        .as_array()
        .unwrap()
        .iter()
        .any(|meeting| meeting["title"] == "Standup Fixture"));

    let search: Value = serde_json::from_str(
        responses[4]["result"]["content"][0]["text"]
            .as_str()
            .unwrap(),
    )
    .unwrap();
    assert!(!search.as_array().unwrap().is_empty());

    let finished_meeting: Value = serde_json::from_str(
        responses[5]["result"]["content"][0]["text"]
            .as_str()
            .unwrap(),
    )
    .unwrap();
    assert_eq!(finished_meeting["status"], "finished");
    assert_eq!(finished_meeting["title"], "Standup Fixture");
    assert_eq!(finished_meeting["duration_secs"], 600);

    let unfinished_meeting: Value = serde_json::from_str(
        responses[6]["result"]["content"][0]["text"]
            .as_str()
            .unwrap(),
    )
    .unwrap();
    assert_eq!(unfinished_meeting["status"], "unfinished");
    assert!(
        unfinished_meeting.get("title").is_none(),
        "an unfinished meeting must never be served as though it were complete"
    );

    let notes: Value = serde_json::from_str(
        responses[7]["result"]["content"][0]["text"]
            .as_str()
            .unwrap(),
    )
    .unwrap();
    assert_eq!(notes["content"], "# Notes\n\nShipped the fixture.\n");

    let transcript: Value = serde_json::from_str(
        responses[8]["result"]["content"][0]["text"]
            .as_str()
            .unwrap(),
    )
    .unwrap();
    assert_eq!(transcript["content"], "**Me** [00:00:01]: good morning\n");

    // Nothing under the fixture root was ever touched by the server.
    let notes_still_there = std::fs::read_to_string(
        root_dir
            .path()
            .join("2026-01-01-0900-standup-01STANDUPFIXTURE")
            .join("notes.md"),
    )
    .unwrap();
    assert_eq!(notes_still_there, "# Notes\n\nShipped the fixture.\n");
}

#[test]
fn test_mcp_refuses_to_start_when_agent_access_disabled() {
    let root_dir = tempfile::tempdir().unwrap();
    let cfg_dir = tempfile::tempdir().unwrap();
    // No config file at all => `Config::default()` => disabled.
    let config_path = cfg_dir.path().join("nonexistent-config.toml");

    let mut child = Command::new(scrybe_bin())
        .args(["mcp", "--root", root_dir.path().to_str().unwrap()])
        .env("SCRYBE_CONFIG", &config_path)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("failed to spawn scrybe mcp subprocess");
    child.stdin.take();

    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    let status = loop {
        if let Some(status) = child.try_wait().expect("polling child status") {
            break status;
        }
        assert!(
            std::time::Instant::now() <= deadline,
            "scrybe mcp did not exit within 10s while agent_access is disabled"
        );
        std::thread::sleep(Duration::from_millis(10));
    };

    assert!(!status.success(), "must refuse to start, not exit 0");
}
