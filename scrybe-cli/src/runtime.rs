// Copyright 2026 Mathews Tom
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//     https://www.apache.org/licenses/LICENSE-2.0

//! Shared runtime helpers: storage-root expansion, config loading, and
//! application-service construction.
//!
//! Session resolution used to live here and accepted any existing
//! absolute directory before consulting the configured root. It is gone:
//! every command now addresses a session by opaque identity and the
//! repository resolves it beneath the configured root.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use scrybe_application::{ConfigService, ScrybeApplication, StorageRoot};
use scrybe_core::config::Config;

/// Expand a `~/...`-prefixed path against the user's home directory.
pub fn expand_root(root: &Path) -> PathBuf {
    scrybe_application::recording::expand_tilde(root, dirs_home().as_deref())
}

fn dirs_home() -> Option<PathBuf> {
    directories::BaseDirs::new().map(|b| b.home_dir().to_path_buf())
}

/// The configuration service over the platform-conventional path or
/// `SCRYBE_CONFIG`.
///
/// `application` needs the configured storage root before a
/// [`ScrybeApplication`] can be assembled over it, so the service is
/// constructed here too. The exists-or-default policy itself lives in
/// [`ConfigService::load`] and nowhere else; the CLI used to carry a
/// second copy of it, which is one decision with two implementations
/// that could drift apart.
///
/// # Errors
///
/// Propagates configuration path resolution failures.
pub fn config_service() -> Result<ConfigService> {
    Ok(ConfigService::new(
        Config::discover_path().context("resolving config path")?,
    ))
}

/// Every application service, over the root this invocation operates
/// on: the `--root` override when given, otherwise the configured
/// root, tilde-expanded.
///
/// Every command that touches a session, the configuration, the
/// diagnosis, or a recording goes through this, so there is one place
/// the CLI decides which install it is looking at.
///
/// # Errors
///
/// Propagates configuration path resolution and loading failures.
pub fn application(root_override: Option<&Path>) -> Result<ScrybeApplication> {
    let config = config_service()?;
    let path = match root_override {
        Some(path) => expand_root(path),
        None => expand_root(&config.load()?.storage.root),
    };
    Ok(ScrybeApplication::new(
        StorageRoot::new(path),
        config.path(),
    ))
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;
    use std::sync::Arc;

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
    fn test_the_application_is_the_only_source_of_a_recording_controller() {
        let dir = tempfile::tempdir().unwrap();
        // An explicit root means no configuration is read, so this
        // test does not touch `SCRYBE_CONFIG` and cannot race the
        // config-service test beside it.
        let app = application(Some(dir.path())).unwrap();

        // `RecordingController::new` and `with_clock` are crate-private
        // to `scrybe-application`, so this accessor is the only way the
        // CLI can obtain one — which is what makes "one process-wide
        // recording state model" structural rather than a convention.
        // Both CLI recording paths, `rec::run` and
        // `shell::run_record_with_shell`, take theirs from here.
        let from_cli = Arc::clone(app.recording());

        assert!(Arc::ptr_eq(&from_cli, app.recording()));
        from_cli.begin_preparing().unwrap();
        assert_eq!(
            app.recording().snapshot().state,
            scrybe_application::recording::RecordingState::Preparing,
            "a second instance would leave the application reading Idle forever"
        );
    }

    #[test]
    fn test_the_config_service_returns_the_default_when_no_file_exists() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("no-such-config.toml");
        std::env::set_var("SCRYBE_CONFIG", &path);

        let service = config_service().unwrap();

        assert_eq!(service.path(), path);
        assert_eq!(
            service.load().unwrap(),
            scrybe_core::config::Config::default()
        );
    }
}
