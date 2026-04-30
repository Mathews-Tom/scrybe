// Copyright 2026 Mathews Tom
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//     https://www.apache.org/licenses/LICENSE-2.0

//! TTY-backed `ConsentPrompter` for the CLI.

use std::io::{BufRead, Write};

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
        let body = render_prompt_body(mode);
        tokio::task::spawn_blocking(move || -> Result<(), ConsentError> {
            let stdin = std::io::stdin();
            let stdout = std::io::stdout();
            read_consent_blocking(&body, stdout.lock(), stdin.lock())
        })
        .await
        .map_err(|e| ConsentError::TtsUnavailable(e.to_string()))?
    }
}

fn render_prompt_body(mode: ConsentMode) -> String {
    format!(
        "scrybe is about to start capturing audio in {mode} mode.\n\
         Press 'y' to confirm, anything else to abort: "
    )
}

fn parse_consent_response(answer: &str) -> Result<(), ConsentError> {
    let trimmed = answer.trim().to_ascii_lowercase();
    if trimmed == "y" || trimmed == "yes" {
        Ok(())
    } else {
        Err(ConsentError::UserAborted)
    }
}

fn read_consent_blocking<W: Write, R: BufRead>(
    body: &str,
    mut writer: W,
    mut reader: R,
) -> Result<(), ConsentError> {
    writer
        .write_all(body.as_bytes())
        .map_err(|e| ConsentError::TtsUnavailable(e.to_string()))?;
    writer
        .flush()
        .map_err(|e| ConsentError::TtsUnavailable(e.to_string()))?;
    let mut buf = String::new();
    reader
        .read_line(&mut buf)
        .map_err(|e| ConsentError::TtsUnavailable(e.to_string()))?;
    parse_consent_response(&buf)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;
    use std::io::Cursor;

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

    #[test]
    fn test_render_prompt_body_includes_quick_mode_label() {
        let body = render_prompt_body(ConsentMode::Quick);

        assert!(body.contains("quick"));
        assert!(body.contains("Press 'y' to confirm"));
    }

    #[test]
    fn test_render_prompt_body_includes_notify_mode_label() {
        let body = render_prompt_body(ConsentMode::Notify);

        assert!(body.contains("notify"));
    }

    #[test]
    fn test_render_prompt_body_includes_announce_mode_label() {
        let body = render_prompt_body(ConsentMode::Announce);

        assert!(body.contains("announce"));
    }

    #[test]
    fn test_parse_consent_response_lowercase_y_returns_ok() {
        assert!(parse_consent_response("y\n").is_ok());
    }

    #[test]
    fn test_parse_consent_response_uppercase_y_returns_ok() {
        assert!(parse_consent_response("Y\n").is_ok());
    }

    #[test]
    fn test_parse_consent_response_lowercase_yes_returns_ok() {
        assert!(parse_consent_response("yes\n").is_ok());
    }

    #[test]
    fn test_parse_consent_response_uppercase_yes_returns_ok() {
        assert!(parse_consent_response("YES\n").is_ok());
    }

    #[test]
    fn test_parse_consent_response_yes_with_surrounding_whitespace_returns_ok() {
        assert!(parse_consent_response("  yes  \n").is_ok());
    }

    #[test]
    fn test_parse_consent_response_n_returns_user_aborted() {
        let err = parse_consent_response("n\n").unwrap_err();

        assert!(matches!(err, ConsentError::UserAborted));
    }

    #[test]
    fn test_parse_consent_response_empty_returns_user_aborted() {
        let err = parse_consent_response("\n").unwrap_err();

        assert!(matches!(err, ConsentError::UserAborted));
    }

    #[test]
    fn test_parse_consent_response_arbitrary_word_returns_user_aborted() {
        let err = parse_consent_response("maybe\n").unwrap_err();

        assert!(matches!(err, ConsentError::UserAborted));
    }

    #[test]
    fn test_read_consent_blocking_writes_body_and_returns_ok_on_y() {
        let body = "prompt: ";
        let mut writer = Vec::<u8>::new();
        let reader = Cursor::new(b"y\n");

        let result = read_consent_blocking(body, &mut writer, reader);

        assert!(result.is_ok());
        assert_eq!(writer, body.as_bytes());
    }

    #[test]
    fn test_read_consent_blocking_returns_user_aborted_on_n() {
        let body = "prompt: ";
        let mut writer = Vec::<u8>::new();
        let reader = Cursor::new(b"n\n");

        let err = read_consent_blocking(body, &mut writer, reader).unwrap_err();

        assert!(matches!(err, ConsentError::UserAborted));
    }

    #[test]
    fn test_read_consent_blocking_returns_user_aborted_on_empty_input() {
        let body = "prompt: ";
        let mut writer = Vec::<u8>::new();
        let reader = Cursor::new(b"");

        let err = read_consent_blocking(body, &mut writer, reader).unwrap_err();

        assert!(matches!(err, ConsentError::UserAborted));
    }

    #[test]
    fn test_read_consent_blocking_propagates_writer_error_as_tts_unavailable() {
        struct BrokenWriter;
        impl Write for BrokenWriter {
            fn write(&mut self, _: &[u8]) -> std::io::Result<usize> {
                Err(std::io::Error::other("disk full"))
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }

        let err = read_consent_blocking("prompt: ", BrokenWriter, Cursor::new(b"y\n")).unwrap_err();

        assert!(matches!(err, ConsentError::TtsUnavailable(_)));
    }

    #[test]
    fn test_read_consent_blocking_propagates_reader_error_as_tts_unavailable() {
        struct BrokenReader;
        impl std::io::Read for BrokenReader {
            fn read(&mut self, _: &mut [u8]) -> std::io::Result<usize> {
                Err(std::io::Error::other("stdin closed"))
            }
        }
        impl BufRead for BrokenReader {
            fn fill_buf(&mut self) -> std::io::Result<&[u8]> {
                Err(std::io::Error::other("stdin closed"))
            }
            fn consume(&mut self, _: usize) {}
        }

        let mut writer = Vec::<u8>::new();
        let err = read_consent_blocking("prompt: ", &mut writer, BrokenReader).unwrap_err();

        assert!(matches!(err, ConsentError::TtsUnavailable(_)));
    }
}
