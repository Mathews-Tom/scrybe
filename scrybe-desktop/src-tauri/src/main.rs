// Copyright 2026 Mathews Tom
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//     https://www.apache.org/licenses/LICENSE-2.0

//! Desktop entry point.

// A release build must not open a console window behind the
// application. The attribute is Windows-only in effect and inert
// elsewhere; macOS is the only platform this application targets.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() -> std::process::ExitCode {
    match scrybe_desktop::run() {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("scrybe-desktop: {error}");
            std::process::ExitCode::FAILURE
        }
    }
}
