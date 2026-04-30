// Copyright 2026 Mathews Tom
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//     https://www.apache.org/licenses/LICENSE-2.0

//! Global hotkey listener for record start/stop toggle.
//!
//! Compiled only with the `cli-shell` cargo feature. Wraps
//! `global_hotkey::GlobalHotKeyManager` (`!Send` on macOS — the
//! Carbon event handler is keyed to the thread that created it) and
//! exposes hotkey events as a `crossbeam_channel::Receiver` so the
//! main-thread shell driver (`scrybe-cli::shell`) can poll without
//! holding the manager across `await` points.
//!
//! Accelerator grammar follows the tao/tauri convention used by both
//! `tray-icon` and `global-hotkey`: e.g. `CmdOrCtrl+Shift+R`,
//! `Alt+F4`, `Super+Space`.

use std::str::FromStr;

use anyhow::{anyhow, Context, Result};
use crossbeam_channel::Receiver;
use global_hotkey::{hotkey::HotKey, GlobalHotKeyEvent, GlobalHotKeyManager, HotKeyState};

/// Default toggle accelerator when the user has not customised
/// `[capture] hotkey` in `config.toml`. Modifiers map to the platform
/// primary metakey: `Cmd` on macOS, `Ctrl` on Linux/Windows.
pub const DEFAULT_TOGGLE_ACCELERATOR: &str = "CmdOrCtrl+Shift+R";

/// Events surfaced by the global hotkey to the recording loop.
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum HotkeyEvent {
    /// User pressed the configured toggle accelerator.
    Toggle,
}

/// Global hotkey listener. Constructed on the thread that owns the
/// platform run loop; never moved across threads.
pub struct HotkeyListener {
    // Held for its `Drop` impl, which unregisters the hotkey from the
    // OS. The field is never read directly — the global
    // `GlobalHotKeyEvent::receiver()` queue is what surfaces events.
    #[allow(dead_code)]
    manager: GlobalHotKeyManager,
    hotkey_id: u32,
    events: Receiver<GlobalHotKeyEvent>,
}

impl HotkeyListener {
    /// Validate the accelerator string and register it with the OS.
    ///
    /// # Errors
    ///
    /// Returns an error when the accelerator string is malformed or
    /// when the OS rejects the registration (e.g. the combination is
    /// already claimed by another app).
    pub fn start(accelerator: &str) -> Result<Self> {
        validate_accelerator_syntax(accelerator)?;

        let manager = GlobalHotKeyManager::new().context("creating global hotkey manager")?;
        let hotkey = HotKey::from_str(accelerator)
            .with_context(|| format!("parsing hotkey accelerator '{accelerator}'"))?;
        let hotkey_id = hotkey.id();
        manager
            .register(hotkey)
            .with_context(|| format!("registering hotkey '{accelerator}'"))?;

        let events = GlobalHotKeyEvent::receiver().clone();
        Ok(Self {
            manager,
            hotkey_id,
            events,
        })
    }

    /// Drain pending hotkey events without blocking, returning the
    /// first `HotkeyEvent::Toggle` that matches the registered ID and
    /// the press half-cycle. Returns `None` when the queue is empty.
    pub fn poll(&self) -> Option<HotkeyEvent> {
        while let Ok(event) = self.events.try_recv() {
            if event.id == self.hotkey_id && event.state == HotKeyState::Pressed {
                return Some(HotkeyEvent::Toggle);
            }
        }
        None
    }
}

/// Validate accelerator syntax without registering. Used both as a
/// pre-flight check before the OS registration call and so a config
/// error surfaces with a meaningful message rather than the
/// `global-hotkey` crate's lower-level parse failure.
///
/// The grammar is `<modifier>+<modifier>+...+<key>` with at least one
/// non-modifier key. Empty strings, segments, or pure-modifier
/// accelerators are rejected.
fn validate_accelerator_syntax(accelerator: &str) -> Result<()> {
    let trimmed = accelerator.trim();
    if trimmed.is_empty() {
        return Err(anyhow!("hotkey accelerator must not be empty"));
    }
    let segments: Vec<&str> = trimmed.split('+').map(str::trim).collect();
    if segments.iter().any(|s| s.is_empty()) {
        return Err(anyhow!(
            "hotkey accelerator '{accelerator}' contains an empty segment"
        ));
    }
    let last = segments
        .last()
        .copied()
        .ok_or_else(|| anyhow!("hotkey accelerator '{accelerator}' has no key segment"))?;
    if is_modifier_token(last) {
        return Err(anyhow!(
            "hotkey accelerator '{accelerator}' must terminate in a non-modifier key"
        ));
    }
    Ok(())
}

fn is_modifier_token(token: &str) -> bool {
    matches!(
        token.to_ascii_lowercase().as_str(),
        "shift"
            | "ctrl"
            | "control"
            | "alt"
            | "option"
            | "super"
            | "meta"
            | "cmd"
            | "command"
            | "cmdorctrl"
            | "commandorcontrol"
    )
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    #[test]
    fn test_validate_accelerator_syntax_accepts_default_toggle() {
        validate_accelerator_syntax(DEFAULT_TOGGLE_ACCELERATOR).unwrap();
    }

    #[test]
    fn test_validate_accelerator_syntax_accepts_alt_function_key() {
        validate_accelerator_syntax("Alt+F4").unwrap();
    }

    #[test]
    fn test_validate_accelerator_syntax_accepts_super_space() {
        validate_accelerator_syntax("Super+Space").unwrap();
    }

    #[test]
    fn test_validate_accelerator_syntax_rejects_empty_string() {
        let err = validate_accelerator_syntax("").unwrap_err();

        assert!(err.to_string().contains("must not be empty"));
    }

    #[test]
    fn test_validate_accelerator_syntax_rejects_pure_modifier_chord() {
        let err = validate_accelerator_syntax("Ctrl+Shift").unwrap_err();

        assert!(err.to_string().contains("non-modifier"));
    }

    #[test]
    fn test_validate_accelerator_syntax_rejects_empty_segment() {
        let err = validate_accelerator_syntax("Ctrl++R").unwrap_err();

        assert!(err.to_string().contains("empty segment"));
    }

    #[test]
    fn test_is_modifier_token_recognises_cmdorctrl_alias() {
        assert!(is_modifier_token("CmdOrCtrl"));
    }

    #[test]
    fn test_is_modifier_token_rejects_letter_key() {
        assert!(!is_modifier_token("R"));
    }

    #[test]
    fn test_default_toggle_accelerator_constant_validates_as_a_well_formed_accelerator() {
        validate_accelerator_syntax(DEFAULT_TOGGLE_ACCELERATOR).unwrap();
        assert!(DEFAULT_TOGGLE_ACCELERATOR.contains('+'));
    }
}
