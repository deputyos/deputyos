//! Wizard state JSON model + on-disk persistence.
//!
//! State is the user's progress through the wizard: which step they're on,
//! and what answers they've supplied so far. Provider keys are NOT stored
//! here — they're written straight to `/etc/deputyos/secrets.env` (or the
//! dev-out equivalent) on submission, then dropped from memory.
//!
//! The state file is updated atomically (write to `<path>.tmp`, then rename)
//! so a crash never leaves a half-written file. mode 0600 to match the rest
//! of the deputyOS config surface.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

/// Schema version so future wizard revisions can detect on-disk state from
/// older binaries and migrate or reject.
///
/// Phase 5 Lane W bumped this from 1 → 2 when the Tailscale, Cloudflare
/// Tunnel, and backup-bucket steps were added. Older state files
/// (`version: 1`) still deserialize cleanly because the new `Answers` fields
/// are all `#[serde(default)]`; we just rewrite them as v2 on next save.
///
/// M8 bumped this from 2 → 3 when the Account step was inserted (between
/// Tailscale and Cloudflare Tunnel). Older v2 state files still deserialize
/// cleanly — the new `Answers.account_*` fields are `#[serde(default)]` — and
/// are rewritten as v3 on next save.
///
/// M3.5 bumped this from 3 → 4 when the Drives step was inserted (between
/// Backup and Review). Older v3 state files still deserialize cleanly — the
/// new `Answers.drives_acknowledged` field is `#[serde(default)]` — and are
/// rewritten as v4 on next save.
pub const STATE_VERSION: u32 = 4;

/// Total number of user-facing wizard steps shown in the progress bar.
/// Bumped 9 → 10 with egress (M5.5), 10 → 11 with the Account step (M8),
/// 11 → 12 with the Drives step (M3.5).
pub const TOTAL_STEPS: usize = 12;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Step {
    System,
    Profile,
    Provider,
    Channels,
    Egress,
    Ssh,
    Tailscale,
    Account,
    CloudflareTunnel,
    Backup,
    Drives,
    Review,
    Done,
}

impl Step {
    pub fn slug(&self) -> &'static str {
        match self {
            Step::System => "system",
            Step::Profile => "profile",
            Step::Provider => "provider",
            Step::Channels => "channels",
            Step::Egress => "egress",
            Step::Ssh => "ssh",
            Step::Tailscale => "tailscale",
            Step::Account => "account",
            Step::CloudflareTunnel => "cloudflare-tunnel",
            Step::Backup => "backup",
            Step::Drives => "drives",
            Step::Review => "review",
            Step::Done => "done",
        }
    }

    pub fn index(&self) -> usize {
        match self {
            Step::System => 1,
            Step::Profile => 2,
            Step::Provider => 3,
            Step::Channels => 4,
            Step::Egress => 5,
            Step::Ssh => 6,
            Step::Tailscale => 7,
            Step::Account => 8,
            Step::CloudflareTunnel => 9,
            Step::Backup => 10,
            Step::Drives => 11,
            Step::Review => 12,
            Step::Done => 12,
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Answers {
    pub hostname: Option<String>,
    pub timezone: Option<String>,
    pub profile: Option<String>,
    pub provider: Option<String>,
    pub channels: Vec<String>,
    pub ssh_keys: Vec<String>,
    /// Whether the user enabled Tailscale at step 6. The auth key itself is
    /// pending in memory until apply (see `PendingTailscale`).
    pub tailscale_enabled: bool,
    /// Cloudflare Tunnel choice from step 7: "skip" | "quick" | "named".
    pub cloudflare_tunnel_choice: Option<String>,
    /// For named tunnel only: the tunnel name extracted from credentials.
    pub cloudflare_tunnel_name: Option<String>,
    /// Backup destination kind chosen at step 8: "skip" | "b2" | "r2" | "s3".
    pub backup_kind: Option<String>,
    /// Non-secret fields associated with the backup choice — bucket name,
    /// account id, endpoint url. Secrets live in `pending_backup` in memory.
    #[serde(default)]
    pub backup_meta: std::collections::BTreeMap<String, String>,
    /// Network egress mode from step 5: "open" | "whitelist" | "airgap".
    #[serde(default)]
    pub egress_mode: Option<String>,
    /// Account email from the Account step (M8). Never stores tokens — the
    /// tunnel/backup tokens minted at registration go straight to
    /// `/etc/deputyos/{tunnel,backup}-token` (0600) and are dropped from memory.
    #[serde(default)]
    pub account_email: Option<String>,
    /// Custom/self-hosted backend API base URL chosen at the Account step. When
    /// set, the wizard registers + polls against this backend instead of the
    /// production `https://api.deputyos.com`, and on success it is persisted
    /// to `/etc/deputyos/api-base` (0644) so the tunnel + command poller pick it
    /// up (see `deputyctl::apibase`). Empty/None = use the default backend.
    #[serde(default)]
    pub account_api_base: Option<String>,
    /// Whether the device was registered against an deputyOS account (M8).
    /// Drives the "integrated tunnel" recommendation on the next step.
    #[serde(default)]
    pub account_registered: bool,
    /// Whether the user acknowledged the Drives step (M3.5). The step is a
    /// hybrid: it surfaces detected/configured mounts and links to the
    /// standalone /mounts page for add/revoke; it does not mutate policy in
    /// the step machine (mounts are live-mutable, not a one-shot choice).
    #[serde(default)]
    pub drives_acknowledged: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WizardState {
    pub version: u32,
    pub started_at: String,
    pub completed_at: Option<String>,
    pub step: Step,
    pub answers: Answers,
}

impl Default for WizardState {
    fn default() -> Self {
        Self {
            version: STATE_VERSION,
            started_at: now_rfc3339(),
            completed_at: None,
            step: Step::System,
            answers: Answers::default(),
        }
    }
}

fn now_rfc3339() -> String {
    use time::format_description::well_known::Rfc3339;
    time::OffsetDateTime::now_utc()
        .format(&Rfc3339)
        .unwrap_or_else(|_| "1970-01-01T00:00:00Z".into())
}

/// Resolve the wizard state file path.
///
/// Resolution order:
/// 1. `DEPUTYWIZARD_STATE_FILE` env var.
/// 2. `/var/lib/deputyos/wizard-state.json` if the parent dir exists.
/// 3. `./.wizard-state.json` (dev fallback).
pub fn state_path() -> PathBuf {
    if let Ok(p) = std::env::var("DEPUTYWIZARD_STATE_FILE") {
        return PathBuf::from(p);
    }
    let canonical = PathBuf::from("/var/lib/deputyos/wizard-state.json");
    if let Some(parent) = canonical.parent() {
        if parent.is_dir() {
            return canonical;
        }
    }
    PathBuf::from("./.wizard-state.json")
}

/// Load state from disk, falling back to a fresh state if the file is missing.
pub fn load_or_new(path: &Path) -> Result<WizardState> {
    match std::fs::read_to_string(path) {
        Ok(raw) => {
            let parsed: WizardState = serde_json::from_str(&raw)
                .with_context(|| format!("parsing wizard state at {}", path.display()))?;
            Ok(parsed)
        }
        Err(_) => Ok(WizardState::default()),
    }
}

/// Atomically persist state. Best-effort `0600` mode where supported.
pub fn save(path: &Path, state: &WizardState) -> Result<()> {
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            let _ = std::fs::create_dir_all(parent);
        }
    }
    let tmp = path.with_extension("json.tmp");
    let body = serde_json::to_string_pretty(state)?;
    std::fs::write(&tmp, body)
        .with_context(|| format!("writing wizard state to {}", tmp.display()))?;
    set_mode_0600(&tmp);
    std::fs::rename(&tmp, path)
        .with_context(|| format!("renaming wizard state to {}", path.display()))?;
    Ok(())
}

#[cfg(unix)]
fn set_mode_0600(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    if let Ok(meta) = std::fs::metadata(path) {
        let mut perms = meta.permissions();
        perms.set_mode(0o600);
        let _ = std::fs::set_permissions(path, perms);
    }
}

#[cfg(not(unix))]
fn set_mode_0600(_path: &Path) {}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn step_indices_are_one_based() {
        assert_eq!(Step::System.index(), 1);
        assert_eq!(Step::Review.index(), TOTAL_STEPS);
        assert_eq!(Step::Done.index(), TOTAL_STEPS);
        assert_eq!(Step::Egress.index(), 5);
        assert_eq!(Step::Tailscale.index(), 7);
        assert_eq!(Step::Account.index(), 8);
        assert_eq!(Step::CloudflareTunnel.index(), 9);
        assert_eq!(Step::Backup.index(), 10);
        assert_eq!(Step::Drives.index(), 11);
    }

    #[test]
    fn round_trips_default_state() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state.json");
        let mut s = WizardState::default();
        s.answers.hostname = Some("deputyos-dev".into());
        s.step = Step::Profile;
        save(&path, &s).unwrap();
        let loaded = load_or_new(&path).unwrap();
        assert_eq!(loaded.step, Step::Profile);
        assert_eq!(loaded.answers.hostname.as_deref(), Some("deputyos-dev"));
        assert_eq!(loaded.version, STATE_VERSION);
    }

    #[test]
    fn missing_state_file_yields_default() {
        let s = load_or_new(Path::new("/tmp/deputyos-no-such-state-xyz.json")).unwrap();
        assert_eq!(s.step, Step::System);
        assert!(s.answers.hostname.is_none());
    }
}
