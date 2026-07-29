//! Profile manifest reader.
//!
//! We only care about the `[profile]` section here — everything else is
//! left untouched in the on-disk TOML by the patcher in `patch.rs`. We
//! keep our own minimal struct rather than depending on `deputyctl` so
//! `deputyos-track` can evolve independently of the runtime crate.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
pub struct ProfileFile {
    pub profile: ProfileSection,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ProfileSection {
    pub id: String,
    pub display_name: String,
    pub upstream_repo: String,
    pub release_channel: String,
    pub pinned_version: String,
}

/// One on-disk profile, paired with its source path.
#[derive(Debug, Clone)]
pub struct LoadedProfile {
    pub path: PathBuf,
    pub profile: ProfileSection,
}

/// Enumerate every `*.toml` profile under `dir`.
pub fn list(dir: &Path) -> Result<Vec<LoadedProfile>> {
    if !dir.is_dir() {
        anyhow::bail!("profiles dir missing: {}", dir.display());
    }
    let mut out = Vec::new();
    for entry in std::fs::read_dir(dir).with_context(|| format!("read_dir {}", dir.display()))? {
        let entry = entry?;
        let path = entry.path();
        if path.extension().and_then(|s| s.to_str()) != Some("toml") {
            continue;
        }
        let text =
            std::fs::read_to_string(&path).with_context(|| format!("read {}", path.display()))?;
        let parsed: ProfileFile =
            toml::from_str(&text).with_context(|| format!("parse {}", path.display()))?;
        out.push(LoadedProfile {
            path,
            profile: parsed.profile,
        });
    }
    out.sort_by(|a, b| a.profile.id.cmp(&b.profile.id));
    Ok(out)
}
