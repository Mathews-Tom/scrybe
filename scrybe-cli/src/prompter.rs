// Copyright 2026 Mathews Tom
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//     https://www.apache.org/licenses/LICENSE-2.0

//! TTY-backed `ConsentPrompter` for the CLI.

use async_trait::async_trait;
use scrybe_core::consent::ConsentPrompter;
use scrybe_core::error::ConsentError;
use scrybe_core::types::ConsentMode;

/// Prompts on stdin/stdout. Accepts `y`/`yes` (case-insensitive); any
/// other response declines.
pub struct TtyPrompter {
    pub auto_accept: bool,
}

impl TtyPrompter {
    #[must_use]
    pub const fn new(auto_accept: bool) -> Self {
        Self { auto_accept }
    }
}

#[async_trait]
impl ConsentPrompter for TtyPrompter {
    async fn prompt(&self, mode: ConsentMode) -> Result<(), ConsentError> {
        if self.auto_accept {
            return Ok(());
        }
        let body = format!(
            "scrybe is about to start capturing audio in {mode} mode.\n\
             Press 'y' to confirm, anything else to abort: "
        );
        let answer = tokio::task::spawn_blocking(move || -> Result<String, ConsentError> {
            use std::io::Write;
            let stdout = std::io::stdout();
            let mut handle = stdout.lock();
            handle
                .write_all(body.as_bytes())
                .map_err(|e| ConsentError::TtsUnavailable(e.to_string()))?;
            handle
                .flush()
                .map_err(|e| ConsentError::TtsUnavailable(e.to_string()))?;
            drop(handle);

            let mut buf = String::new();
            std::io::stdin()
                .read_line(&mut buf)
                .map_err(|e| ConsentError::TtsUnavailable(e.to_string()))?;
            Ok(buf)
        })
        .await
        .map_err(|e| ConsentError::TtsUnavailable(e.to_string()))??;

        let trimmed = answer.trim().to_ascii_lowercase();
        if trimmed == "y" || trimmed == "yes" {
            Ok(())
        } else {
            Err(ConsentError::UserAborted)
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    #[tokio::test]
    async fn test_tty_prompter_with_auto_accept_returns_ok_without_reading_stdin() {
        let prompter = TtyPrompter::new(true);

        let result = prompter.prompt(ConsentMode::Quick).await;

        assert!(result.is_ok());
    }

    #[test]
    fn test_tty_prompter_constructor_records_auto_accept_flag() {
        let prompter = TtyPrompter::new(false);

        assert_eq!(prompter.auto_accept, false);
    }

    #[tokio::test]
    async fn test_tty_prompter_with_auto_accept_returns_ok_for_announce_mode() {
        let prompter = TtyPrompter::new(true);

        let result = prompter.prompt(ConsentMode::Announce).await;

        assert!(result.is_ok());
    }
}
