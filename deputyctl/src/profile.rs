//! Profile discovery and loading.
//!
//! Wraps [`crate::manifest`] with on-disk enumeration. Pure functions — no
//! global state — so doctor/limits/up commands can call them independently.

use std::path::{Path, PathBuf};

use anyhow::{anyhow, Context, Result};
use serde::Serialize;

use crate::manifest::{self, Manifest};
use crate::paths;

/// One installed profile on disk.
#[derive(Debug, Clone, Serialize)]
pub struct InstalledProfile {
    pub id: String,
    pub display_name: String,
    pub pinned_version: String,
    pub release_channel: String,
    pub manifest_path: PathBuf,
    pub active: bool,
}

/// Enumerate every `*.toml` profile under the configured profiles dir.
///
/// Returns an empty vec — not an error — if the dir is missing. Parse errors
/// of individual manifests are surfaced as `Err` because a malformed manifest
/// in production would break image-rev guarantees.
pub fn list() -> Result<Vec<InstalledProfile>> {
    let dir = paths::profiles_dir();
    if !dir.is_dir() {
        tracing::debug!(path = %dir.display(), "profiles dir missing");
        return Ok(Vec::new());
    }
    let active = paths::read_active_profile_id();
    let mut out = Vec::new();
    for entry in std::fs::read_dir(&dir)
        .with_context(|| format!("reading profiles dir {}", dir.display()))?
    {
        let entry = entry?;
        let path = entry.path();
        if path.extension().and_then(|s| s.to_str()) != Some("toml") {
            continue;
        }
        let m = manifest::load(&path).with_context(|| format!("loading {}", path.display()))?;
        let active_match = active.as_deref() == Some(m.profile.id.as_str());
        tracing::debug!(profile = %m.profile.id, "loaded profile");
        out.push(InstalledProfile {
            id: m.profile.id.clone(),
            display_name: m.profile.display_name.clone(),
            pinned_version: m.profile.pinned_version.clone(),
            release_channel: m.profile.release_channel.clone(),
            manifest_path: path,
            active: active_match,
        });
    }
    out.sort_by(|a, b| a.id.cmp(&b.id));
    Ok(out)
}

/// Load the manifest for the active profile.
///
/// Errors if `/etc/deputyos/active-profile` is missing or names a profile
/// whose manifest cannot be located/parsed.
pub fn load_active() -> Result<(String, Manifest)> {
    let id = paths::read_active_profile_id().ok_or_else(|| {
        anyhow!(
            "no active profile (missing {})",
            paths::active_profile_file().display()
        )
    })?;
    let path = paths::profiles_dir().join(format!("{id}.toml"));
    let m = manifest::load(&path).with_context(|| format!("loading active profile {id}"))?;
    Ok((id, m))
}

/// Load a profile by id (used by `profile switch`-style flows in M2).
pub fn load_by_id(id: &str) -> Result<Manifest> {
    let path: &Path = &paths::profiles_dir().join(format!("{id}.toml"));
    manifest::load(path).with_context(|| format!("loading profile {id}"))
}
