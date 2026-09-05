// Copyright 2026 Mathews Tom
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//     https://www.apache.org/licenses/LICENSE-2.0

//! `scrybe record <title>` — ergonomic entry point.
//!
//! Resolves capture source, Whisper model, and LLM kind from config
//! defaults plus platform probes (see `scrybe_core::record_defaults`).
//! On macOS with `mic+system` source, auto-launches via the `.app`
//! bundle so `TCC`'s `AudioCapture` grant binds to the bundle's
//! responsible process — see `.docs/handoff.md` §1 and §7 for why.
//!
//! Power users and CI scripts that need explicit flag control should
//! use `scrybe rec` instead; this command is the happy path for
//! end-user recording with sensible defaults.

#[cfg(feature = "system-capture-mac")]
use std::path::Path;
use std::path::PathBuf;

#[cfg(feature = "system-capture-mac")]
use anyhow::Context;
use anyhow::Result;
use clap::Args as ClapArgs;
use scrybe_core::config::{
    Config, RECORD_LLM_OPENAI_COMPAT, RECORD_SOURCE_MIC, RECORD_SOURCE_MIC_SYSTEM,
    RECORD_SYSTEM_BACKEND_SCK, RECORD_SYSTEM_BACKEND_TAP,
};
#[cfg(feature = "system-capture-mac")]
use scrybe_core::config::{RECORD_LLM_STUB, RECORD_SOURCE_SYNTHETIC};
use scrybe_core::record_defaults;

use crate::commands::rec::{self, CaptureSourceArg, LlmBackendArg};
use crate::runtime::load_or_default_config;

#[cfg(feature = "system-capture-mac")]
const SCRYBE_BUNDLE_ENV: &str = "SCRYBE_BUNDLE";
#[cfg(feature = "system-capture-mac")]
const BUNDLE_FILE_NAME: &str = "scrybe.app";

#[derive(ClapArgs, Clone, Debug)]
pub struct Args {
    /// Session title (positional). Becomes the folder-name component
    /// and the transcript heading.
    pub title: String,

    /// Override the capture source. Defaults to `mic+system` on macOS
    /// and `mic` elsewhere; honors explicit `[record].source` from
    /// config when set to a non-synthetic value.
    #[arg(long, value_enum)]
    pub source: Option<CaptureSourceArg>,

    /// Exact macOS Core Audio input-device UID from `scrybe devices`.
    #[arg(long)]
    pub input_device: Option<String>,

    /// Override the `sck` or `tap` system-audio adapter.
    #[arg(long, value_enum)]
    pub system_backend: Option<super::rec::SystemBackendArg>,

    /// Override the Whisper model path. Defaults to the model under
    /// `~/Library/Application Support/dev.scrybe.scrybe/models/` if
    /// the file exists; otherwise the session errors at start time
    /// with a setup hint.
    #[arg(long)]
    pub whisper_model: Option<PathBuf>,

    /// Override the LLM backend. Defaults to `openai-compat` if a
    /// local Ollama is reachable at `127.0.0.1:11434`, else `stub`.
    #[arg(long, value_enum)]
    pub llm: Option<LlmBackendArg>,

    /// Storage root override.
    #[arg(long)]
    pub root: Option<PathBuf>,

    /// Force in-process invocation; do not auto-launch via the .app
    /// bundle even on macOS. Used to bypass TCC binding for unit
    /// testing or non-system-tap recordings.
    #[arg(long, hide = true)]
    pub no_bundle: bool,
}

/// Dispatch the ergonomic record subcommand.
///
/// # Errors
///
/// Surfaces config-load errors, bundle-launch errors, and the
/// underlying `rec::run` errors verbatim.
pub async fn run(args: Args) -> Result<()> {
    let cfg = load_or_default_config()?;
    let resolved = resolve(&cfg, &args);

    if should_use_bundle(&resolved, &args) {
        #[cfg(feature = "system-capture-mac")]
        {
            let bundle = resolve_bundle_path()
                .context("no scrybe.app bundle found; install one or pass `--no-bundle`")?;
            let session_root = effective_session_root(&cfg, args.root.as_deref());
            let rec_argv = build_rec_argv(&resolved);
            return crate::bundle_launcher::launch_via_bundle(&bundle, &rec_argv, &session_root)
                .await;
        }
        #[cfg(not(feature = "system-capture-mac"))]
        {
            anyhow::bail!(
                "bundle auto-launch requires the `system-capture-mac` feature (needed for \
                 the mic+system TCC grant); rebuild with `--features system-capture-mac` or \
                 pass `--no-bundle`"
            );
        }
    }
    rec::run(resolved.into_rec_args()).await
}

#[derive(Clone, Debug)]
struct Resolved {
    title: String,
    source: CaptureSourceArg,
    system_backend: super::rec::SystemBackendArg,
    whisper_model: Option<PathBuf>,
    llm: LlmBackendArg,
    root: Option<PathBuf>,
    input_device: Option<String>,
}

impl Resolved {
    fn into_rec_args(self) -> rec::Args {
        rec::Args {
            title: Some(self.title),
            root: self.root,
            yes: true,
            consent: None,
            synthetic_secs: 5,
            source: Some(self.source),
            system_backend: Some(self.system_backend),
            whisper_model: self.whisper_model,
            llm: Some(self.llm),
            input_device: self.input_device,
            shell: false,
        }
    }
}

fn resolve(cfg: &Config, args: &Args) -> Resolved {
    let source = args.source.unwrap_or_else(|| {
        capture_source_arg_from_str(record_defaults::ergonomic_source(&cfg.record))
    });
    let system_backend = args.system_backend.unwrap_or_else(|| {
        system_backend_arg_from_str(
            cfg.record
                .validated_system_backend()
                .unwrap_or(RECORD_SYSTEM_BACKEND_SCK),
        )
    });
    let whisper_model = args
        .whisper_model
        .clone()
        .or_else(|| record_defaults::ergonomic_whisper_model(&cfg.record));
    let llm = args
        .llm
        .unwrap_or_else(|| llm_backend_arg_from_str(record_defaults::ergonomic_llm(&cfg.record)));
    Resolved {
        title: args.title.clone(),
        source,
        system_backend,
        whisper_model,
        llm,
        input_device: args.input_device.clone(),
        root: args.root.clone(),
    }
}

fn should_use_bundle(resolved: &Resolved, args: &Args) -> bool {
    if args.no_bundle || !cfg!(target_os = "macos") {
        return false;
    }
    matches!(resolved.source, CaptureSourceArg::MicSystem)
        && matches!(resolved.system_backend, super::rec::SystemBackendArg::Tap)
        && !already_inside_bundle()
}

fn already_inside_bundle() -> bool {
    std::env::current_exe()
        .ok()
        .and_then(|p| p.into_os_string().into_string().ok())
        .is_some_and(|s| s.contains(".app/Contents/MacOS/"))
}

#[cfg(feature = "system-capture-mac")]
fn resolve_bundle_path() -> Option<PathBuf> {
    if let Ok(path) = std::env::var(SCRYBE_BUNDLE_ENV) {
        let p = PathBuf::from(path);
        if p.is_dir() {
            return Some(p);
        }
    }
    if let Ok(cwd) = std::env::current_dir() {
        let dev = cwd.join(BUNDLE_FILE_NAME);
        if dev.is_dir() {
            return Some(dev);
        }
    }
    let sys = PathBuf::from("/Applications").join(BUNDLE_FILE_NAME);
    if sys.is_dir() {
        return Some(sys);
    }
    if let Some(home) = directories::UserDirs::new() {
        let user = home.home_dir().join("Applications").join(BUNDLE_FILE_NAME);
        if user.is_dir() {
            return Some(user);
        }
    }
    None
}

#[cfg(feature = "system-capture-mac")]
fn effective_session_root(cfg: &Config, override_root: Option<&Path>) -> PathBuf {
    if let Some(root) = override_root {
        return crate::runtime::expand_root(root);
    }
    crate::runtime::expand_root(Path::new(&cfg.storage.root))
}

#[cfg(feature = "system-capture-mac")]
fn build_rec_argv(resolved: &Resolved) -> Vec<String> {
    let mut argv = vec!["--title".to_string(), resolved.title.clone()];
    argv.push("--source".to_string());
    argv.push(capture_source_arg_to_str(resolved.source).to_string());
    argv.push("--llm".to_string());
    argv.push(llm_backend_arg_to_str(resolved.llm).to_string());
    if let Some(model) = &resolved.whisper_model {
        argv.push("--whisper-model".to_string());
        argv.push(model.to_string_lossy().into_owned());
    }
    argv.push("--system-backend".to_string());
    argv.push(system_backend_arg_to_str(resolved.system_backend).to_string());
    if let Some(input_device) = &resolved.input_device {
        argv.push("--input-device".to_string());
        argv.push(input_device.clone());
    }
    if let Some(root) = &resolved.root {
        argv.push("--root".to_string());
        argv.push(root.to_string_lossy().into_owned());
    }
    argv.push("--yes".to_string());
    argv
}

fn capture_source_arg_from_str(s: &str) -> CaptureSourceArg {
    match s {
        RECORD_SOURCE_MIC => CaptureSourceArg::Mic,
        RECORD_SOURCE_MIC_SYSTEM => CaptureSourceArg::MicSystem,
        _ => CaptureSourceArg::Synthetic,
    }
}

fn system_backend_arg_from_str(s: &str) -> super::rec::SystemBackendArg {
    match s {
        RECORD_SYSTEM_BACKEND_TAP => super::rec::SystemBackendArg::Tap,
        _ => super::rec::SystemBackendArg::Sck,
    }
}

#[cfg(feature = "system-capture-mac")]
const fn system_backend_arg_to_str(arg: super::rec::SystemBackendArg) -> &'static str {
    match arg {
        super::rec::SystemBackendArg::Sck => RECORD_SYSTEM_BACKEND_SCK,
        super::rec::SystemBackendArg::Tap => RECORD_SYSTEM_BACKEND_TAP,
    }
}

#[cfg(feature = "system-capture-mac")]
const fn capture_source_arg_to_str(arg: CaptureSourceArg) -> &'static str {
    match arg {
        CaptureSourceArg::Synthetic => RECORD_SOURCE_SYNTHETIC,
        CaptureSourceArg::Mic => RECORD_SOURCE_MIC,
        CaptureSourceArg::MicSystem => RECORD_SOURCE_MIC_SYSTEM,
    }
}

fn llm_backend_arg_from_str(s: &str) -> LlmBackendArg {
    if s == RECORD_LLM_OPENAI_COMPAT {
        LlmBackendArg::OpenAiCompat
    } else {
        LlmBackendArg::Stub
    }
}

#[cfg(feature = "system-capture-mac")]
const fn llm_backend_arg_to_str(arg: LlmBackendArg) -> &'static str {
    match arg {
        LlmBackendArg::Stub => RECORD_LLM_STUB,
        LlmBackendArg::OpenAiCompat => RECORD_LLM_OPENAI_COMPAT,
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use scrybe_core::config::Config;

    fn args_with_title(title: &str) -> Args {
        Args {
            title: title.to_string(),
            source: None,
            input_device: None,
            system_backend: None,
            whisper_model: None,
            llm: None,
            root: None,
            no_bundle: false,
        }
    }

    #[test]
    fn test_resolve_uses_explicit_overrides_when_provided() {
        let cfg = Config::default();
        let mut args = args_with_title("explicit");
        args.source = Some(CaptureSourceArg::Mic);
        args.llm = Some(LlmBackendArg::Stub);
        args.whisper_model = Some(PathBuf::from("/explicit/model.bin"));
        let resolved = resolve(&cfg, &args);
        assert_eq!(resolved.source, CaptureSourceArg::Mic);
        assert_eq!(resolved.llm, LlmBackendArg::Stub);
        assert_eq!(
            resolved.whisper_model,
            Some(PathBuf::from("/explicit/model.bin"))
        );
        assert_eq!(resolved.title, "explicit");
    }

    #[test]
    fn test_should_use_bundle_returns_false_when_no_bundle_set() {
        let resolved = Resolved {
            title: "t".into(),
            source: CaptureSourceArg::MicSystem,
            system_backend: super::rec::SystemBackendArg::Sck,
            whisper_model: None,
            llm: LlmBackendArg::Stub,
            root: None,
            input_device: None,
        };
        let mut args = args_with_title("t");
        args.no_bundle = true;
        assert!(!should_use_bundle(&resolved, &args));
    }

    #[test]
    fn test_should_use_bundle_returns_false_for_non_system_source() {
        let resolved = Resolved {
            title: "t".into(),
            source: CaptureSourceArg::Mic,
            system_backend: super::rec::SystemBackendArg::Sck,
            whisper_model: None,
            llm: LlmBackendArg::Stub,
            root: None,
            input_device: None,
        };
        let args = args_with_title("t");
        assert!(!should_use_bundle(&resolved, &args));
    }

    #[cfg(feature = "system-capture-mac")]
    #[test]
    fn test_capture_source_arg_round_trip_via_strings() {
        for arg in [
            CaptureSourceArg::Synthetic,
            CaptureSourceArg::Mic,
            CaptureSourceArg::MicSystem,
        ] {
            let s = capture_source_arg_to_str(arg);
            assert_eq!(capture_source_arg_from_str(s), arg);
        }
    }

    #[cfg(feature = "system-capture-mac")]
    #[test]
    fn test_llm_backend_arg_round_trip_via_strings() {
        for arg in [LlmBackendArg::Stub, LlmBackendArg::OpenAiCompat] {
            let s = llm_backend_arg_to_str(arg);
            assert_eq!(llm_backend_arg_from_str(s), arg);
        }
    }

    #[cfg(feature = "system-capture-mac")]
    #[test]
    fn test_build_rec_argv_emits_required_flags_in_canonical_order() {
        let resolved = Resolved {
            title: "client-call".into(),
            source: CaptureSourceArg::MicSystem,
            system_backend: super::rec::SystemBackendArg::Sck,
            whisper_model: Some(PathBuf::from("/m.bin")),
            llm: LlmBackendArg::OpenAiCompat,
            root: None,
            input_device: None,
        };
        let argv = build_rec_argv(&resolved);
        assert_eq!(argv[0], "--title");
        assert_eq!(argv[1], "client-call");
        assert!(argv.iter().any(|a| a == "--source"));
        assert!(argv.iter().any(|a| a == "mic+system"));
        assert!(argv.iter().any(|a| a == "--llm"));
        assert!(argv.iter().any(|a| a == "openai-compat"));
        assert!(argv.iter().any(|a| a == "--whisper-model"));
        assert!(argv.iter().any(|a| a == "/m.bin"));
        assert!(argv.iter().any(|a| a == "--yes"));
    }

    #[cfg(feature = "system-capture-mac")]
    #[test]
    fn test_resolve_bundle_path_honors_env_override_when_dir_exists() {
        let tmp = tempfile::tempdir().unwrap();
        let bundle = tmp.path().join("scrybe.app");
        std::fs::create_dir(&bundle).unwrap();
        std::env::set_var(SCRYBE_BUNDLE_ENV, &bundle);
        let resolved = resolve_bundle_path();
        std::env::remove_var(SCRYBE_BUNDLE_ENV);
        assert_eq!(resolved.as_deref(), Some(bundle.as_path()));
    }

    #[cfg(feature = "system-capture-mac")]
    #[test]
    fn test_resolve_bundle_path_ignores_env_when_not_a_dir() {
        std::env::set_var(SCRYBE_BUNDLE_ENV, "/no/such/path/scrybe.app");
        let _ = resolve_bundle_path();
        std::env::remove_var(SCRYBE_BUNDLE_ENV);
    }
}
