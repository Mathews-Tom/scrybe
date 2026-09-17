// Copyright 2026 Mathews Tom
// Licensed under the Apache License, Version 2.0

//! Regenerate `notes.md` from a session's durable transcript.
//!
//! Session resolution, eligibility, the durable replacement, and cache
//! invalidation belong to the shared application service, and so does
//! the configured provider that produces the text. What stays here is
//! the command: its arguments, and where it reports the result.

use std::path::PathBuf;

#[cfg(feature = "llm-openai-compat")]
use anyhow::Context;
use anyhow::Result;
use clap::Args as ClapArgs;
#[cfg(feature = "llm-openai-compat")]
use scrybe_application::sessions::ConfiguredNotesGenerator;
#[cfg(feature = "llm-openai-compat")]
use scrybe_application::SessionRef;

#[cfg(feature = "llm-openai-compat")]
use crate::runtime::application;

#[derive(ClapArgs, Debug)]
pub struct Args {
    /// Session folder name, or an unambiguous session-ID prefix. Paths
    /// are not accepted: every session resolves beneath the configured
    /// storage root.
    pub id_or_folder: String,

    /// Override the storage root from config.
    #[arg(long)]
    pub root: Option<PathBuf>,
}

/// Regenerate notes from the canonical durable transcript.
///
/// # Errors
///
/// Returns configuration, transcript-parse, provider, or storage errors.
#[allow(clippy::unused_async)]
pub async fn run(args: Args) -> Result<()> {
    #[cfg(feature = "llm-openai-compat")]
    {
        let app = application(args.root.as_deref())?;
        let repository = app.sessions();
        let id = SessionRef::parse(&args.id_or_folder)
            .map_err(scrybe_application::ApplicationError::from)
            .with_context(|| format!("resolving session {}", args.id_or_folder))?;
        let generator = ConfiguredNotesGenerator::new(app.config().load()?);
        let result = repository
            .regenerate_notes(&id, &generator)
            .await
            .with_context(|| format!("regenerating notes for session {}", args.id_or_folder))?;
        let path = repository.root().resolve(&result.id).join("notes.md");
        println!("scrybe notes: wrote {}", path.display());
        Ok(())
    }

    #[cfg(not(feature = "llm-openai-compat"))]
    {
        let _ = args;
        anyhow::bail!("scrybe notes requires a build with the `llm-openai-compat` feature");
    }
}
