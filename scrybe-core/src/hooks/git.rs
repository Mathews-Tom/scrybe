// Copyright 2026 Mathews Tom
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//     https://www.apache.org/licenses/LICENSE-2.0

//! `Hook::Git` reference implementation.
//!
//! Behind the `hook-git` cargo feature so the default workspace build
//! does not pull `git2` (which links libgit2). When enabled, the hook
//! commits `notes.md` and `meta.toml` to a git repository at
//! `repo_root` whenever the session emits `LifecycleEvent::NotesGenerated`.
//!
//! This file is **always compiled** (the trait shape is stable
//! regardless of the feature) so call sites in `scrybe-cli` and the
//! Android shell can refer to `GitHook` unconditionally; the
//! `hook-git`-disabled build returns `HookError::Hook` from `on_event`
//! to make the missing-feature obvious.

use std::path::PathBuf;

use async_trait::async_trait;

use crate::error::HookError;
use crate::hooks::{Hook, LifecycleEvent};

/// Configuration for the git auto-commit hook.
#[derive(Clone, Debug)]
pub struct GitHookConfig {
    /// Repository root. Must already be a `git init`-ed repo.
    pub repo_root: PathBuf,
    /// Commit message template. `{title}` is substituted with the
    /// session title (or "untitled session" when none was supplied).
    pub commit_message: String,
    /// Optional remote name to push to after commit. `None` skips push.
    pub push_remote: Option<String>,
}

impl GitHookConfig {
    /// Construct with the documented v0.1 default commit message.
    #[must_use]
    pub fn new(repo_root: PathBuf) -> Self {
        Self {
            repo_root,
            commit_message: "scrybe: notes for {title}".to_string(),
            push_remote: None,
        }
    }
}

/// Git auto-commit hook. Subscribes to `NotesGenerated`; ignores all
/// other events.
pub struct GitHook {
    config: GitHookConfig,
}

impl GitHook {
    #[must_use]
    pub const fn new(config: GitHookConfig) -> Self {
        Self { config }
    }

    /// Render the commit message, substituting the `{title}`
    /// placeholder. Title is read from the session-folder name when no
    /// explicit title is in scope; the orchestrator passes
    /// `notes_path` whose parent folder name is `session_folder_name`.
    #[must_use]
    #[allow(clippy::literal_string_with_formatting_args)]
    pub fn render_commit_message(&self, title: &str) -> String {
        self.config.commit_message.replace("{title}", title)
    }
}

#[async_trait]
impl Hook for GitHook {
    async fn on_event(&self, event: &LifecycleEvent) -> Result<(), HookError> {
        let LifecycleEvent::NotesGenerated { notes_path, .. } = event else {
            return Ok(());
        };
        let title = derive_title_from_path(notes_path);
        let message = self.render_commit_message(&title);
        commit_notes(&self.config, notes_path, &message).await
    }

    fn name(&self) -> &'static str {
        "git"
    }
}

/// Best-effort title extraction from the notes-path's parent folder.
/// `2026-04-29-1430-acme-discovery-01HXY7K9` → `acme-discovery`.
fn derive_title_from_path(notes_path: &std::path::Path) -> String {
    let parent = match notes_path.parent().and_then(std::path::Path::file_name) {
        Some(name) => name.to_string_lossy().into_owned(),
        None => return "untitled session".to_string(),
    };
    let parts: Vec<&str> = parent.split('-').collect();
    if parts.len() < 6 {
        return "untitled session".to_string();
    }
    let middle = &parts[4..parts.len() - 1];
    if middle.is_empty() {
        "untitled session".to_string()
    } else {
        middle.join("-")
    }
}

#[cfg(feature = "hook-git")]
async fn commit_notes(
    config: &GitHookConfig,
    notes_path: &std::path::Path,
    message: &str,
) -> Result<(), HookError> {
    use git2::{IndexAddOption, Repository, Signature};

    let repo_root = config.repo_root.clone();
    let notes = notes_path.to_owned();
    let push_remote = config.push_remote.clone();
    let message = message.to_string();

    tokio::task::spawn_blocking(move || {
        let repo = Repository::open(&repo_root).map_err(|e| HookError::Hook(Box::new(e)))?;
        let mut index = repo.index().map_err(|e| HookError::Hook(Box::new(e)))?;
        index
            .add_all([notes].iter(), IndexAddOption::DEFAULT, None)
            .map_err(|e| HookError::Hook(Box::new(e)))?;
        index.write().map_err(|e| HookError::Hook(Box::new(e)))?;
        let oid = index
            .write_tree()
            .map_err(|e| HookError::Hook(Box::new(e)))?;
        let tree = repo
            .find_tree(oid)
            .map_err(|e| HookError::Hook(Box::new(e)))?;
        let sig = Signature::now("scrybe", "scrybe@localhost")
            .map_err(|e| HookError::Hook(Box::new(e)))?;
        let parents: Vec<git2::Commit<'_>> = match repo.head() {
            Ok(head) => head
                .target()
                .and_then(|oid| repo.find_commit(oid).ok())
                .into_iter()
                .collect(),
            Err(_) => Vec::new(),
        };
        let parent_refs: Vec<&git2::Commit<'_>> = parents.iter().collect();
        repo.commit(Some("HEAD"), &sig, &sig, &message, &tree, &parent_refs)
            .map_err(|e| HookError::Hook(Box::new(e)))?;
        if let Some(remote_name) = push_remote {
            let mut remote = repo
                .find_remote(&remote_name)
                .map_err(|e| HookError::Hook(Box::new(e)))?;
            remote
                .push(&["HEAD"], None)
                .map_err(|e| HookError::Hook(Box::new(e)))?;
        }
        Ok::<(), HookError>(())
    })
    .await
    .map_err(|e| HookError::Hook(Box::new(e)))??;

    Ok(())
}

#[cfg(not(feature = "hook-git"))]
#[allow(clippy::unused_async)]
async fn commit_notes(
    _config: &GitHookConfig,
    _notes_path: &std::path::Path,
    _message: &str,
) -> Result<(), HookError> {
    Err(HookError::Hook(Box::new(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "scrybe-core was built without the `hook-git` cargo feature; \
         enable it to commit notes via git2",
    ))))
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use std::path::PathBuf;
    use std::sync::Arc;

    use super::*;
    use crate::context::MeetingContext;
    use crate::types::SessionId;
    use pretty_assertions::assert_eq;

    fn cfg() -> GitHookConfig {
        GitHookConfig::new(PathBuf::from("/tmp/repo"))
    }

    #[test]
    fn test_git_hook_render_commit_message_substitutes_title_placeholder() {
        let hook = GitHook::new(cfg());

        let rendered = hook.render_commit_message("acme-discovery");

        assert_eq!(rendered, "scrybe: notes for acme-discovery");
    }

    #[test]
    fn test_git_hook_render_commit_message_handles_no_placeholder() {
        let mut config = cfg();
        config.commit_message = "scrybe-static".to_string();
        let hook = GitHook::new(config);

        let rendered = hook.render_commit_message("ignored");

        assert_eq!(rendered, "scrybe-static");
    }

    #[test]
    fn test_derive_title_from_path_extracts_middle_slug_segments() {
        let path = PathBuf::from("/tmp/2026-04-29-1430-acme-discovery-01HXY7K9RZ/notes.md");

        let title = derive_title_from_path(&path);

        assert_eq!(title, "acme-discovery");
    }

    #[test]
    fn test_derive_title_from_path_returns_default_for_short_folder_name() {
        let path = PathBuf::from("/tmp/short/notes.md");

        let title = derive_title_from_path(&path);

        assert_eq!(title, "untitled session");
    }

    #[tokio::test]
    async fn test_git_hook_ignores_session_start_event() {
        let hook = GitHook::new(cfg());
        let event = LifecycleEvent::SessionStart {
            id: SessionId::new(),
            ctx: Arc::new(MeetingContext::default()),
        };

        let result = hook.on_event(&event).await;

        assert!(result.is_ok());
    }

    #[cfg(not(feature = "hook-git"))]
    #[tokio::test]
    async fn test_git_hook_returns_unsupported_when_feature_disabled() {
        let hook = GitHook::new(cfg());
        let event = LifecycleEvent::NotesGenerated {
            id: SessionId::new(),
            notes_path: PathBuf::from("/tmp/2026-04-29-1430-x-01HXY7K9RZ/notes.md"),
        };

        let err = hook.on_event(&event).await.unwrap_err();

        let HookError::Hook(boxed) = err else {
            panic!("expected HookError::Hook with feature disabled");
        };
        let message = boxed.to_string();
        assert!(message.contains("hook-git"), "message: {message}");
    }

    #[cfg(feature = "hook-git")]
    #[tokio::test]
    async fn test_git_hook_commits_notes_to_initialized_repository() {
        use git2::Repository;
        let dir = tempfile::tempdir().unwrap();
        let repo_root = dir.path().to_path_buf();
        Repository::init(&repo_root).unwrap();
        let notes_dir = repo_root.join("2026-04-29-1430-test-01HXY7K9RZ");
        std::fs::create_dir_all(&notes_dir).unwrap();
        let notes_path = notes_dir.join("notes.md");
        std::fs::write(&notes_path, b"# Test\n## TL;DR\n- hi\n").unwrap();

        let hook = GitHook::new(GitHookConfig::new(repo_root.clone()));
        let event = LifecycleEvent::NotesGenerated {
            id: SessionId::new(),
            notes_path,
        };

        hook.on_event(&event).await.unwrap();

        let repo = Repository::open(&repo_root).unwrap();
        let head = repo.head().unwrap();
        let commit = head.peel_to_commit().unwrap();
        assert!(
            commit.message().unwrap().contains("test"),
            "commit message: {}",
            commit.message().unwrap()
        );
    }
}
