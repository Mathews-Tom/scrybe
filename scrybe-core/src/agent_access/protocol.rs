// Copyright 2026 Mathews Tom
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//     https://www.apache.org/licenses/LICENSE-2.0

//! Hand-rolled JSON-RPC 2.0 transport and the minimal Model Context
//! Protocol (MCP) surface built on it: `initialize`, `tools/list`,
//! `tools/call`, and `ping`.
//!
//! No MCP SDK dependency — the wire format is
//! small enough to implement directly against `serde_json`, which
//! `scrybe-core` already depends on unconditionally.
//!
//! [`handle_message`] is the entire entry point: given one line of
//! newline-delimited JSON, it returns the line to write back (or
//! `None` for a notification, which JSON-RPC never answers). The
//! caller (`scrybe-cli`'s `mcp` command) owns the actual stdio loop;
//! this module never touches stdin/stdout itself, which keeps it
//! synchronous and trivially testable.
//!
//! Every result this module produces — `initialize`, `tools/list`,
//! `ping`, and each `tools/call` payload — carries a top-level
//! `schema_version` field, per the M9 contract that every response
//! from this surface is versioned.

use std::path::Path;

use serde::Serialize;
use serde_json::{json, Value};

use super::fs::ReadOnlyFs;
use super::reader::{self, AgentAccessError, DEFAULT_LIST_LIMIT, MAX_LIST_LIMIT};

/// Schema version stamped onto every response this module produces.
///
/// Bump when a response shape changes in a way a client could not
/// tolerate via ordinary JSON forward-compatibility (i.e. a field is
/// removed or its meaning changes, not merely added).
pub const SCHEMA_VERSION: u32 = 1;
const PROTOCOL_VERSION: &str = "2024-11-05";
const JSONRPC_VERSION: &str = "2.0";

const PARSE_ERROR: i64 = -32_700;
const INVALID_REQUEST: i64 = -32_600;
const METHOD_NOT_FOUND: i64 = -32_601;
const INVALID_PARAMS: i64 = -32_602;

#[derive(Debug, Clone, Serialize)]
struct RpcResponse {
    jsonrpc: &'static str,
    id: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<RpcErrorBody>,
}

#[derive(Debug, Clone, Serialize)]
struct RpcErrorBody {
    code: i64,
    message: String,
}

const fn ok_response(id: Value, result: Value) -> RpcResponse {
    RpcResponse {
        jsonrpc: JSONRPC_VERSION,
        id,
        result: Some(result),
        error: None,
    }
}

fn err_response(id: Value, code: i64, message: impl Into<String>) -> RpcResponse {
    RpcResponse {
        jsonrpc: JSONRPC_VERSION,
        id,
        result: None,
        error: Some(RpcErrorBody {
            code,
            message: message.into(),
        }),
    }
}

fn encode(response: &RpcResponse) -> String {
    serde_json::to_string(response).unwrap_or_else(|e| {
        format!(
            r#"{{"jsonrpc":"2.0","id":null,"error":{{"code":-32603,"message":"failed to encode response: {e}"}}}}"#
        )
    })
}

/// Handles one newline-delimited JSON-RPC 2.0 request line.
///
/// Returns a response line, or `None` for a notification. JSON-RPC never
/// answers requests with no `id`, an `id: null`, or a `"notifications/"`
/// method. A malformed line still receives the appropriate `-32700` or
/// `-32600` response with `id: null`.
#[must_use]
pub fn handle_message(fs: &dyn ReadOnlyFs, root: &Path, line: &str) -> Option<String> {
    let value: Value = match serde_json::from_str(line) {
        Ok(v) => v,
        Err(e) => {
            return Some(encode(&err_response(
                Value::Null,
                PARSE_ERROR,
                format!("parse error: {e}"),
            )));
        }
    };
    dispatch(fs, root, &value).map(|response| encode(&response))
}

fn dispatch(fs: &dyn ReadOnlyFs, root: &Path, value: &Value) -> Option<RpcResponse> {
    let Some(method) = value.get("method").and_then(Value::as_str) else {
        return Some(err_response(
            Value::Null,
            INVALID_REQUEST,
            "invalid request: missing or non-string \"method\"",
        ));
    };
    // JSON-RPC notifications carry no "id" and receive no response.
    // We treat an explicit `"id": null` the same way: no well-behaved
    // MCP client sends one, and folding the two cases keeps the
    // "no id to answer" rule exception-free.
    let id = value.get("id").cloned().filter(|v| !v.is_null())?;
    if method.starts_with("notifications/") {
        return None;
    }

    let params = value.get("params").cloned().unwrap_or_else(|| json!({}));
    let outcome = match method {
        "initialize" => Ok(initialize_result()),
        "ping" => Ok(ping_result()),
        "tools/list" => Ok(tools_list_result()),
        "tools/call" => tools_call_result(fs, root, &params),
        other => Err((METHOD_NOT_FOUND, format!("method not found: {other}"))),
    };

    Some(match outcome {
        Ok(result) => ok_response(id, result),
        Err((code, message)) => err_response(id, code, message),
    })
}

fn initialize_result() -> Value {
    json!({
        "schema_version": SCHEMA_VERSION,
        "protocolVersion": PROTOCOL_VERSION,
        "serverInfo": { "name": "scrybe", "version": env!("CARGO_PKG_VERSION") },
        "capabilities": { "tools": {} },
    })
}

fn ping_result() -> Value {
    json!({ "schema_version": SCHEMA_VERSION })
}

fn tools_list_result() -> Value {
    json!({ "schema_version": SCHEMA_VERSION, "tools": tool_definitions() })
}

fn id_schema() -> Value {
    json!({
        "type": "object",
        "properties": { "id": { "type": "string", "description": "Session folder name, or an unambiguous ULID/prefix." } },
        "required": ["id"],
        "additionalProperties": false,
    })
}

fn limit_property() -> Value {
    json!({ "type": "integer", "minimum": 1, "maximum": MAX_LIST_LIMIT, "default": DEFAULT_LIST_LIMIT })
}

fn tool_definitions() -> Value {
    json!([
        {
            "name": "list_recent_meetings",
            "description": "List the most recently started local meetings under the configured storage root, most recent first. A meeting whose recording never finished is reported with status \"unfinished\" rather than served as complete.",
            "inputSchema": {
                "type": "object",
                "properties": { "limit": limit_property() },
                "additionalProperties": false,
            },
        },
        {
            "name": "search_meetings",
            "description": "Search local meetings by title, folder name, notes, and transcript text (case-insensitive substring match). An unfinished meeting can match by folder name only; its content is never searched or served.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "query": { "type": "string" },
                    "limit": limit_property(),
                },
                "required": ["query"],
                "additionalProperties": false,
            },
        },
        {
            "name": "get_meeting",
            "description": "Fetch metadata for one local meeting by folder name or an unambiguous ULID/prefix.",
            "inputSchema": id_schema(),
        },
        {
            "name": "get_meeting_notes",
            "description": "Fetch the generated notes.md content for one local meeting. `content` is null when the meeting is unfinished.",
            "inputSchema": id_schema(),
        },
        {
            "name": "get_meeting_transcript",
            "description": "Fetch the transcript.md content for one local meeting. `content` is null when the meeting is unfinished.",
            "inputSchema": id_schema(),
        },
    ])
}

fn tools_call_result(
    fs: &dyn ReadOnlyFs,
    root: &Path,
    params: &Value,
) -> Result<Value, (i64, String)> {
    let Some(name) = params.get("name").and_then(Value::as_str) else {
        return Err((INVALID_PARAMS, "missing required params.name".to_string()));
    };
    let empty = json!({});
    let arguments = params.get("arguments").unwrap_or(&empty);

    let result = match name {
        "list_recent_meetings" => to_tool_result(reader::list_recent_meetings(
            fs,
            root,
            read_limit(arguments),
        )),
        "search_meetings" => {
            let query = required_str(arguments, "query")?;
            to_tool_result(reader::search_sessions(
                fs,
                root,
                query,
                read_limit(arguments),
            ))
        }
        "get_meeting" => {
            let id = required_str(arguments, "id")?;
            to_tool_result(reader::get_meeting(fs, root, id))
        }
        "get_meeting_notes" => {
            let id = required_str(arguments, "id")?;
            to_tool_result(reader::get_meeting_notes(fs, root, id))
        }
        "get_meeting_transcript" => {
            let id = required_str(arguments, "id")?;
            to_tool_result(reader::get_meeting_transcript(fs, root, id))
        }
        other => tool_error_envelope(&format!("unknown tool: {other}")),
    };
    Ok(result)
}

fn required_str<'a>(arguments: &'a Value, key: &str) -> Result<&'a str, (i64, String)> {
    arguments.get(key).and_then(Value::as_str).ok_or_else(|| {
        (
            INVALID_PARAMS,
            format!("missing or non-string params.arguments.{key}"),
        )
    })
}

fn read_limit(arguments: &Value) -> usize {
    let requested = arguments
        .get("limit")
        .and_then(Value::as_u64)
        .and_then(|v| usize::try_from(v).ok())
        .unwrap_or(DEFAULT_LIST_LIMIT);
    requested.clamp(1, MAX_LIST_LIMIT)
}

fn to_tool_result<T: Serialize>(outcome: Result<T, AgentAccessError>) -> Value {
    match outcome {
        Ok(payload) => match serde_json::to_value(payload) {
            Ok(value) => tool_ok_envelope(&value),
            Err(e) => tool_error_envelope(&format!("serializing tool result: {e}")),
        },
        Err(e) => tool_error_envelope(&e.to_string()),
    }
}

fn tool_ok_envelope(payload: &Value) -> Value {
    json!({
        "schema_version": SCHEMA_VERSION,
        "isError": false,
        "content": [ { "type": "text", "text": payload.to_string() } ],
    })
}

fn tool_error_envelope(message: &str) -> Value {
    json!({
        "schema_version": SCHEMA_VERSION,
        "isError": true,
        "content": [ { "type": "text", "text": message } ],
    })
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use crate::agent_access::fs::RealReadOnlyFs;
    use crate::agent_access::test_support::{
        snapshot_tree, write_finished_session, write_unfinished_session,
    };

    fn fs() -> RealReadOnlyFs {
        RealReadOnlyFs
    }

    fn call(fs: &dyn ReadOnlyFs, root: &Path, line: &str) -> Value {
        let response = handle_message(fs, root, line).expect("expected a response line");
        serde_json::from_str(&response).unwrap()
    }

    #[test]
    fn test_initialize_returns_schema_version_and_protocol_version() {
        let dir = tempfile::tempdir().unwrap();

        let response = call(
            &fs(),
            dir.path(),
            r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}"#,
        );

        assert_eq!(response["result"]["schema_version"], SCHEMA_VERSION);
        assert_eq!(response["result"]["protocolVersion"], PROTOCOL_VERSION);
        assert_eq!(response["id"], 1);
    }

    #[test]
    fn test_ping_returns_schema_version() {
        let dir = tempfile::tempdir().unwrap();

        let response = call(
            &fs(),
            dir.path(),
            r#"{"jsonrpc":"2.0","id":2,"method":"ping"}"#,
        );

        assert_eq!(response["result"]["schema_version"], SCHEMA_VERSION);
    }

    #[test]
    fn test_tools_list_returns_exactly_the_five_required_tools_with_schema_version() {
        let dir = tempfile::tempdir().unwrap();

        let response = call(
            &fs(),
            dir.path(),
            r#"{"jsonrpc":"2.0","id":3,"method":"tools/list"}"#,
        );

        assert_eq!(response["result"]["schema_version"], SCHEMA_VERSION);
        let tools = response["result"]["tools"].as_array().unwrap();
        let names: Vec<&str> = tools.iter().map(|t| t["name"].as_str().unwrap()).collect();
        assert_eq!(
            names,
            vec![
                "list_recent_meetings",
                "search_meetings",
                "get_meeting",
                "get_meeting_notes",
                "get_meeting_transcript",
            ]
        );
    }

    #[test]
    fn test_tools_call_list_recent_meetings_returns_versioned_envelope_with_meeting() {
        let dir = tempfile::tempdir().unwrap();
        write_finished_session(
            dir.path(),
            "2026-01-01-0900-standup-01AAA",
            "Standup",
            60,
            "",
            "",
        );

        let response = call(
            &fs(),
            dir.path(),
            r#"{"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"list_recent_meetings","arguments":{}}}"#,
        );

        let result = &response["result"];
        assert_eq!(result["schema_version"], SCHEMA_VERSION);
        assert_eq!(result["isError"], false);
        let text = result["content"][0]["text"].as_str().unwrap();
        let meetings: Value = serde_json::from_str(text).unwrap();
        assert_eq!(meetings[0]["title"], "Standup");
    }

    #[test]
    fn test_tools_call_get_meeting_unfinished_reports_status_not_complete() {
        let dir = tempfile::tempdir().unwrap();
        write_unfinished_session(dir.path(), "2026-01-01-0900-crashed-01AAA");

        let response = call(
            &fs(),
            dir.path(),
            r#"{"jsonrpc":"2.0","id":5,"method":"tools/call","params":{"name":"get_meeting","arguments":{"id":"2026-01-01-0900-crashed-01AAA"}}}"#,
        );

        let text = response["result"]["content"][0]["text"].as_str().unwrap();
        let meeting: Value = serde_json::from_str(text).unwrap();
        assert_eq!(meeting["status"], "unfinished");
        assert!(meeting.get("title").is_none());
    }

    #[test]
    fn test_tools_call_get_meeting_not_found_sets_is_error_true() {
        let dir = tempfile::tempdir().unwrap();

        let response = call(
            &fs(),
            dir.path(),
            r#"{"jsonrpc":"2.0","id":6,"method":"tools/call","params":{"name":"get_meeting","arguments":{"id":"missing"}}}"#,
        );

        assert_eq!(response["result"]["isError"], true);
        assert_eq!(response["result"]["schema_version"], SCHEMA_VERSION);
    }

    #[test]
    fn test_tools_call_unknown_tool_name_sets_is_error_true_not_protocol_error() {
        let dir = tempfile::tempdir().unwrap();

        let response = call(
            &fs(),
            dir.path(),
            r#"{"jsonrpc":"2.0","id":7,"method":"tools/call","params":{"name":"delete_meeting","arguments":{}}}"#,
        );

        assert!(response.get("error").is_none());
        assert_eq!(response["result"]["isError"], true);
    }

    #[test]
    fn test_tools_call_missing_name_is_a_protocol_invalid_params_error() {
        let dir = tempfile::tempdir().unwrap();

        let response = call(
            &fs(),
            dir.path(),
            r#"{"jsonrpc":"2.0","id":8,"method":"tools/call","params":{}}"#,
        );

        assert_eq!(response["error"]["code"], INVALID_PARAMS);
    }

    #[test]
    fn test_unknown_method_returns_method_not_found_error() {
        let dir = tempfile::tempdir().unwrap();

        let response = call(
            &fs(),
            dir.path(),
            r#"{"jsonrpc":"2.0","id":9,"method":"shutdown"}"#,
        );

        assert_eq!(response["error"]["code"], METHOD_NOT_FOUND);
    }

    #[test]
    fn test_malformed_json_returns_parse_error() {
        let dir = tempfile::tempdir().unwrap();

        let response = call(&fs(), dir.path(), "{not valid json");

        assert_eq!(response["error"]["code"], PARSE_ERROR);
        assert!(response["id"].is_null());
    }

    #[test]
    fn test_notification_produces_no_response() {
        let dir = tempfile::tempdir().unwrap();

        let response = handle_message(
            &fs(),
            dir.path(),
            r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#,
        );

        assert!(response.is_none());
    }

    #[test]
    fn test_request_with_no_id_produces_no_response() {
        let dir = tempfile::tempdir().unwrap();

        let response = handle_message(&fs(), dir.path(), r#"{"jsonrpc":"2.0","method":"ping"}"#);

        assert!(response.is_none());
    }

    /// The structural mutation-guard test required by M9 acceptance
    /// (b): proves no write, delete, or mutate call path exists in
    /// the served module — not by inspecting source text, but by
    /// exercising every JSON-RPC method and all five tools against a
    /// real fixture tree through the production `RealReadOnlyFs` and
    /// asserting the tree is byte-for-byte identical afterward. Any
    /// mutation anywhere in the call graph, however indirect, would
    /// change the snapshot and fail this test.
    #[test]
    fn test_dispatch_never_mutates_fixture_tree_across_every_method_and_tool() {
        let dir = tempfile::tempdir().unwrap();
        write_finished_session(
            dir.path(),
            "2026-01-01-0900-standup-01AAA",
            "Standup",
            120,
            "notes body",
            "transcript body",
        );
        write_unfinished_session(dir.path(), "2026-01-02-0900-crashed-01BBB");

        let before = snapshot_tree(dir.path());

        let requests = [
            r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}"#,
            r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#,
            r#"{"jsonrpc":"2.0","id":2,"method":"ping"}"#,
            r#"{"jsonrpc":"2.0","id":3,"method":"tools/list"}"#,
            r#"{"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"list_recent_meetings","arguments":{}}}"#,
            r#"{"jsonrpc":"2.0","id":5,"method":"tools/call","params":{"name":"search_meetings","arguments":{"query":"standup"}}}"#,
            r#"{"jsonrpc":"2.0","id":6,"method":"tools/call","params":{"name":"get_meeting","arguments":{"id":"2026-01-01-0900-standup-01AAA"}}}"#,
            r#"{"jsonrpc":"2.0","id":7,"method":"tools/call","params":{"name":"get_meeting","arguments":{"id":"2026-01-02-0900-crashed-01BBB"}}}"#,
            r#"{"jsonrpc":"2.0","id":8,"method":"tools/call","params":{"name":"get_meeting_notes","arguments":{"id":"2026-01-01-0900-standup-01AAA"}}}"#,
            r#"{"jsonrpc":"2.0","id":9,"method":"tools/call","params":{"name":"get_meeting_transcript","arguments":{"id":"2026-01-01-0900-standup-01AAA"}}}"#,
            r#"{"jsonrpc":"2.0","id":10,"method":"tools/call","params":{"name":"get_meeting","arguments":{"id":"nonexistent"}}}"#,
            r#"{"jsonrpc":"2.0","id":11,"method":"tools/call","params":{"name":"not_a_real_tool","arguments":{}}}"#,
            r#"{"jsonrpc":"2.0","id":12,"method":"unsupported_method"}"#,
            "not even json",
        ];
        for request in requests {
            let _ = handle_message(&fs(), dir.path(), request);
        }

        let after = snapshot_tree(dir.path());
        assert_eq!(
            before, after,
            "agent_access dispatch must never mutate the served tree"
        );
    }
}
