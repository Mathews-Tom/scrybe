// Copyright 2026 Mathews Tom
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//     https://www.apache.org/licenses/LICENSE-2.0

//! Read-only local-agent access over `~/scrybe/` (`.docs/DEVELOPMENT_PLAN.md` §6 M9).
//!
//! A compatible agent can search and read local meeting artifacts
//! through a small stdio JSON-RPC/MCP surface; there is no write,
//! delete, or mutate capability anywhere in this module, and no
//! network listener — the transport is stdin/stdout only, wired up by
//! `scrybe-cli`'s `mcp` command, which also refuses to start the
//! server unless `[agent_access].enabled = true` in config.
//!
//! Compiled only when the `agent-access` feature is enabled; the
//! feature is off by default, so a default build carries none of this
//! surface.
//!
//! - [`fs::ReadOnlyFs`] is the capability-limited filesystem
//!   abstraction every read in this module goes through. It exposes
//!   no method that could create, modify, rename, or delete anything.
//! - [`reader`] walks the session-directory layout `scrybe-core::session`
//!   and `scrybe-cli`'s `list`/`show` commands already use, reporting
//!   an unfinished session explicitly rather than serving it as
//!   complete.
//! - [`protocol`] implements JSON-RPC 2.0 and the minimal MCP surface
//!   (`initialize`, `tools/list`, `tools/call`, `ping`) for the five
//!   required tools, with a `schema_version` on every response.

pub mod fs;
pub mod protocol;
pub mod reader;

#[cfg(test)]
mod test_support;

pub use fs::{ReadOnlyFs, RealReadOnlyFs};
pub use protocol::{handle_message, SCHEMA_VERSION};
pub use reader::{AgentAccessError, MeetingContent, MeetingSummary, SessionStatus};
