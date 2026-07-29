//! Profile manifest deserialization.
//!
//! Manifest schema is documented in `docs/02-profiles.md`. The structs here
//! must round-trip both `profiles/openclaw.toml` and `profiles/hermes.toml`
//! verbatim — the integration test in `tests/manifest.rs` enforces that.
//!
//! The schema is intentionally permissive: every section beyond `[profile]`,
//! `[paths]`, `[runtime]`, `[service]`, and `[health]` is optional, because
//! profiles legitimately omit sections that don't apply (e.g. OpenClaw has
//! no `[memory]` block today, Hermes has no `[channels]` defaults override).

use std::collections::BTreeMap;
use std::path::Path;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Manifest {
    pub profile: ProfileSection,
    pub paths: PathsSection,
    pub runtime: RuntimeSection,
    pub service: ServiceSection,
    pub health: HealthSection,
    #[serde(default)]
    pub apparmor: Option<AppArmorSection>,
    #[serde(default)]
    pub kernel: Option<KernelSection>,
    #[serde(default)]
    pub wizard: Option<WizardSection>,
    #[serde(default)]
    pub channels: Option<ChannelsSection>,
    #[serde(default)]
    pub memory: Option<MemorySection>,
    #[serde(default)]
    pub upgrade: Option<UpgradeSection>,
    /// Profile-level mount defaults surfaced in the wizard Drives step (M3.5).
    /// Optional; profiles without it simply offer no suggestions.
    #[serde(default)]
    pub mounts: Option<MountsSection>,
    /// Air-gapped build defaults for this profile (M4.5 Lane F). Optional;
    /// non-airgap builds ignore it.
    #[serde(default)]
    pub airgap: Option<AirgapSection>,
    /// Profile-level egress defaults surfaced in the wizard Egress step (M5.5
    /// Lane F). Optional; profiles without it offer no pre-selected mode/hints.
    #[serde(default)]
    pub default_egress: Option<EgressSection>,
}

/// Air-gapped build defaults (M4.5 Lane F). The wizard pre-selects
/// `default_provider` on an airgap build so the baked local LLM is chosen
/// automatically. Convention: `local-llamacpp-airgap`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AirgapSection {
    /// Provider id to pre-select on an airgap build (e.g.
    /// `local-llamacpp-airgap`). Must be a provider the airgap wizard
    /// actually offers (a `local-llamacpp` entry from the catalog).
    pub default_provider: String,
}

/// Profile-level egress defaults for the wizard Egress step (M5.5 Lane F).
/// A profile can pre-select an egress mode and suggest a starter allow-list of
/// hosts (LLM providers + chat gateways the profile needs). The wizard shows
/// these as hints; the live policy is seeded from `network-defaults.json`
/// (M2) when the operator switches to `whitelist`. No secrets live here.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EgressSection {
    /// Recommended default mode: `"open"` | `"whitelist"` | `"airgap"`. The
    /// wizard pre-selects this radio but does not enforce it.
    #[serde(default = "default_egress_mode")]
    pub mode: String,
    /// Suggested starter hostnames for `whitelist` (e.g. `["api.openai.com"]`).
    /// Surfaced as hints; the operator's `network-defaults.json` is the seed.
    #[serde(default)]
    pub allow_hosts: Vec<String>,
}

fn default_egress_mode() -> String {
    "open".into()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProfileSection {
    pub id: String,
    pub display_name: String,
    pub upstream_repo: String,
    pub release_channel: String,
    pub min_ram_mb: u32,
    pub pinned_version: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PathsSection {
    pub install_root: String,
    pub data_dir: String,
    pub binary: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RuntimeSection {
    pub language: String,
    #[serde(default)]
    pub node_version: Option<String>,
    #[serde(default)]
    pub python_version: Option<String>,
    pub package_manager: String,
    #[serde(default)]
    pub extra_apt: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServiceSection {
    pub unit: String,
    pub entrypoint: String,
    pub ports: Vec<u16>,
    #[serde(default = "default_restart_policy")]
    pub restart_policy: String,
}

fn default_restart_policy() -> String {
    "always".into()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HealthSection {
    pub http_check: String,
    pub journal_unit: String,
    pub startup_grace_s: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppArmorSection {
    pub profile: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KernelSection {
    #[serde(default)]
    pub required_sysctls: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WizardSection {
    #[serde(default)]
    pub prompts: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChannelsSection {
    #[serde(default)]
    pub supported: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemorySection {
    #[serde(default)]
    pub session_db: Option<String>,
    #[serde(default)]
    pub backup_strategy: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpgradeSection {
    #[serde(default)]
    pub preserve_dirs: Vec<String>,
    #[serde(default)]
    pub post_upgrade_hooks: Vec<String>,
}

/// Profile-level mount defaults for the wizard Drives step (M3.5 Lane F).
/// A profile can default-suggest guest paths and a default mode; the user
/// still confirms each mount in the wizard. No secrets live here —
/// credentials stay in `/etc/deputyos/secrets.env`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MountsSection {
    /// Default mode for suggested mounts: `"ro"` | `"rw"`. Defaults to `"ro"`.
    #[serde(default = "default_mounts_mode")]
    pub default_mode: String,
    /// Suggested guest paths to offer (e.g. `["/mnt/deputyos/documents"]`).
    /// Each must live under `/mnt/deputyos/`; the wizard validates.
    #[serde(default)]
    pub suggested_paths: Vec<String>,
}

fn default_mounts_mode() -> String {
    "ro".into()
}

/// Load a manifest from a TOML file on disk.
pub fn load(path: impl AsRef<Path>) -> Result<Manifest> {
    let path = path.as_ref();
    let raw = std::fs::read_to_string(path)
        .with_context(|| format!("reading manifest from {}", path.display()))?;
    parse(&raw).with_context(|| format!("parsing manifest at {}", path.display()))
}

/// Parse a manifest from a TOML string.
pub fn parse(raw: &str) -> Result<Manifest> {
    Ok(toml::from_str(raw)?)
}
