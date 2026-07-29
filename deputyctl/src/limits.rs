//! `deputyctl limits` — per-device capability + limitation report.
//!
//! Source of truth at runtime: `/etc/deputyos/limits.json` (baked at image
//! build time by the Lane B Ansible role). The schema is deliberately small
//! and stable; new capability flags are added by extending [`Capabilities`]
//! with `#[serde(default)]` so older images parse forward-compatibly.
//!
//! See the format spec in `docs/14-limitations.md`
//! §"The `deputyctl limits` command (spec)".

use std::path::Path;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::paths;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Limits {
    pub target: String,
    pub tier: String,
    pub ram_mb: u32,
    pub ram_class: String,
    pub storage_class: String,
    #[serde(default)]
    pub capabilities: Capabilities,
    #[serde(default)]
    pub limitations: Vec<Limitation>,
    /// True if this target can run any tier of the M4.5 air-gapped LLM
    /// bundle. Older limits files default to `false`.
    #[serde(default)]
    pub airgap_supported: bool,
    /// Highest TIER (lean | standard | rich) this target can usefully run
    /// in air-gapped mode. None on targets where airgap_supported is false.
    #[serde(default)]
    pub airgap_max_tier: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Capabilities {
    #[serde(default)]
    pub local_llm: bool,
    #[serde(default)]
    pub voice_wake_word: bool,
    #[serde(default)]
    pub voice_tts: bool,
    #[serde(default)]
    pub clamav_daemon: bool,
    #[serde(default)]
    pub channels_heavy: Vec<String>,
    #[serde(default)]
    pub channels_disabled_by_ram: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Limitation {
    pub id: String,
    pub reason: String,
    pub unblock: String,
}

/// Load limits from the configured path (env-overridable).
pub fn load() -> Result<Limits> {
    let path = paths::limits_file();
    load_from(&path)
}

/// Load from an explicit path. Used by tests.
pub fn load_from(path: &Path) -> Result<Limits> {
    let raw = std::fs::read_to_string(path)
        .with_context(|| format!("reading limits from {}", path.display()))?;
    let parsed: Limits = serde_json::from_str(&raw)
        .with_context(|| format!("parsing limits at {}", path.display()))?;
    Ok(parsed)
}

/// Format `limits` as the human-readable spec block from
/// `docs/14-limitations.md`. Pure function (no I/O) so tests can exercise
/// the formatter.
pub fn format_human(limits: &Limits) -> String {
    let mut s = String::new();
    s.push_str(&format!(
        "Device:   {} ({} MB RAM, {} storage)\n",
        limits.target, limits.ram_mb, limits.storage_class
    ));
    s.push_str(&format!("Tier:     {}\n", limits.tier));
    s.push('\n');

    s.push_str("What this device CAN do:\n");
    if limits.capabilities.local_llm {
        s.push_str("  ✓ Run a local LLM\n");
    }
    if limits.capabilities.voice_wake_word {
        s.push_str("  ✓ Voice wake-word\n");
    }
    if limits.capabilities.voice_tts {
        s.push_str("  ✓ Voice TTS\n");
    }
    if limits.capabilities.clamav_daemon {
        s.push_str("  ✓ Run ClamAV daemon (clamd) persistently\n");
    } else {
        s.push_str("  ✓ On-demand virus scan via Magika + clamscan\n");
    }
    if !limits.capabilities.channels_heavy.is_empty() {
        s.push_str(&format!(
            "  ✓ Heavy channels available: {}\n",
            limits.capabilities.channels_heavy.join(", ")
        ));
    }
    s.push('\n');

    s.push_str("What this device CANNOT do (and why):\n");
    if limits.limitations.is_empty() {
        s.push_str("  (none recorded)\n");
    } else {
        for lim in &limits.limitations {
            s.push_str(&format!("  ✗ {} ({})\n", lim.id, &lim.reason));
            s.push_str(&format!("       Unblock: {}\n", lim.unblock));
        }
    }
    if !limits.capabilities.channels_disabled_by_ram.is_empty() {
        s.push_str(&format!(
            "\n  Channels disabled by RAM tier: {}\n",
            limits.capabilities.channels_disabled_by_ram.join(", ")
        ));
    }
    s.push('\n');

    // Airgap surface (M4.5).
    if limits.airgap_supported {
        let max = limits.airgap_max_tier.as_deref().unwrap_or("standard");
        s.push_str(&format!(
            "Airgap support: yes (max tier: {max})\n  build with: make build TARGET={tgt} TIER={max} AIRGAP=1\n",
            max = max,
            tgt = limits.target,
        ));
    } else {
        s.push_str("Airgap support: no\n");
    }
    s.push('\n');

    // Mounts surface (M3.5). Read the live policy file; unconditional —
    // even an empty policy is informative.
    if let Ok(entries) = crate::mounts::list(None) {
        s.push_str("Connected drives + shares:\n");
        if entries.is_empty() {
            s.push_str("  (none configured — see `deputyctl mounts add --help`)\n");
        } else {
            for e in entries {
                s.push_str(&format!(
                    "  ✓ [{}] {} → {} ({})\n",
                    e.kind, e.id, e.guest_path, e.mode
                ));
            }
        }
        s.push('\n');
    }

    // Network egress posture (M4.5 / M5.5).
    if let Ok(pol) = crate::network::read(None) {
        s.push_str(&format!("Network egress: {}\n", pol.mode));
        if matches!(pol.mode, crate::network::Mode::Whitelist) && !pol.allow_hosts.is_empty() {
            s.push_str("  Allow-list:\n");
            for h in &pol.allow_hosts {
                s.push_str(&format!("    - {h}\n"));
            }
        }
        s.push('\n');
    }

    s.push_str("Currently active warnings:\n  (none)\n");
    s.push('\n');
    s.push_str("Run `deputyctl doctor` for live pressure metrics.\n");
    s
}
