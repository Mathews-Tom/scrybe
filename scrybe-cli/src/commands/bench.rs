// Copyright 2026 Mathews Tom
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//     https://www.apache.org/licenses/LICENSE-2.0

//! `scrybe bench` — harvest Criterion benchmark results into a
//! versioned JSON snapshot under `<storage_root>/.bench/<git-sha>.json`.
//!
//! Behavior is deliberately decoupled from `cargo bench`: this command
//! does not invoke cargo. The maintainer runs
//! `cargo bench --bench pipeline -p scrybe-core` first; afterwards
//! `scrybe bench` walks the `target/criterion/` directory and
//! aggregates the per-benchmark `new/estimates.json` files emitted by
//! Criterion 0.5 into a single timestamped snapshot. This split keeps
//! the CLI binary free of cargo invocation logic and makes the
//! aggregator reusable from CI shells, makefiles, and the self-hosted
//! Tier-3 nightly runner described in `docs/system-design.md` §11.
//!
//! Honest scope: the snapshot is informational. The >10% regression
//! gate documented in §11 runs on the Tier-3 nightly runner via a
//! workflow that diffs two snapshots; this command produces the
//! snapshot, it does not implement the gate.

use std::path::{Path, PathBuf};
use std::time::SystemTime;

use anyhow::{anyhow, Context, Result};
use clap::Args;
use serde::{Deserialize, Serialize};

use scrybe_core::storage::atomic_replace;

#[derive(Args, Debug, Clone)]
pub struct BenchArgs {
    /// Criterion output directory. Defaults to `<workspace>/target/criterion`.
    #[arg(long)]
    pub criterion_dir: Option<PathBuf>,

    /// Storage root that owns the `.bench/` subdirectory. Defaults to
    /// the platform-conventional `~/scrybe/`.
    #[arg(long)]
    pub root: Option<PathBuf>,

    /// Git SHA tag for the snapshot file name. Defaults to the
    /// `SCRYBE_GIT_SHA` env var; falls back to `unknown`.
    #[arg(long)]
    pub git_sha: Option<String>,

    /// Print the snapshot to stdout instead of writing it.
    #[arg(long)]
    pub print: bool,
}

#[derive(Debug, Serialize, Deserialize, PartialEq)]
pub struct BenchSnapshot {
    pub git_sha: String,
    pub generated_at_secs: u64,
    pub criterion_dir: PathBuf,
    pub benches: Vec<BenchEntry>,
}

#[derive(Debug, Serialize, Deserialize, PartialEq)]
pub struct BenchEntry {
    /// Slash-joined criterion path, e.g. `vad/energy_vad/decide/1600`.
    pub id: String,
    pub mean_ns: f64,
    pub median_ns: f64,
    pub std_dev_ns: f64,
}

/// Run the harvest.
///
/// `async` matches the signature shape every other subcommand uses
/// (`bench::run`, `init::run`, `record::run`, …) so the dispatcher
/// in `commands::run` stays a flat match without per-subcommand
/// branching. Internally the body is fully synchronous.
///
/// # Errors
///
/// Surfaces `anyhow::Error` for any IO, parse, or atomic-write failure.
#[allow(clippy::unused_async)]
pub async fn run(args: BenchArgs) -> Result<()> {
    let snapshot = harvest(&args)?;
    if args.print {
        let json =
            serde_json::to_string_pretty(&snapshot).context("serializing snapshot for --print")?;
        println!("{json}");
        return Ok(());
    }
    let target = snapshot_path(&args, &snapshot.git_sha)?;
    write_snapshot(&target, &snapshot)?;
    println!(
        "wrote {} bench entries to {}",
        snapshot.benches.len(),
        target.display()
    );
    Ok(())
}

fn harvest(args: &BenchArgs) -> Result<BenchSnapshot> {
    let criterion_dir = resolve_criterion_dir(args);
    let benches = scan_criterion_tree(&criterion_dir)?;
    let git_sha = resolve_git_sha(args);
    let generated_at_secs = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    Ok(BenchSnapshot {
        git_sha,
        generated_at_secs,
        criterion_dir,
        benches,
    })
}

fn resolve_criterion_dir(args: &BenchArgs) -> PathBuf {
    args.criterion_dir
        .clone()
        .unwrap_or_else(default_criterion_dir)
}

fn default_criterion_dir() -> PathBuf {
    PathBuf::from("target").join("criterion")
}

fn resolve_git_sha(args: &BenchArgs) -> String {
    args.git_sha
        .clone()
        .or_else(|| std::env::var("SCRYBE_GIT_SHA").ok())
        .unwrap_or_else(|| "unknown".to_string())
}

fn snapshot_path(args: &BenchArgs, git_sha: &str) -> Result<PathBuf> {
    let root = match &args.root {
        Some(p) => p.clone(),
        None => default_storage_root()?,
    };
    let dir = root.join(".bench");
    std::fs::create_dir_all(&dir)
        .with_context(|| format!("creating snapshot directory {}", dir.display()))?;
    Ok(dir.join(format!("{git_sha}.json")))
}

fn default_storage_root() -> Result<PathBuf> {
    if let Ok(home) = std::env::var("HOME") {
        return Ok(PathBuf::from(home).join("scrybe"));
    }
    Err(anyhow!(
        "no $HOME set; pass --root explicitly to choose the snapshot location"
    ))
}

fn write_snapshot(target: &Path, snapshot: &BenchSnapshot) -> Result<()> {
    let payload = serde_json::to_vec_pretty(snapshot).context("serializing snapshot to JSON")?;
    atomic_replace(target, &payload)
        .with_context(|| format!("writing snapshot to {}", target.display()))?;
    Ok(())
}

/// Walk `criterion_dir` and harvest every `<bench>/<id>/new/estimates.json`.
/// Skips intermediate `report/` directories Criterion uses for HTML output.
fn scan_criterion_tree(criterion_dir: &Path) -> Result<Vec<BenchEntry>> {
    if !criterion_dir.is_dir() {
        return Err(anyhow!(
            "criterion directory {} does not exist; run `cargo bench --bench pipeline -p scrybe-core` first",
            criterion_dir.display()
        ));
    }
    let mut entries = Vec::new();
    walk_for_estimates(criterion_dir, criterion_dir, &mut entries)?;
    entries.sort_by(|a, b| a.id.cmp(&b.id));
    Ok(entries)
}

fn walk_for_estimates(base: &Path, current: &Path, out: &mut Vec<BenchEntry>) -> Result<()> {
    let read =
        std::fs::read_dir(current).with_context(|| format!("reading {}", current.display()))?;
    for entry_result in read {
        let entry = entry_result.with_context(|| format!("iterating {}", current.display()))?;
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        if path.file_name().and_then(|n| n.to_str()) == Some("report") {
            continue;
        }
        let estimates = path.join("new").join("estimates.json");
        if estimates.is_file() {
            let id = id_from_path(base, &path);
            match parse_estimates(&estimates) {
                Ok(stats) => out.push(BenchEntry {
                    id,
                    mean_ns: stats.mean,
                    median_ns: stats.median,
                    std_dev_ns: stats.std_dev,
                }),
                Err(e) => {
                    tracing::warn!(
                        path = %estimates.display(),
                        error = %e,
                        "skipping unparseable criterion estimates file"
                    );
                }
            }
        } else {
            walk_for_estimates(base, &path, out)?;
        }
    }
    Ok(())
}

fn id_from_path(base: &Path, leaf: &Path) -> String {
    leaf.strip_prefix(base)
        .ok()
        .and_then(|relative| relative.to_str())
        .map_or_else(
            || leaf.display().to_string(),
            |s| s.replace(std::path::MAIN_SEPARATOR, "/"),
        )
}

#[derive(Debug)]
struct EstimateStats {
    mean: f64,
    median: f64,
    std_dev: f64,
}

fn parse_estimates(path: &Path) -> Result<EstimateStats> {
    let body =
        std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    let parsed: serde_json::Value =
        serde_json::from_str(&body).with_context(|| format!("parsing {}", path.display()))?;
    let mean = read_point(&parsed, "mean")
        .ok_or_else(|| anyhow!("missing mean.point_estimate in {}", path.display()))?;
    let median = read_point(&parsed, "median")
        .ok_or_else(|| anyhow!("missing median.point_estimate in {}", path.display()))?;
    let std_dev = read_point(&parsed, "std_dev").unwrap_or(0.0);
    Ok(EstimateStats {
        mean,
        median,
        std_dev,
    })
}

fn read_point(parsed: &serde_json::Value, key: &str) -> Option<f64> {
    parsed
        .get(key)
        .and_then(|v| v.get("point_estimate"))
        .and_then(serde_json::Value::as_f64)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use std::fs;

    use super::*;
    use pretty_assertions::assert_eq;

    fn write_estimate(dir: &Path, id_path: &str, mean: f64, median: f64, std_dev: f64) {
        let leaf = dir.join(id_path);
        fs::create_dir_all(leaf.join("new")).unwrap();
        let body = format!(
            r#"{{
              "mean":   {{"point_estimate": {mean}}},
              "median": {{"point_estimate": {median}}},
              "std_dev":{{"point_estimate": {std_dev}}}
            }}"#
        );
        fs::write(leaf.join("new").join("estimates.json"), body).unwrap();
    }

    #[test]
    fn test_scan_criterion_tree_collects_estimates_from_nested_layout() {
        let dir = tempfile::tempdir().unwrap();
        write_estimate(
            dir.path(),
            "vad/energy_vad/decide/1600",
            1234.5,
            1230.0,
            12.7,
        );
        write_estimate(
            dir.path(),
            "resample/linear/48000_to_16000",
            5678.0,
            5670.0,
            42.0,
        );

        let entries = scan_criterion_tree(dir.path()).unwrap();

        assert_eq!(entries.len(), 2);
        let by_id = |id: &str| entries.iter().find(|e| e.id == id).expect("entry exists");
        let v = by_id("resample/linear/48000_to_16000");
        assert!((v.mean_ns - 5678.0).abs() < f64::EPSILON);
        assert!((v.std_dev_ns - 42.0).abs() < f64::EPSILON);
        let w = by_id("vad/energy_vad/decide/1600");
        assert!((w.median_ns - 1230.0).abs() < f64::EPSILON);
    }

    #[test]
    fn test_scan_criterion_tree_skips_report_directory_in_walk() {
        let dir = tempfile::tempdir().unwrap();
        write_estimate(dir.path(), "vad/decide/1", 1.0, 1.0, 0.0);
        // Criterion's HTML output: presence must not crash the walk.
        fs::create_dir_all(dir.path().join("report")).unwrap();
        fs::write(dir.path().join("report").join("index.html"), "").unwrap();

        let entries = scan_criterion_tree(dir.path()).unwrap();

        assert_eq!(entries.len(), 1);
    }

    #[test]
    fn test_scan_criterion_tree_returns_error_when_dir_missing() {
        let err = scan_criterion_tree(Path::new("/nonexistent/scrybe/criterion"))
            .err()
            .unwrap();

        assert!(err.to_string().contains("does not exist"));
    }

    #[test]
    fn test_scan_criterion_tree_returns_empty_when_dir_has_no_estimates() {
        let dir = tempfile::tempdir().unwrap();

        let entries = scan_criterion_tree(dir.path()).unwrap();

        assert!(entries.is_empty());
    }

    /// Both env-var paths exercised in one test so the process-global
    /// `SCRYBE_GIT_SHA` mutation does not race against a sibling test.
    /// `cargo test` runs unit tests in parallel inside the same process,
    /// and split tests would each see the other's env writes.
    #[test]
    fn test_resolve_git_sha_priority_arg_then_env_then_unknown() {
        let prior = std::env::var("SCRYBE_GIT_SHA").ok();

        // 1. Explicit arg wins regardless of env.
        std::env::set_var("SCRYBE_GIT_SHA", "from-env");
        let arg_path = resolve_git_sha(&BenchArgs {
            criterion_dir: None,
            root: None,
            git_sha: Some("from-arg".into()),
            print: false,
        });

        // 2. Env wins when arg is absent.
        let env_path = resolve_git_sha(&BenchArgs {
            criterion_dir: None,
            root: None,
            git_sha: None,
            print: false,
        });

        // 3. Falls back to "unknown" when arg and env are both absent.
        std::env::remove_var("SCRYBE_GIT_SHA");
        let unknown_path = resolve_git_sha(&BenchArgs {
            criterion_dir: None,
            root: None,
            git_sha: None,
            print: false,
        });

        if let Some(p) = prior {
            std::env::set_var("SCRYBE_GIT_SHA", p);
        }

        assert_eq!(arg_path, "from-arg");
        assert_eq!(env_path, "from-env");
        assert_eq!(unknown_path, "unknown");
    }

    #[tokio::test]
    async fn test_run_writes_snapshot_to_bench_subdirectory() {
        let workdir = tempfile::tempdir().unwrap();
        let crit = workdir.path().join("crit");
        write_estimate(&crit, "vad/decide/1", 100.0, 99.0, 5.0);

        let root = workdir.path().join("scrybe");
        run(BenchArgs {
            criterion_dir: Some(crit),
            root: Some(root.clone()),
            git_sha: Some("abcdef".into()),
            print: false,
        })
        .await
        .unwrap();

        let snapshot_path = root.join(".bench").join("abcdef.json");
        let body = std::fs::read_to_string(&snapshot_path).unwrap();
        let parsed: BenchSnapshot = serde_json::from_str(&body).unwrap();
        assert_eq!(parsed.git_sha, "abcdef");
        assert_eq!(parsed.benches.len(), 1);
    }

    #[tokio::test]
    async fn test_run_with_print_does_not_create_snapshot_file() {
        let workdir = tempfile::tempdir().unwrap();
        let crit = workdir.path().join("crit");
        write_estimate(&crit, "vad/decide/1", 100.0, 99.0, 5.0);
        let root = workdir.path().join("scrybe");

        run(BenchArgs {
            criterion_dir: Some(crit),
            root: Some(root.clone()),
            git_sha: Some("abcdef".into()),
            print: true,
        })
        .await
        .unwrap();

        let bench_dir = root.join(".bench");
        assert!(
            !bench_dir.exists(),
            "--print should not create snapshot directory"
        );
    }

    #[test]
    fn test_id_from_path_uses_forward_slashes_regardless_of_platform() {
        let base = Path::new("/tmp/crit");
        let leaf = base.join("vad").join("decide").join("1600");

        let id = id_from_path(base, &leaf);

        assert_eq!(id, "vad/decide/1600");
    }

    #[test]
    fn test_parse_estimates_tolerates_missing_std_dev_and_returns_zero() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("estimates.json");
        std::fs::write(
            &path,
            r#"{"mean":{"point_estimate":1.0}, "median":{"point_estimate":2.0}}"#,
        )
        .unwrap();

        let stats = parse_estimates(&path).unwrap();

        assert!((stats.std_dev - 0.0).abs() < f64::EPSILON);
        assert!((stats.mean - 1.0).abs() < f64::EPSILON);
        assert!((stats.median - 2.0).abs() < f64::EPSILON);
    }
}
