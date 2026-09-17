// Copyright 2026 Mathews Tom
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//     https://www.apache.org/licenses/LICENSE-2.0

//! Read-only local-agent access over the configured storage root.
//!
//! A compatible agent can search and read local meeting artifacts
//! through a small stdio JSON-RPC/MCP surface. There is no write,
//! delete, or mutate capability anywhere in this module, and no network
//! listener — the transport is stdin and stdout only, wired up by
//! `scrybe-cli`'s `mcp` command, which refuses to start the server
//! unless `[agent_access].enabled = true`.
//!
//! The mutation-free guarantee is structural. [`handle_message`] is
//! handed a [`SessionReader`](crate::sessions::SessionReader), whose
//! trait surface has no method that repairs a session, regenerates
//! notes, writes configuration, or changes anything else. No handler in
//! this module could perform a mutation however it were written,
//! because there is nothing to call.
//!
//! The reads themselves are the repository's, so an agent sees the same
//! classification, confinement, paging, and cache behavior the CLI
//! sees.
//!
//! Compiled only when the `agent-access` feature is enabled; the
//! feature is off by default, so a default build carries none of this
//! surface.

mod protocol;

pub use protocol::{handle_message, MeetingContent, MeetingSummary, SessionStatus, SCHEMA_VERSION};
