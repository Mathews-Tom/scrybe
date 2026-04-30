// Copyright 2026 Mathews Tom
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//     https://www.apache.org/licenses/LICENSE-2.0

//! Shared runtime helpers: storage-root expansion, config loading.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use scrybe_core::config::Config;

/// Expand a `~/...`-prefixed path against the user's home directory.
pub fn expand_root(root: &Path) -> PathBuf {
    let path_str = root.to_string_lossy();
    if let Some(rest) = path_str.strip_prefix("~/") {
        if let Some(home) = dirs_home() {
            return home.join(rest);
        }
    } else if path_str == "~" {
        if let Some(home) = dirs_home() {
            return home;
        }
    }
    root.to_path_buf()
}

fn dirs_home() -> Option<PathBuf> {
    directories::BaseDirs::new().map(|b| b.home_dir().to_path_buf())
}

/// Load config from the platform-conventional path or
/// `SCRYBE_CONFIG`, returning the default if no file exists.
pub fn load_or_default_config() -> Result<Config> {
    let path = Config::discover_path().context("resolving config path")?;
    if path.exists() {
        Config::load(&path).with_context(|| format!("loading config at {}", path.display()))
    } else {
        Ok(Config::default())
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    #[test]
    fn test_expand_root_returns_input_path_for_absolute_path() {
        let p = PathBuf::from("/var/scrybe");

        let expanded = expand_root(&p);

        assert_eq!(expanded, p);
    }

    #[test]
    fn test_expand_root_returns_input_path_for_relative_path() {
        let p = PathBuf::from("relative/dir");

        let expanded = expand_root(&p);

        assert_eq!(expanded, p);
    }

    #[test]
    fn test_expand_root_substitutes_tilde_prefix_with_home() {
        let p = PathBuf::from("~/scrybe");

        let expanded = expand_root(&p);

        if let Some(home) = dirs_home() {
            assert_eq!(expanded, home.join("scrybe"));
        }
    }

    #[test]
    fn test_expand_root_returns_home_for_bare_tilde() {
        let p = PathBuf::from("~");

        let expanded = expand_root(&p);

        if let Some(home) = dirs_home() {
            assert_eq!(expanded, home);
        }
    }

    #[test]
    fn test_load_or_default_config_returns_default_when_path_does_not_exist() {
        let dir = tempfile::tempdir().unwrap();
        std::env::set_var("SCRYBE_CONFIG", dir.path().join("no-such-config.toml"));

        let cfg = load_or_default_config().unwrap();

        assert_eq!(cfg, scrybe_core::config::Config::default());
    }
}
