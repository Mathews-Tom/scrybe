// Copyright 2026 Mathews Tom
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//     https://www.apache.org/licenses/LICENSE-2.0

//! `IcsFileProvider` — populate `MeetingContext` from a local `.ics`
//! calendar file (`.docs/development-plan.md` §8.2 deliverable #6).
//!
//! Calendar systems vary in how they encode line breaks, timezones,
//! attendee email-vs-CN ordering, and recurring-event metadata. The
//! provider matches by **start-time window** — within `MatchWindow` of
//! the session's `started_at` — so a meeting that begins five minutes
//! late still matches its calendar entry.
//!
//! Recurring events are emitted by every calendar implementation as
//! one `VEVENT` per occurrence (Google + Outlook), or as a single
//! `RRULE`-bearing master with override children (Apple Calendar). The
//! provider treats both shapes uniformly: it matches the closest
//! `DTSTART` to the session start regardless of recurrence shape, and
//! does not expand `RRULE` itself — the burden of producing the
//! per-occurrence event is on the calendar exporter, which every tool
//! does correctly for short windows.

use std::path::PathBuf;
use std::time::Duration;

use async_trait::async_trait;
use chrono::{DateTime, NaiveDate, NaiveDateTime, NaiveTime, TimeZone, Utc};
use ical::IcalParser;

use super::{ContextProvider, MeetingContext};
use crate::error::{ConfigError, CoreError, StorageError};

/// Configuration for [`IcsFileProvider`].
#[derive(Clone, Debug)]
pub struct IcsFileConfig {
    /// Filesystem path to the `.ics` file.
    pub path: PathBuf,
    /// Window around the session's `started_at` within which a calendar
    /// `VEVENT.DTSTART` is considered a match. Defaults to
    /// [`MatchWindow::default`] (15 minutes).
    pub match_window: MatchWindow,
}

/// Symmetric window around the session start. The provider matches any
/// `VEVENT` whose `DTSTART` lies within `±half_window` of `started_at`.
#[derive(Clone, Copy, Debug)]
pub struct MatchWindow {
    pub half_window: Duration,
}

impl Default for MatchWindow {
    fn default() -> Self {
        Self {
            half_window: Duration::from_secs(15 * 60),
        }
    }
}

/// Calendar-context source.
pub struct IcsFileProvider {
    config: IcsFileConfig,
}

impl IcsFileProvider {
    /// Construct from a config. The provider does not read the file
    /// until [`context_for`] is called, so configuration errors and
    /// I/O errors surface at the right place in the session lifecycle.
    ///
    /// [`context_for`]: ContextProvider::context_for
    #[must_use]
    pub const fn new(config: IcsFileConfig) -> Self {
        Self { config }
    }
}

#[async_trait]
impl ContextProvider for IcsFileProvider {
    async fn context_for(&self, started_at: DateTime<Utc>) -> Result<MeetingContext, CoreError> {
        if !self.config.path.exists() {
            return Err(CoreError::Config(ConfigError::NotFound {
                path: self.config.path.clone(),
            }));
        }
        let bytes = std::fs::read(&self.config.path).map_err(|e| match e.kind() {
            std::io::ErrorKind::NotFound => CoreError::Config(ConfigError::NotFound {
                path: self.config.path.clone(),
            }),
            _ => CoreError::Storage(StorageError::Io(e)),
        })?;
        let context =
            scan_for_match(&bytes, started_at, self.config.match_window).unwrap_or_default();
        Ok(context)
    }
}

/// Best-effort scan for a `VEVENT` whose `DTSTART` falls within
/// `match_window` of `started_at`. Returns the populated
/// `MeetingContext` for the closest matching event, or `None` if none
/// match.
fn scan_for_match(
    bytes: &[u8],
    started_at: DateTime<Utc>,
    window: MatchWindow,
) -> Option<MeetingContext> {
    let parser = IcalParser::new(std::io::BufReader::new(bytes));

    let mut best: Option<(i64, MeetingContext)> = None;
    let half = i64::try_from(window.half_window.as_secs()).unwrap_or(i64::MAX);

    for calendar in parser.flatten() {
        for event in &calendar.events {
            let Some(dtstart) = property_value(&event.properties, "DTSTART") else {
                continue;
            };
            let Some(start) = parse_calendar_datetime(&dtstart) else {
                continue;
            };
            let delta = (start - started_at).num_seconds().abs();
            if delta > half {
                continue;
            }
            let ctx = build_context(event);
            match best {
                Some((best_delta, _)) if best_delta <= delta => {}
                _ => best = Some((delta, ctx)),
            }
        }
    }

    best.map(|(_, ctx)| ctx)
}

fn build_context(event: &ical::parser::ical::component::IcalEvent) -> MeetingContext {
    let title = property_value(&event.properties, "SUMMARY");
    let description = property_value(&event.properties, "DESCRIPTION");
    let mut attendees: Vec<String> = event
        .properties
        .iter()
        .filter(|p| p.name == "ATTENDEE")
        .filter_map(extract_attendee)
        .collect();
    attendees.sort();
    attendees.dedup();

    MeetingContext {
        title,
        attendees,
        agenda: description,
        ..MeetingContext::default()
    }
}

fn property_value(properties: &[ical::property::Property], name: &str) -> Option<String> {
    properties
        .iter()
        .find(|p| p.name == name)
        .and_then(|p| p.value.clone())
}

/// Pull a human-friendly attendee string out of an `ATTENDEE` property.
/// Calendar exports vary — Google emits `ATTENDEE;CN=Tom:mailto:tom@example.com`,
/// Outlook emits `ATTENDEE;CN="Tom Mathews":mailto:tom@example.com`,
/// Apple Calendar emits `ATTENDEE:mailto:tom@example.com`. This
/// extractor prefers `CN=`, falls back to the local-part of the
/// `mailto:` URI, and finally to the raw value.
fn extract_attendee(property: &ical::property::Property) -> Option<String> {
    if let Some(params) = &property.params {
        for (name, values) in params {
            if name.eq_ignore_ascii_case("CN") {
                if let Some(first) = values.first() {
                    let cleaned = first.trim_matches('"').trim();
                    if !cleaned.is_empty() {
                        return Some(cleaned.to_string());
                    }
                }
            }
        }
    }
    let value = property.value.as_deref()?;
    if let Some(rest) = value.strip_prefix("mailto:") {
        if let Some((local, _)) = rest.split_once('@') {
            if !local.is_empty() {
                return Some(local.to_string());
            }
        }
        return Some(rest.to_string());
    }
    Some(value.to_string())
}

/// Parse the subset of `DTSTART`/`DTEND` formats that calendar
/// exporters actually emit.
///
/// Supported:
/// - `YYYYMMDDTHHMMSSZ` (Google, UTC; the most common shape)
/// - `YYYYMMDDTHHMMSS` (floating local time — interpreted as UTC; the
///   provider does not have a session-local timezone available, and
///   ±15 minutes of slop covers most BST/PST boundary cases)
/// - `YYYYMMDD` (all-day events; treated as midnight UTC)
fn parse_calendar_datetime(value: &str) -> Option<DateTime<Utc>> {
    let trimmed = value.trim();
    if let Ok(dt) = NaiveDateTime::parse_from_str(trimmed, "%Y%m%dT%H%M%SZ") {
        return Some(Utc.from_utc_datetime(&dt));
    }
    if let Ok(dt) = NaiveDateTime::parse_from_str(trimmed, "%Y%m%dT%H%M%S") {
        return Some(Utc.from_utc_datetime(&dt));
    }
    if let Ok(date) = NaiveDate::parse_from_str(trimmed, "%Y%m%d") {
        let dt = NaiveDateTime::new(date, NaiveTime::MIN);
        return Some(Utc.from_utc_datetime(&dt));
    }
    None
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use std::io::Write;

    use super::*;
    use chrono::TimeZone;
    use pretty_assertions::assert_eq;

    fn write_ics(content: &str) -> tempfile::NamedTempFile {
        let mut f = tempfile::NamedTempFile::new().unwrap();
        f.write_all(content.as_bytes()).unwrap();
        f
    }

    fn started_at(year: i32, month: u32, day: u32, hour: u32, minute: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(year, month, day, hour, minute, 0)
            .unwrap()
    }

    #[test]
    fn test_parse_calendar_datetime_handles_utc_shape() {
        let dt = parse_calendar_datetime("20260429T143000Z").unwrap();

        assert_eq!(dt, started_at(2026, 4, 29, 14, 30));
    }

    #[test]
    fn test_parse_calendar_datetime_handles_floating_shape() {
        let dt = parse_calendar_datetime("20260429T143000").unwrap();

        assert_eq!(dt, started_at(2026, 4, 29, 14, 30));
    }

    #[test]
    fn test_parse_calendar_datetime_handles_date_only_shape_as_midnight_utc() {
        let dt = parse_calendar_datetime("20260429").unwrap();

        assert_eq!(dt, Utc.with_ymd_and_hms(2026, 4, 29, 0, 0, 0).unwrap());
    }

    #[test]
    fn test_parse_calendar_datetime_returns_none_for_garbage() {
        assert!(parse_calendar_datetime("not a date").is_none());
    }

    #[tokio::test]
    async fn test_ics_provider_returns_empty_context_when_file_missing_event_match() {
        let ics = write_ics(
            "BEGIN:VCALENDAR\r\n\
             VERSION:2.0\r\n\
             BEGIN:VEVENT\r\n\
             UID:1@scrybe-test\r\n\
             DTSTART:20260101T100000Z\r\n\
             DTEND:20260101T110000Z\r\n\
             SUMMARY:Different day\r\n\
             END:VEVENT\r\n\
             END:VCALENDAR\r\n",
        );

        let provider = IcsFileProvider::new(IcsFileConfig {
            path: ics.path().to_owned(),
            match_window: MatchWindow::default(),
        });

        let ctx = provider
            .context_for(started_at(2026, 4, 29, 14, 30))
            .await
            .unwrap();

        assert_eq!(ctx, MeetingContext::default());
    }

    #[tokio::test]
    async fn test_ics_provider_matches_event_within_window_and_returns_summary() {
        let ics = write_ics(
            "BEGIN:VCALENDAR\r\n\
             VERSION:2.0\r\n\
             BEGIN:VEVENT\r\n\
             UID:1@scrybe-test\r\n\
             DTSTART:20260429T143000Z\r\n\
             SUMMARY:Acme discovery\r\n\
             DESCRIPTION:Walk through the proposal.\r\n\
             ATTENDEE;CN=Tom Mathews:mailto:tom@example.com\r\n\
             ATTENDEE:mailto:alex@example.com\r\n\
             END:VEVENT\r\n\
             END:VCALENDAR\r\n",
        );

        let provider = IcsFileProvider::new(IcsFileConfig {
            path: ics.path().to_owned(),
            match_window: MatchWindow::default(),
        });

        let ctx = provider
            .context_for(started_at(2026, 4, 29, 14, 32))
            .await
            .unwrap();

        assert_eq!(ctx.title.as_deref(), Some("Acme discovery"));
        assert_eq!(ctx.agenda.as_deref(), Some("Walk through the proposal."));
        assert_eq!(ctx.attendees, vec!["Tom Mathews", "alex"]);
    }

    #[tokio::test]
    async fn test_ics_provider_picks_closest_event_when_multiple_match_window() {
        let ics = write_ics(
            "BEGIN:VCALENDAR\r\n\
             VERSION:2.0\r\n\
             BEGIN:VEVENT\r\n\
             UID:1@scrybe-test\r\n\
             DTSTART:20260429T142500Z\r\n\
             SUMMARY:Earlier match\r\n\
             END:VEVENT\r\n\
             BEGIN:VEVENT\r\n\
             UID:2@scrybe-test\r\n\
             DTSTART:20260429T143200Z\r\n\
             SUMMARY:Closer match\r\n\
             END:VEVENT\r\n\
             END:VCALENDAR\r\n",
        );

        let provider = IcsFileProvider::new(IcsFileConfig {
            path: ics.path().to_owned(),
            match_window: MatchWindow::default(),
        });

        let ctx = provider
            .context_for(started_at(2026, 4, 29, 14, 30))
            .await
            .unwrap();

        assert_eq!(ctx.title.as_deref(), Some("Closer match"));
    }

    #[tokio::test]
    async fn test_ics_provider_returns_not_found_for_missing_path() {
        let provider = IcsFileProvider::new(IcsFileConfig {
            path: PathBuf::from("/nonexistent/calendar.ics"),
            match_window: MatchWindow::default(),
        });

        let err = provider
            .context_for(started_at(2026, 4, 29, 14, 30))
            .await
            .unwrap_err();

        match err {
            CoreError::Config(ConfigError::NotFound { .. }) => {}
            other => panic!("expected NotFound, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn test_ics_provider_handles_outlook_quoted_cn_format() {
        let ics = write_ics(
            "BEGIN:VCALENDAR\r\n\
             VERSION:2.0\r\n\
             BEGIN:VEVENT\r\n\
             UID:1@scrybe-test\r\n\
             DTSTART:20260429T143000Z\r\n\
             SUMMARY:Outlook export\r\n\
             ATTENDEE;CN=\"Tom, Mathews\":mailto:tom@example.com\r\n\
             END:VEVENT\r\n\
             END:VCALENDAR\r\n",
        );

        let provider = IcsFileProvider::new(IcsFileConfig {
            path: ics.path().to_owned(),
            match_window: MatchWindow::default(),
        });

        let ctx = provider
            .context_for(started_at(2026, 4, 29, 14, 30))
            .await
            .unwrap();

        assert_eq!(ctx.attendees, vec!["Tom, Mathews"]);
    }

    #[tokio::test]
    async fn test_ics_provider_dedupes_attendee_list() {
        let ics = write_ics(
            "BEGIN:VCALENDAR\r\n\
             VERSION:2.0\r\n\
             BEGIN:VEVENT\r\n\
             UID:1@scrybe-test\r\n\
             DTSTART:20260429T143000Z\r\n\
             SUMMARY:Test\r\n\
             ATTENDEE;CN=Tom:mailto:tom@example.com\r\n\
             ATTENDEE;CN=Tom:mailto:tom@example.com\r\n\
             END:VEVENT\r\n\
             END:VCALENDAR\r\n",
        );

        let provider = IcsFileProvider::new(IcsFileConfig {
            path: ics.path().to_owned(),
            match_window: MatchWindow::default(),
        });

        let ctx = provider
            .context_for(started_at(2026, 4, 29, 14, 30))
            .await
            .unwrap();

        assert_eq!(ctx.attendees, vec!["Tom"]);
    }
}
