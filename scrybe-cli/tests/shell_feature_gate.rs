// Copyright 2026 Mathews Tom
// Licensed under the Apache License, Version 2.0 (the "License");

#![cfg(not(feature = "cli-shell"))]
#![allow(clippy::expect_used)]

use std::process::Command;

#[test]
fn explicit_shell_requests_fail_instead_of_recording_headless() {
    for args in [
        vec!["rec", "--shell"],
        vec!["record", "qualification", "--shell"],
    ] {
        let output = Command::new(env!("CARGO_BIN_EXE_scrybe"))
            .args(&args)
            .output()
            .expect("run scrybe");
        assert!(!output.status.success(), "args: {args:?}");
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(stderr.contains("requires the `cli-shell` build feature"));
        assert!(!stderr.contains("running headless"));
    }
}
