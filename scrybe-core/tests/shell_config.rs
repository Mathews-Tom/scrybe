// Copyright 2026 Mathews Tom
// Licensed under the Apache License, Version 2.0 (the "License");

#![allow(clippy::unwrap_used)]

use std::path::Path;

use scrybe_core::config::{Config, ShellIndicator};

const ALL: [ShellIndicator; 3] = [
    ShellIndicator::MenuBarWaveform,
    ShellIndicator::MenuBarLabel,
    ShellIndicator::FloatingWindow,
];

fn parse(text: &str) -> Config {
    Config::from_toml_str(text, Path::new("config.toml")).unwrap()
}

#[test]
fn omitted_shell_block_enables_the_hybrid_default() {
    assert_eq!(parse("schema_version = 1").shell.indicators(), ALL);
}

#[test]
fn every_nonempty_indicator_subset_resolves_in_canonical_order() {
    let cases: [(&str, &[ShellIndicator]); 7] = [
        ("\"menu-bar-waveform\"", &ALL[..1]),
        ("\"menu-bar-label\"", &ALL[1..2]),
        ("\"floating-window\"", &ALL[2..]),
        ("\"menu-bar-label\", \"menu-bar-waveform\"", &ALL[..2]),
        (
            "\"floating-window\", \"menu-bar-waveform\"",
            &[ALL[0], ALL[2]],
        ),
        ("\"floating-window\", \"menu-bar-label\"", &ALL[1..]),
        (
            "\"floating-window\", \"menu-bar-label\", \"menu-bar-waveform\"",
            &ALL,
        ),
    ];

    for (input, expected) in cases {
        let config = parse(&format!(
            "schema_version = 1\n[shell]\nindicators = [{input}]"
        ));
        assert_eq!(config.shell.indicators(), expected, "input: {input}");
    }
}

#[test]
fn invalid_indicator_lists_fail_during_config_load() {
    for (input, message) in [
        ("", "must contain at least one indicator"),
        (
            "\"menu-bar-label\", \"menu-bar-label\"",
            "duplicate shell indicator `menu-bar-label`",
        ),
        ("\"elapsed-timer\"", "unknown variant `elapsed-timer`"),
    ] {
        let error = Config::from_toml_str(
            &format!("schema_version = 1\n[shell]\nindicators = [{input}]"),
            Path::new("config.toml"),
        )
        .unwrap_err();
        assert!(error.to_string().contains(message), "{error}");
    }
}

#[test]
fn serialized_default_makes_the_shell_contract_explicit() {
    let encoded = toml::to_string(&Config::default()).unwrap();
    assert!(encoded.contains("[shell]"));
    assert!(encoded
        .contains("indicators = [\"menu-bar-waveform\", \"menu-bar-label\", \"floating-window\"]"));
}
