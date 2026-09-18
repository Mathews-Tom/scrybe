// Copyright 2026 Mathews Tom
// Licensed under the Apache License, Version 2.0 (the "License");

#![allow(unsafe_code)]
//! Main-thread `AppKit` panel for one active recording session.

use anyhow::{anyhow, Result};
use crossbeam_channel::{bounded, Receiver, Sender};
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, NSObjectProtocol};
use objc2::{define_class, msg_send, sel, DefinedClass, MainThreadOnly};
use objc2_app_kit::{
    NSAccessibility, NSApplication, NSApplicationActivationPolicy, NSBackingStoreType,
    NSBezelStyle, NSBox, NSBoxType, NSButton, NSColor, NSFloatingWindowLevel, NSPanel, NSScreen,
    NSTextAlignment, NSTextField, NSTitlePosition, NSWindowCollectionBehavior, NSWindowStyleMask,
    NSWorkspace,
};
use objc2_foundation::{MainThreadMarker, NSObject, NSPoint, NSRect, NSSize, NSString};

use crate::view::{ShellState, ShellView};

const PANEL_WIDTH: f64 = 240.0;
const PANEL_HEIGHT: f64 = 44.0;
const TOP_INSET: f64 = 24.0;

define_class!(
    // SAFETY: NSObject has no subclassing requirements and this class has no Drop impl.
    #[unsafe(super = NSObject)]
    #[name = "ScrybeRecordingStopTarget"]
    #[thread_kind = MainThreadOnly]
    #[ivars = Sender<()>]
    struct StopTarget;

    // SAFETY: NSObjectProtocol has no additional safety requirements.
    unsafe impl NSObjectProtocol for StopTarget {}

    impl StopTarget {
        #[unsafe(method(stopRecording:))]
        fn stop_recording(&self, _sender: Option<&AnyObject>) {
            let _ = self.ivars().try_send(());
        }
    }
);

impl StopTarget {
    fn new(mtm: MainThreadMarker, sender: Sender<()>) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(sender);
        // SAFETY: NSObject's `init` signature and ownership family are correct.
        unsafe { msg_send![super(this), init] }
    }
}

/// Session-local non-activating panel. Must remain on the macOS main thread.
pub struct FloatingPanel {
    panel: Retained<NSPanel>,
    recording_dot: Retained<NSTextField>,
    state_label: Retained<NSTextField>,
    elapsed_label: Retained<NSTextField>,
    stop_button: Retained<NSButton>,
    _target: Retained<StopTarget>,
    stop_events: Receiver<()>,
    last_state: ShellState,
    last_elapsed_second: u64,
}

/// Register the command-line process as an `AppKit` accessory application.
///
/// # Errors
///
/// Returns an error when called off the macOS main thread.
pub fn prepare_application() -> Result<()> {
    let mtm = MainThreadMarker::new()
        .ok_or_else(|| anyhow!("recording shell must be created on the macOS main thread"))?;
    let application = NSApplication::sharedApplication(mtm);
    let _ = application.setActivationPolicy(NSApplicationActivationPolicy::Accessory);
    application.finishLaunching();
    Ok(())
}

impl FloatingPanel {
    /// Create and show the panel without activating Scrybe.
    ///
    /// # Errors
    ///
    /// Returns an error if `AppKit` has no main-thread marker, screen, or content view.
    pub fn start(initial: ShellView) -> Result<Self> {
        let mtm = MainThreadMarker::new()
            .ok_or_else(|| anyhow!("floating window must be created on the macOS main thread"))?;

        let screen = NSScreen::mainScreen(mtm)
            .ok_or_else(|| anyhow!("no active macOS screen for floating window"))?;
        let frame = panel_frame(screen.visibleFrame());
        let panel = NSPanel::initWithContentRect_styleMask_backing_defer(
            NSPanel::alloc(mtm),
            frame,
            NSWindowStyleMask::Borderless | NSWindowStyleMask::NonactivatingPanel,
            NSBackingStoreType::Buffered,
            false,
        );

        // SAFETY: The retained panel is owned by this struct, so AppKit must not
        // release it implicitly if a future close action is introduced.
        unsafe { panel.setReleasedWhenClosed(false) };
        panel.setFloatingPanel(true);
        panel.setBecomesKeyOnlyIfNeeded(true);
        panel.setMovableByWindowBackground(true);
        panel.setHidesOnDeactivate(false);
        panel.setLevel(NSFloatingWindowLevel);
        panel.setOpaque(false);
        panel.setBackgroundColor(Some(&NSColor::clearColor()));
        panel.setHasShadow(true);
        panel.setCollectionBehavior(
            NSWindowCollectionBehavior::Transient
                | NSWindowCollectionBehavior::IgnoresCycle
                | NSWindowCollectionBehavior::FullScreenAuxiliary,
        );
        panel.setTitle(&NSString::from_str("Scrybe recording"));

        let content = NSBox::initWithFrame(
            NSBox::alloc(mtm),
            NSRect::new(
                NSPoint::new(0.0, 0.0),
                NSSize::new(PANEL_WIDTH, PANEL_HEIGHT),
            ),
        );
        content.setBoxType(NSBoxType::Custom);
        content.setTitlePosition(NSTitlePosition::NoTitle);
        content.setBorderWidth(0.0);
        content.setCornerRadius(PANEL_HEIGHT / 2.0);
        content.setFillColor(&NSColor::colorWithSRGBRed_green_blue_alpha(
            0.063, 0.082, 0.075, 0.96,
        ));
        content.setContentViewMargins(NSSize::new(0.0, 0.0));
        panel.setContentView(Some(&content));

        let recording_dot = NSTextField::labelWithString(&NSString::from_str("●"), mtm);
        recording_dot.setFrame(NSRect::new(
            NSPoint::new(13.0, 12.0),
            NSSize::new(12.0, 18.0),
        ));
        recording_dot.setTextColor(Some(&dot_color(initial.state)));

        let state_label = NSTextField::labelWithString(&state_string(initial.state), mtm);
        state_label.setFrame(NSRect::new(
            NSPoint::new(30.0, 13.0),
            NSSize::new(70.0, 18.0),
        ));
        state_label.setTextColor(Some(&NSColor::whiteColor()));
        state_label.setAlignment(NSTextAlignment::Left);

        let elapsed_label =
            NSTextField::labelWithString(&NSString::from_str(&initial.elapsed_label()), mtm);
        elapsed_label.setFrame(NSRect::new(
            NSPoint::new(104.0, 13.0),
            NSSize::new(64.0, 18.0),
        ));
        elapsed_label.setTextColor(Some(&NSColor::colorWithWhite_alpha(1.0, 0.7)));
        elapsed_label.setAlignment(NSTextAlignment::Left);

        let (stop_tx, stop_events) = bounded(1);
        let target = StopTarget::new(mtm, stop_tx);
        // SAFETY: `stopRecording:` is implemented by StopTarget with the
        // standard one-sender Objective-C action signature.
        let stop_button = unsafe {
            NSButton::buttonWithTitle_target_action(
                &NSString::from_str("■"),
                Some(&target),
                Some(sel!(stopRecording:)),
                mtm,
            )
        };
        stop_button.setFrame(NSRect::new(
            NSPoint::new(204.0, 9.0),
            NSSize::new(26.0, 26.0),
        ));
        stop_button.setBezelStyle(NSBezelStyle::Circular);
        stop_button.setBezelColor(Some(&NSColor::systemRedColor()));
        stop_button.setContentTintColor(Some(&NSColor::whiteColor()));
        stop_button.setAccessibilityLabel(Some(&NSString::from_str("Stop & save")));
        stop_button.setEnabled(initial.stop_enabled);

        content.addSubview(&recording_dot);
        content.addSubview(&state_label);
        content.addSubview(&elapsed_label);
        content.addSubview(&stop_button);
        panel.orderFrontRegardless();

        Ok(Self {
            panel,
            recording_dot,
            state_label,
            elapsed_label,
            stop_button,
            _target: target,
            stop_events,
            last_state: initial.state,
            last_elapsed_second: initial.elapsed.as_secs(),
        })
    }

    /// Render the shared lifecycle snapshot.
    pub fn render(&mut self, view: ShellView) {
        if view.state != self.last_state {
            self.recording_dot
                .setTextColor(Some(&dot_color(view.state)));
            self.state_label.setStringValue(&state_string(view.state));
            self.stop_button.setEnabled(view.stop_enabled);
            self.last_state = view.state;
        }
        let elapsed_second = view.elapsed.as_secs();
        if elapsed_second != self.last_elapsed_second {
            self.elapsed_label
                .setStringValue(&NSString::from_str(&view.elapsed_label()));
            self.last_elapsed_second = elapsed_second;
        }
    }

    /// Whether the Stop & save button was pressed.
    #[must_use]
    pub fn poll_stop(&self) -> bool {
        self.stop_events.try_recv().is_ok()
    }
}

impl Drop for FloatingPanel {
    fn drop(&mut self) {
        self.panel.orderOut(None);
    }
}

/// Read the current macOS Reduce Motion preference.
#[must_use]
pub fn reduce_motion_enabled() -> bool {
    NSWorkspace::sharedWorkspace().accessibilityDisplayShouldReduceMotion()
}

fn panel_frame(screen: NSRect) -> NSRect {
    let x = screen.origin.x + (screen.size.width - PANEL_WIDTH) / 2.0;
    let y = screen.origin.y + screen.size.height - PANEL_HEIGHT - TOP_INSET;
    NSRect::new(NSPoint::new(x, y), NSSize::new(PANEL_WIDTH, PANEL_HEIGHT))
}

fn state_string(state: ShellState) -> Retained<NSString> {
    NSString::from_str(match state {
        ShellState::Recording => "Recording",
        ShellState::Saving => "Saving…",
    })
}

fn dot_color(state: ShellState) -> Retained<NSColor> {
    match state {
        ShellState::Recording => NSColor::systemRedColor(),
        ShellState::Saving => NSColor::colorWithWhite_alpha(1.0, 0.35),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn panel_is_top_centered_within_the_active_screen() {
        let frame = panel_frame(NSRect::new(
            NSPoint::new(100.0, 50.0),
            NSSize::new(1_200.0, 800.0),
        ));
        assert!((frame.origin.x - 580.0).abs() < f64::EPSILON);
        assert!((frame.origin.y - 782.0).abs() < f64::EPSILON);
        assert_eq!(frame.size, NSSize::new(PANEL_WIDTH, PANEL_HEIGHT));
    }
}
