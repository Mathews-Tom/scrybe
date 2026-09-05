// Copyright 2026 Mathews Tom
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//     https://www.apache.org/licenses/LICENSE-2.0

//! `scrybe devices` — list stable input-device selectors.
//!
//! On macOS, each UID comes directly from Core Audio and is the only value
//! accepted by M4's `--input-device` option. Display names are descriptive:
//! duplicate names remain separate rows and cannot be used as selectors.

#[cfg(all(target_os = "macos", feature = "system-capture-mac"))]
use anyhow::Context;
use anyhow::Result;
use clap::Args as ClapArgs;

#[derive(ClapArgs, Debug)]
pub struct Args {}

#[cfg(any(test, all(target_os = "macos", feature = "system-capture-mac")))]
#[derive(Clone, Debug, Eq, PartialEq)]
struct DeviceRow {
    uid: String,
    name: String,
    is_default: bool,
}

#[cfg(any(test, all(target_os = "macos", feature = "system-capture-mac")))]
/// Render a stable, tab-separated device catalog.
fn render_devices(mut devices: Vec<DeviceRow>) -> String {
    devices.sort_by(|left, right| left.uid.cmp(&right.uid));
    let mut rendered = String::from("uid\tdefault\tname\n");
    for device in devices {
        let default = if device.is_default { "yes" } else { "" };
        rendered.push_str(&device.uid);
        rendered.push('\t');
        rendered.push_str(default);
        rendered.push('\t');
        rendered.push_str(&device.name);
        rendered.push('\n');
    }
    rendered
}

/// Print macOS input devices and their native selectors.
///
/// # Errors
///
/// Fails loudly outside the macOS Core Audio feature graph. Linux and Windows
/// must wait for M8 to define native identifiers rather than accepting an
/// ambiguous display name.
pub fn run(_args: Args) -> Result<()> {
    #[cfg(all(target_os = "macos", feature = "system-capture-mac"))]
    {
        let devices = scrybe_capture_mac::input_devices()
            .map_err(anyhow::Error::from)
            .context("enumerating macOS Core Audio input devices")?
            .into_iter()
            .map(|device| DeviceRow {
                uid: device.uid,
                name: device.name,
                is_default: device.is_default,
            })
            .collect();
        print!("{}", render_devices(devices));
        Ok(())
    }

    #[cfg(not(all(target_os = "macos", feature = "system-capture-mac")))]
    {
        anyhow::bail!(
            "scrybe devices is supported only by a macOS build with the `system-capture-mac` \
             feature; Linux and Windows device selectors are deferred to M8"
        );
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    #[test]
    fn render_devices_keeps_duplicate_names_distinct_by_uid() {
        let rendered = render_devices(vec![
            DeviceRow {
                uid: "usb-input".to_string(),
                name: "MacBook Pro Microphone".to_string(),
                is_default: false,
            },
            DeviceRow {
                uid: "built-in-input".to_string(),
                name: "MacBook Pro Microphone".to_string(),
                is_default: true,
            },
        ]);

        assert_eq!(
            rendered,
            "uid\tdefault\tname\n\
             built-in-input\tyes\tMacBook Pro Microphone\n\
             usb-input\t\tMacBook Pro Microphone\n"
        );
    }

    #[test]
    #[cfg(not(all(target_os = "macos", feature = "system-capture-mac")))]
    fn devices_fail_loudly_without_native_identifier_support() {
        let error = run(Args {}).unwrap_err();
        assert!(error.to_string().contains("deferred to M8"));
    }
}
