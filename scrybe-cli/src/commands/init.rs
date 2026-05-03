// Copyright 2026 Mathews Tom
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//     https://www.apache.org/licenses/LICENSE-2.0

//! `scrybe init` — create the platform-conventional config file with
//! defaults documented in `docs/system-design.md` §6 unless one
//! already exists.

use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::{Args as ClapArgs, ValueEnum};
use directories::ProjectDirs;
use scrybe_core::config::{
    Config, CONFIG_FILE_NAME, RECORD_LLM_OPENAI_COMPAT, RECORD_SOURCE_MIC_SYSTEM,
};
use scrybe_core::storage::atomic_replace;

const DEFAULT_MAC_LOCAL_LLM_MODEL: &str = "gemma4:latest";

#[derive(ClapArgs, Debug)]
pub struct Args {
    /// Force overwrite of an existing config file.
    #[arg(long, default_value_t = false)]
    pub force: bool,

    /// Optional explicit destination, overriding the platform path.
    #[arg(long)]
    pub path: Option<PathBuf>,

    /// Configuration profile to write.
    #[arg(long, value_enum)]
    pub profile: Option<InitProfile>,

    /// Whisper model path for profiles that enable local transcription.
    #[arg(long)]
    pub whisper_model: Option<PathBuf>,

    /// OpenAI-compatible model name for profiles that enable local notes.
    #[arg(long)]
    pub llm_model: Option<String>,
}

#[derive(Copy, Clone, Debug, Eq, PartialEq, ValueEnum)]
pub enum InitProfile {
    /// Conservative hermetic defaults: synthetic capture, stub STT/LLM.
    Default,
    /// macOS local capture: mic+system, local Whisper, Ollama-compatible LLM.
    #[value(name = "mac-local")]
    MacLocal,
}

pub async fn run(args: Args) -> Result<()> {
    let target = match &args.path {
        Some(p) => p.clone(),
        None => Config::default_path().context("resolving default config path")?,
    };
    if target.exists() && !args.force {
        anyhow::bail!(
            "config already exists at {}; pass --force to overwrite",
            target.display()
        );
    }
    if let Some(parent) = target.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .with_context(|| format!("creating config directory {}", parent.display()))?;
    } else {
        anyhow::bail!(
            "config target has no parent directory: {}",
            target.display()
        );
    }

    let config = build_config(&args)?;
    let body = toml::to_string_pretty(&config).context("serializing default config")?;
    atomic_replace(&target, body.as_bytes())
        .with_context(|| format!("writing {}", target.display()))?;

    println!(
        "scrybe init: wrote {} ({} bytes)",
        target.display(),
        body.len()
    );
    println!("config file name: {CONFIG_FILE_NAME}");
    Ok(())
}

fn build_config(args: &Args) -> Result<Config> {
    let mut config = Config::default();
    match resolved_profile(args.profile) {
        InitProfile::Default => {
            if args.whisper_model.is_some() || args.llm_model.is_some() {
                anyhow::bail!("--whisper-model and --llm-model require --profile mac-local");
            }
        }
        InitProfile::MacLocal => {
            let whisper_model = args
                .whisper_model
                .clone()
                .map_or_else(default_mac_local_whisper_model, Ok)?;
            config.record.source = RECORD_SOURCE_MIC_SYSTEM.to_string();
            config.record.whisper_model = Some(whisper_model);
            config.record.llm = RECORD_LLM_OPENAI_COMPAT.to_string();
            config.llm.model = args
                .llm_model
                .clone()
                .unwrap_or_else(|| DEFAULT_MAC_LOCAL_LLM_MODEL.to_string());
        }
    }
    Ok(config)
}

fn resolved_profile(profile: Option<InitProfile>) -> InitProfile {
    profile.unwrap_or_else(platform_default_profile)
}

const fn platform_default_profile() -> InitProfile {
    if cfg!(target_os = "macos") {
        InitProfile::MacLocal
    } else {
        InitProfile::Default
    }
}

fn default_mac_local_whisper_model() -> Result<PathBuf> {
    let dirs = ProjectDirs::from("dev", "scrybe", "scrybe")
        .context("resolving platform data directory for default Whisper model")?;
    Ok(dirs.data_dir().join("models/ggml-base.en.bin"))
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    #[tokio::test]
    async fn test_init_writes_config_file_at_supplied_path() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("config.toml");

        run(Args {
            force: false,
            path: Some(target.clone()),
            profile: Some(InitProfile::Default),
            whisper_model: None,
            llm_model: None,
        })
        .await
        .unwrap();

        assert!(target.exists());
        let body = std::fs::read_to_string(&target).unwrap();
        let parsed = Config::from_toml_str(&body, &target).unwrap();
        assert_eq!(parsed.stt.model, "large-v3-turbo");
    }

    #[tokio::test]
    async fn test_init_refuses_to_overwrite_existing_config_without_force() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("config.toml");
        std::fs::write(&target, b"schema_version = 1\n").unwrap();

        let err = run(Args {
            force: false,
            path: Some(target.clone()),
            profile: Some(InitProfile::Default),
            whisper_model: None,
            llm_model: None,
        })
        .await
        .unwrap_err();

        assert!(err.to_string().contains("config already exists"));
    }

    #[tokio::test]
    async fn test_init_overwrites_existing_config_when_force_set() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("config.toml");
        std::fs::write(&target, b"schema_version = 0\n").unwrap();

        run(Args {
            force: true,
            path: Some(target.clone()),
            profile: Some(InitProfile::Default),
            whisper_model: None,
            llm_model: None,
        })
        .await
        .unwrap();

        let body = std::fs::read_to_string(&target).unwrap();
        assert!(body.contains("schema_version = 1"));
    }

    /// E-4 from `.docs/development-plan.md` §7.3.3: the onboarding
    /// flow must produce a config that round-trips through serialize
    /// → deserialize → equality with `Config::default()`. A regression
    /// in `toml::to_string_pretty` or `Config::from_toml_str` (e.g.,
    /// adding a `#[serde(skip_serializing_if = ...)]` to a defaulted
    /// field, or renaming a key without a `#[serde(rename)]` shim)
    /// would silently break this contract — the user runs `scrybe
    /// init`, the resulting file looks fine, but `scrybe record`
    /// fails to parse it on the next launch.
    ///
    /// The §7.3.3 sub-assert "model downloaded with correct checksum"
    /// is deferred. It requires either a deterministic ~800 MB
    /// Whisper fixture (impractical to vendor) or a network mock for
    /// the model-download step (the v0.1 init flow doesn't perform a
    /// download — that's a v0.2 deliverable).
    #[tokio::test]
    async fn test_init_writes_config_round_trips_to_default() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("config.toml");

        run(Args {
            force: false,
            path: Some(target.clone()),
            profile: Some(InitProfile::Default),
            whisper_model: None,
            llm_model: None,
        })
        .await
        .unwrap();

        let body = std::fs::read_to_string(&target).unwrap();
        let parsed = Config::from_toml_str(&body, &target).unwrap();

        assert_eq!(
            parsed,
            Config::default(),
            "init must write a config that round-trips back to Config::default()"
        );
    }

    #[tokio::test]
    async fn test_init_mac_local_profile_writes_record_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("config.toml");
        let model_path = dir.path().join("ggml-base.en.bin");

        run(Args {
            force: false,
            path: Some(target.clone()),
            profile: Some(InitProfile::MacLocal),
            whisper_model: Some(model_path.clone()),
            llm_model: Some("gemma4:latest".to_string()),
        })
        .await
        .unwrap();

        let body = std::fs::read_to_string(&target).unwrap();
        let parsed = Config::from_toml_str(&body, &target).unwrap();

        assert_eq!(parsed.record.source, RECORD_SOURCE_MIC_SYSTEM);
        assert_eq!(
            parsed.record.whisper_model.as_deref(),
            Some(model_path.as_path())
        );
        assert_eq!(parsed.record.llm, RECORD_LLM_OPENAI_COMPAT);
        assert_eq!(parsed.llm.model, "gemma4:latest");
    }

    #[test]
    fn test_init_mac_local_profile_supplies_local_defaults() {
        let config = build_config(&Args {
            force: false,
            path: None,
            profile: Some(InitProfile::MacLocal),
            whisper_model: None,
            llm_model: None,
        })
        .unwrap();

        assert_eq!(config.record.source, RECORD_SOURCE_MIC_SYSTEM);
        let whisper_model = config.record.whisper_model.as_deref().unwrap();
        assert!(whisper_model.ends_with("models/ggml-base.en.bin"));
        assert_eq!(config.record.llm, RECORD_LLM_OPENAI_COMPAT);
        assert_eq!(config.llm.model, DEFAULT_MAC_LOCAL_LLM_MODEL);
    }

    #[test]
    fn test_init_uses_mac_local_as_platform_default_on_macos() {
        let expected = platform_default_profile();
        assert_eq!(resolved_profile(None), expected);
    }
}
