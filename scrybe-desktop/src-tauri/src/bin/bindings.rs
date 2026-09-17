// Copyright 2026 Mathews Tom
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//     https://www.apache.org/licenses/LICENSE-2.0

//! Writes the TypeScript transport contract the frontend imports.
//!
//! The drift check is a test, not this binary: a developer runs this to
//! regenerate, and continuous integration runs the test to prove the
//! checked-in file is what this binary would have written.

fn main() -> std::process::ExitCode {
    let path = scrybe_desktop::contract::bindings_path();
    match std::fs::write(&path, scrybe_desktop::contract::render()) {
        Ok(()) => {
            println!("wrote {}", path.display());
            std::process::ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("{}: {error}", path.display());
            std::process::ExitCode::FAILURE
        }
    }
}
