// Copyright 2026 Mathews Tom
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//     https://www.apache.org/licenses/LICENSE-2.0

//! Shared runtime helpers: storage-root expansion, config loading,
//! session-folder resolution.

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

/// Resolves `id_or_folder` to a session folder under `root`.
///
/// Accepts, in order: an absolute path to an existing directory, a
/// folder name relative to `root`, or a substring match against
/// folder names under `root` (typically a ULID or ULID prefix) —
/// resolved only when exactly one folder matches.
///
/// # Errors
///
/// Returns an error if `root` does not exist, if no folder matches
/// `id_or_folder`, or if more than one folder matches an ambiguous
/// substring.
pub fn resolve_session_folder(root: &Path, id_or_folder: &str) -> Result<PathBuf> {
    let direct = Path::new(id_or_folder);
    if direct.is_absolute() && direct.is_dir() {
        return Ok(direct.to_path_buf());
    }
    let direct_in_root = root.join(id_or_folder);
    if direct_in_root.is_dir() {
        return Ok(direct_in_root);
    }
    if !root.exists() {
        anyhow::bail!("storage root {} does not exist", root.display());
    }
    let entries = std::fs::read_dir(root).with_context(|| format!("reading {}", root.display()))?;
    let mut hits: Vec<PathBuf> = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let name = path
            .file_name()
            .map_or_else(String::new, |s| s.to_string_lossy().into_owned());
        if name.contains(id_or_folder) {
            hits.push(path);
        }
    }
    match hits.len() {
        0 => anyhow::bail!("no session matches {id_or_folder}"),
        1 => Ok(hits.remove(0)),
        n => {
            anyhow::bail!("session prefix {id_or_folder} matches {n} folders; please disambiguate")
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod resolve_session_folder_tests {
    use super::*;

    #[test]
    fn test_resolve_session_folder_returns_path_when_folder_exists_in_root() {
        let dir = tempfile::tempdir().unwrap();
        let folder = dir.path().join("2026-04-29-1430-test-01HXYZ");
        std::fs::create_dir(&folder).unwrap();

        let resolved = resolve_session_folder(dir.path(), "2026-04-29-1430-test-01HXYZ").unwrap();

        assert_eq!(resolved, folder);
    }

    #[test]
    fn test_resolve_session_folder_resolves_partial_prefix_unambiguously() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("2026-04-29-1430-acme-01HXYZ")).unwrap();
        std::fs::create_dir(dir.path().join("2026-04-30-0900-other-02HABCD")).unwrap();

        let resolved = resolve_session_folder(dir.path(), "acme").unwrap();

        assert!(resolved.to_string_lossy().contains("acme"));
    }

    #[test]
    fn test_resolve_session_folder_returns_error_for_no_matching_session() {
        let dir = tempfile::tempdir().unwrap();

        let err = resolve_session_folder(dir.path(), "missing").unwrap_err();

        assert!(err.to_string().contains("no session matches"));
    }

    #[test]
    fn test_resolve_session_folder_returns_error_when_prefix_is_ambiguous() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("2026-04-29-1430-acme-01HXYZ")).unwrap();
        std::fs::create_dir(dir.path().join("2026-04-29-1500-acme-02HABCD")).unwrap();

        let err = resolve_session_folder(dir.path(), "acme").unwrap_err();

        assert!(err.to_string().contains("matches 2 folders"));
    }

    #[test]
    fn test_resolve_session_folder_returns_path_when_absolute_dir_supplied() {
        let dir = tempfile::tempdir().unwrap();
        let folder = dir.path().join("session-abs");
        std::fs::create_dir(&folder).unwrap();
        let unrelated = tempfile::tempdir().unwrap();

        let resolved = resolve_session_folder(unrelated.path(), folder.to_str().unwrap()).unwrap();

        assert_eq!(resolved, folder);
    }

    #[test]
    fn test_resolve_session_folder_returns_error_when_root_is_missing() {
        let dir = tempfile::tempdir().unwrap();
        let bogus = dir.path().join("nonexistent");

        let err = resolve_session_folder(&bogus, "anything").unwrap_err();

        assert!(err.to_string().contains("storage root"));
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
