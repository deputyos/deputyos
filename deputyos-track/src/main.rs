//! `deputyos-track` — release-tracker bot.
//!
//! Polls the upstream GitHub repo for each profile in `profiles/*.toml`,
//! compares the latest tag to `[profile].pinned_version`, and proposes a
//! diff that bumps the version. Local-first: the same binary runs in CI
//! every 30 min and on a contributor laptop via `make track`.
//!
//! Subcommands
//! -----------
//! * `check` — print which profiles have a newer upstream release.
//! * `propose` — emit `propose-<id>-<v>.patch` files (or stdout) with
//!   the bumped TOML body.
//! * `apply` — write the patches into `profiles/<id>.toml`. Gated by
//!   `--yes` to avoid clobbering by accident.
//! * `open-pr` — wraps `apply` + git + `gh pr create` for one profile.
//!
//! Network: subcommands hit `api.github.com`. `--offline` short-circuits
//! the HTTP path with synthetic data so `make ci` can exercise the tool
//! without leaking dependence on external services.
//!
//! This crate intentionally has *no* coupling to the `deputyctl` crate —
//! it parses only the `[profile]` section it needs. The TOML patcher is
//! a targeted line edit so comments and alignment in the profile files
//! are preserved verbatim.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};

use deputyos_track::github::{Channel, Client, Release};
use deputyos_track::patch::{bump_pinned_version, Patch};
use deputyos_track::profile::{self, LoadedProfile};
use deputyos_track::version::Version;

#[derive(Parser, Debug)]
#[command(
    name = "deputyos-track",
    version,
    about = "Polls upstream profile repositories and proposes pinned_version bumps."
)]
struct Cli {
    /// Path to the profiles directory.
    #[arg(long, default_value = "profiles")]
    profiles_dir: PathBuf,

    /// Skip network calls; emit no-op results. Used in CI smoke runs.
    #[arg(long, global = true)]
    offline: bool,

    #[command(subcommand)]
    cmd: Command,
}

#[derive(Subcommand, Debug)]
enum Command {
    /// Print profiles that have an upstream release newer than pinned.
    Check,
    /// Emit patch files describing the bump.
    Propose {
        /// Output dir for `propose-<id>-<v>.patch` files.
        #[arg(long, default_value = ".")]
        out_dir: PathBuf,
        /// Print the patched TOML to stdout instead of writing files.
        #[arg(long)]
        stdout: bool,
    },
    /// Apply the bump in place to profiles/<id>.toml.
    Apply {
        /// Required to actually write. Without it, prints a dry-run.
        #[arg(long)]
        yes: bool,
    },
    /// Apply + commit + push + open a PR via `gh`.
    #[command(name = "open-pr")]
    OpenPr {
        /// Required to actually run.
        #[arg(long)]
        yes: bool,
    },
}

fn main() -> Result<()> {
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("deputyos_track=info,info"));
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(false)
        .compact()
        .init();

    let cli = Cli::parse();
    match cli.cmd {
        Command::Check => cmd_check(&cli.profiles_dir, cli.offline),
        Command::Propose { out_dir, stdout } => {
            cmd_propose(&cli.profiles_dir, cli.offline, &out_dir, stdout)
        }
        Command::Apply { yes } => cmd_apply(&cli.profiles_dir, cli.offline, yes),
        Command::OpenPr { yes } => cmd_open_pr(&cli.profiles_dir, cli.offline, yes),
    }
}

/// One profile's tracker outcome.
struct Bump {
    profile: LoadedProfile,
    release: Release,
    new_version: String,
}

/// Inspect every profile and figure out which have a newer upstream tag.
fn discover(profiles_dir: &Path, offline: bool) -> Result<Vec<Bump>> {
    let profiles = profile::list(profiles_dir)?;
    if offline {
        tracing::info!("offline mode: skipping GitHub API; emitting empty bump list");
        return Ok(Vec::new());
    }
    let client = Client::default();
    let mut bumps = Vec::new();
    for p in profiles {
        let channel = Channel::parse(&p.profile.release_channel)?;
        let latest = match client.latest(&p.profile.upstream_repo, channel) {
            Ok(Some(r)) => r,
            Ok(None) => {
                tracing::info!(profile = %p.profile.id, "no upstream release available");
                continue;
            }
            Err(e) => {
                // Network / rate-limit / 5xx — log and skip; we want the
                // tool to keep working for the other profile.
                tracing::warn!(profile = %p.profile.id, error = %e, "upstream query failed");
                continue;
            }
        };
        let new = Version::parse(&latest.tag_name)
            .with_context(|| format!("parse upstream tag {:?}", latest.tag_name))?;
        let pinned = Version::parse(&p.profile.pinned_version)
            .with_context(|| format!("parse pinned_version for {}", p.profile.id))?;
        if new > pinned {
            // Strip a leading `v` for storage consistency with current profiles.
            let new_version = latest
                .tag_name
                .strip_prefix('v')
                .unwrap_or(&latest.tag_name)
                .to_string();
            bumps.push(Bump {
                profile: p,
                release: latest,
                new_version,
            });
        }
    }
    Ok(bumps)
}

fn cmd_check(profiles_dir: &Path, offline: bool) -> Result<()> {
    let bumps = discover(profiles_dir, offline)?;
    if bumps.is_empty() {
        println!("all profiles up to date");
        return Ok(());
    }
    for b in &bumps {
        let url = b.release.html_url.as_deref().unwrap_or("(no url)");
        println!(
            "{}: {} -> {}  ({})",
            b.profile.profile.id, b.profile.profile.pinned_version, b.new_version, url
        );
    }
    Ok(())
}

fn cmd_propose(profiles_dir: &Path, offline: bool, out_dir: &Path, to_stdout: bool) -> Result<()> {
    let bumps = discover(profiles_dir, offline)?;
    if bumps.is_empty() {
        println!("no bumps to propose");
        return Ok(());
    }
    if !to_stdout {
        std::fs::create_dir_all(out_dir)
            .with_context(|| format!("create out dir {}", out_dir.display()))?;
    }
    for b in &bumps {
        let original = std::fs::read_to_string(&b.profile.path)
            .with_context(|| format!("read {}", b.profile.path.display()))?;
        let patch = bump_pinned_version(&original, &b.new_version)?;
        if to_stdout {
            print_patch_summary(b, &patch);
            println!("---");
            print!("{}", patch.patched);
        } else {
            let fname = format!("propose-{}-{}.patch", b.profile.profile.id, b.new_version);
            let p = out_dir.join(&fname);
            std::fs::write(&p, &patch.patched).with_context(|| format!("write {}", p.display()))?;
            // Also drop a sidecar JSON with the metadata the workflow needs.
            let meta = serde_json::json!({
                "profile_id": b.profile.profile.id,
                "old_version": patch.old_version,
                "new_version": patch.new_version,
                "upstream_repo": b.profile.profile.upstream_repo,
                "release_url": b.release.html_url,
                "release_name": b.release.name,
                "release_body": b.release.body,
                "released_at": b.release.published_at,
                "profile_path": b.profile.path.to_string_lossy(),
            });
            let meta_path = out_dir.join(format!(
                "propose-{}-{}.json",
                b.profile.profile.id, b.new_version
            ));
            std::fs::write(&meta_path, serde_json::to_string_pretty(&meta)?)
                .with_context(|| format!("write {}", meta_path.display()))?;
            println!(
                "wrote {} ({} -> {})",
                p.display(),
                patch.old_version,
                patch.new_version
            );
        }
    }
    Ok(())
}

fn cmd_apply(profiles_dir: &Path, offline: bool, yes: bool) -> Result<()> {
    let bumps = discover(profiles_dir, offline)?;
    if bumps.is_empty() {
        println!("nothing to apply");
        return Ok(());
    }
    for b in &bumps {
        let original = std::fs::read_to_string(&b.profile.path)?;
        let patch = bump_pinned_version(&original, &b.new_version)?;
        if !yes {
            println!(
                "[dry-run] would bump {}: {} -> {}",
                b.profile.profile.id, patch.old_version, patch.new_version
            );
            continue;
        }
        std::fs::write(&b.profile.path, &patch.patched)
            .with_context(|| format!("write {}", b.profile.path.display()))?;
        println!(
            "applied: {} ({} -> {})",
            b.profile.path.display(),
            patch.old_version,
            patch.new_version
        );
    }
    Ok(())
}

fn cmd_open_pr(profiles_dir: &Path, offline: bool, yes: bool) -> Result<()> {
    let bumps = discover(profiles_dir, offline)?;
    if bumps.is_empty() {
        println!("nothing to open");
        return Ok(());
    }
    if !has_gh() {
        eprintln!("gh CLI not on PATH; skipping open-pr (use propose + apply locally instead)");
        return Ok(());
    }
    for b in &bumps {
        let original = std::fs::read_to_string(&b.profile.path)?;
        let patch = bump_pinned_version(&original, &b.new_version)?;
        if !yes {
            println!(
                "[dry-run] would open PR for {}: {} -> {}",
                b.profile.profile.id, patch.old_version, patch.new_version
            );
            continue;
        }
        open_one_pr(b, &patch)?;
    }
    Ok(())
}

fn has_gh() -> bool {
    std::process::Command::new("gh")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

fn open_one_pr(b: &Bump, patch: &Patch) -> Result<()> {
    let branch = format!("track/{}-{}", b.profile.profile.id, b.new_version);
    run("git", &["checkout", "-B", &branch])?;
    std::fs::write(&b.profile.path, &patch.patched)?;
    run("git", &["add", b.profile.path.to_string_lossy().as_ref()])?;
    let title = format!(
        "track: bump {} pinned_version {} -> {}",
        b.profile.profile.id, patch.old_version, patch.new_version
    );
    run("git", &["commit", "-m", &title])?;
    run("git", &["push", "-u", "origin", &branch])?;
    let body = build_pr_body(b);
    run(
        "gh",
        &[
            "pr", "create", "--title", &title, "--body", &body, "--label", "tracker", "--label",
            "auto-pr",
        ],
    )?;
    Ok(())
}

fn build_pr_body(b: &Bump) -> String {
    let mut body = format!(
        "Auto-bump from upstream `{}` tag `{}`.\n\n",
        b.profile.profile.upstream_repo, b.release.tag_name
    );
    if let Some(url) = &b.release.html_url {
        body.push_str(&format!("Release: {}\n", url));
    }
    if let Some(at) = &b.release.published_at {
        body.push_str(&format!("Published: {}\n", at));
    }
    body.push_str("\n---\n\n");
    if let Some(notes) = &b.release.body {
        let mut truncated = notes.clone();
        if truncated.len() > 4000 {
            truncated.truncate(4000);
            truncated.push_str("\n\n[...truncated]");
        }
        body.push_str(&truncated);
    }
    body
}

fn run(prog: &str, args: &[&str]) -> Result<()> {
    let status = std::process::Command::new(prog)
        .args(args)
        .status()
        .with_context(|| format!("spawn {prog}"))?;
    if !status.success() {
        anyhow::bail!("{prog} {args:?} exited with {status}");
    }
    Ok(())
}

fn print_patch_summary(b: &Bump, patch: &Patch) {
    println!(
        "# {}: {} -> {}",
        b.profile.profile.id, patch.old_version, patch.new_version
    );
}
