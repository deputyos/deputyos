//! `deputyctl mounts` — read + mutate `/etc/deputyos/mounts-policy.json`.
//!
//! Materialising the policy (running `mount`, generating systemd units,
//! reloading udev) is the job of `deputyos-mounts.service`. This module is
//! the data layer + CLI surface; it stays platform-clean so `cargo test`
//! works on macOS and Linux dev hosts.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};

use crate::paths;

/// Mount mode advertised to the agent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    /// Read-only — agent can stat + read, never write.
    Ro,
    /// Read-write — agent can write, modify, delete.
    Rw,
}

impl std::str::FromStr for Mode {
    type Err = anyhow::Error;
    fn from_str(s: &str) -> Result<Self> {
        match s.to_ascii_lowercase().as_str() {
            "ro" | "readonly" | "read-only" => Ok(Mode::Ro),
            "rw" | "readwrite" | "read-write" => Ok(Mode::Rw),
            other => bail!("unknown mount mode: {other:?} (expected ro|rw)"),
        }
    }
}

impl std::fmt::Display for Mode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Mode::Ro => f.write_str("ro"),
            Mode::Rw => f.write_str("rw"),
        }
    }
}

/// One host-FS share (WSL2 / virtiofs / DrvFs).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HostFsMount {
    /// Stable id (e.g. `documents`, `code`).
    pub id: String,
    /// Path on the host (informational; the helper materialises this).
    pub host_path: String,
    /// Path inside the appliance, always under `/mnt/deputyos/`.
    pub guest_path: String,
    pub mode: Mode,
}

/// Removable-drive policy (USB, SD).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RemovablePolicy {
    pub enabled: bool,
    pub auto_mount: bool,
    pub default_mode: Mode,
    /// Mount options forced for unknown filesystems.
    pub mount_options_unknown_fs: String,
}

impl Default for RemovablePolicy {
    fn default() -> Self {
        Self {
            enabled: false,
            auto_mount: false,
            default_mode: Mode::Ro,
            mount_options_unknown_fs: "nosuid,nodev,noexec".to_string(),
        }
    }
}

/// One SMB / NFS share (credentials live in /etc/deputyos/secrets.env).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NetworkMount {
    pub id: String,
    /// `cifs` or `nfs`.
    pub kind: String,
    /// e.g. `//nas.lan/photos` (cifs) or `nas.lan:/srv/photos` (nfs).
    pub source: String,
    pub guest_path: String,
    pub mode: Mode,
    /// Reference to the env var holding credentials in secrets.env (cifs).
    pub credentials_env: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Policy {
    #[serde(default = "default_schema", rename = "$schema")]
    pub schema: String,
    #[serde(default = "default_version")]
    pub version: u32,
    #[serde(default)]
    pub host_fs: Vec<HostFsMount>,
    #[serde(default)]
    pub removable: RemovablePolicy,
    #[serde(default)]
    pub network: Vec<NetworkMount>,
}

fn default_schema() -> String {
    "https://www.deputyos.com/schemas/mounts-policy-v1.json".to_string()
}

fn default_version() -> u32 {
    POLICY_SCHEMA_VERSION
}

/// Schema version of the on-disk `mounts-policy.json` (M3.5 Lane D). Bump
/// when the policy shape changes; the release manifest advertises this via
/// `release::Manifest::mounts_policy_schema_version` so consumers can
/// detect a mismatch and migrate.
pub const POLICY_SCHEMA_VERSION: u32 = 1;

impl Default for Policy {
    fn default() -> Self {
        Self {
            schema: default_schema(),
            version: default_version(),
            host_fs: Vec::new(),
            removable: RemovablePolicy::default(),
            network: Vec::new(),
        }
    }
}

pub fn read(path: Option<&Path>) -> Result<Policy> {
    let default = paths::mounts_policy_file();
    let p = path.unwrap_or(default.as_path());
    if !p.exists() {
        return Ok(Policy::default());
    }
    let body = fs::read_to_string(p).with_context(|| format!("reading {}", p.display()))?;
    let policy: Policy =
        serde_json::from_str(&body).with_context(|| format!("parsing JSON in {}", p.display()))?;
    Ok(policy)
}

pub fn write(path: Option<&Path>, policy: &Policy) -> Result<PathBuf> {
    let p = path
        .map(Path::to_path_buf)
        .unwrap_or_else(paths::mounts_policy_file);
    let parent = p.parent().context("mounts-policy path has no parent")?;
    fs::create_dir_all(parent).with_context(|| format!("ensuring {} exists", parent.display()))?;
    let tmp = parent.join(".mounts-policy.json.tmp");
    let body = serde_json::to_string_pretty(policy).context("serialising policy")?;
    fs::write(&tmp, body.as_bytes()).with_context(|| format!("writing {}", tmp.display()))?;
    fs::rename(&tmp, &p).with_context(|| format!("renaming → {}", p.display()))?;
    Ok(p)
}

/// Path validator: any agent-visible path must live under `/mnt/deputyos/`
/// so AppArmor's per-profile rules can confine it.
pub fn validate_guest_path(s: &str) -> Result<()> {
    if !s.starts_with("/mnt/deputyos/") {
        bail!("guest_path must start with /mnt/deputyos/ (got {s:?})");
    }
    if s.contains("..") {
        bail!("guest_path cannot contain '..' segments");
    }
    Ok(())
}

pub fn add_host_fs(
    path: Option<&Path>,
    id: &str,
    host_path: &str,
    guest_path: &str,
    mode: Mode,
) -> Result<Policy> {
    validate_guest_path(guest_path)?;
    let mut policy = read(path)?;
    if policy.host_fs.iter().any(|m| m.id == id) {
        bail!("a host-fs mount with id {id:?} already exists; remove first");
    }
    policy.host_fs.push(HostFsMount {
        id: id.to_string(),
        host_path: host_path.to_string(),
        guest_path: guest_path.to_string(),
        mode,
    });
    write(path, &policy)?;
    Ok(policy)
}

pub fn remove_by_id(path: Option<&Path>, id: &str) -> Result<Policy> {
    let mut policy = read(path)?;
    let before = policy.host_fs.len() + policy.network.len();
    policy.host_fs.retain(|m| m.id != id);
    policy.network.retain(|m| m.id != id);
    let after = policy.host_fs.len() + policy.network.len();
    if before == after {
        bail!("no mount found with id {id:?}");
    }
    write(path, &policy)?;
    Ok(policy)
}

/// Add a network share (CIFS or NFS) to the mounts policy.
pub fn add_network_mount(
    path: Option<&Path>,
    id: &str,
    kind: &str,
    source: &str,
    guest_path: &str,
    mode: Mode,
    credentials_env: Option<&str>,
) -> Result<Policy> {
    validate_guest_path(guest_path)?;
    if kind != "cifs" && kind != "nfs" {
        bail!("kind must be 'cifs' or 'nfs', got {kind:?}");
    }
    if source.trim().is_empty() {
        bail!("source is required (e.g. //nas/photos or nas:/srv/photos)");
    }
    let mut policy = read(path)?;
    if policy.host_fs.iter().any(|m| m.id == id) || policy.network.iter().any(|m| m.id == id) {
        bail!("a mount with id {id:?} already exists; remove it first");
    }
    policy.network.push(NetworkMount {
        id: id.to_string(),
        kind: kind.to_string(),
        source: source.to_string(),
        guest_path: guest_path.to_string(),
        mode,
        credentials_env: credentials_env.map(String::from),
    });
    write(path, &policy)?;
    Ok(policy)
}

/// Trigger the mount materialiser service to apply the current policy.
/// Returns the number of mounts in the current policy.
pub fn apply_mounts() -> Result<usize> {
    let policy = read(None)?;
    let count = policy.host_fs.len() + policy.network.len();
    if paths::dev_out_dir().is_some() {
        eprintln!(
            "note: DEPUTYOS_DEV_OUT set — skipping systemctl restart of \
             deputyos-mounts.service (dev mode; policy saved)"
        );
        return Ok(count);
    }
    let status = std::process::Command::new("systemctl")
        .args(["restart", "deputyos-mounts.service"])
        .status();
    match status {
        Ok(s) if s.success() => {
            if count == 0 {
                eprintln!("note: no mounts configured; service restarted (no-op)");
            }
        }
        Ok(s) => {
            eprintln!(
                "warn: systemctl restart exited with {} — mounts may not be applied",
                s.code().unwrap_or(-1)
            );
        }
        Err(e) => {
            eprintln!("warn: systemctl not available ({e}) — mounts policy saved but not applied");
        }
    }
    Ok(count)
}

/// Run a health check on all configured mounts.
#[derive(Debug, Serialize)]
pub struct MountHealth {
    pub id: String,
    pub kind: String,
    pub guest_path: String,
    pub status: String,
    pub detail: Option<String>,
}

pub fn health_check(path: Option<&Path>) -> Result<Vec<MountHealth>> {
    let policy = read(path)?;
    let mut results = Vec::new();

    for m in &policy.host_fs {
        let guest = std::path::Path::new(&m.guest_path);
        let status = if guest.is_dir() {
            // Check if there's actually something mounted at this path.
            let output = std::process::Command::new("mountpoint")
                .arg("-q")
                .arg(guest)
                .status();
            match output {
                Ok(s) if s.success() => "mounted".to_string(),
                _ => "not-mounted".to_string(),
            }
        } else {
            "missing".to_string()
        };
        results.push(MountHealth {
            id: m.id.clone(),
            kind: "host-fs".into(),
            guest_path: m.guest_path.clone(),
            status,
            detail: Some(format!("source: {}", m.host_path)),
        });
    }

    for m in &policy.network {
        let guest = std::path::Path::new(&m.guest_path);
        let status = if guest.is_dir() {
            let output = std::process::Command::new("mountpoint")
                .arg("-q")
                .arg(guest)
                .status();
            match output {
                Ok(s) if s.success() => "mounted".to_string(),
                _ => "not-mounted".to_string(),
            }
        } else {
            "missing".to_string()
        };
        results.push(MountHealth {
            id: m.id.clone(),
            kind: format!("network/{}", m.kind),
            guest_path: m.guest_path.clone(),
            status,
            detail: Some(format!("source: {} ({}:{})", m.source, m.kind, m.mode)),
        });
    }

    Ok(results)
}

/// Summary form rendered by `deputyctl mounts list`.
#[derive(Debug, Clone, Serialize)]
pub struct ListEntry {
    pub kind: String,
    pub id: String,
    pub guest_path: String,
    pub mode: String,
    pub source: String,
}

pub fn list(path: Option<&Path>) -> Result<Vec<ListEntry>> {
    let policy = read(path)?;
    let mut out = Vec::new();
    for m in policy.host_fs {
        out.push(ListEntry {
            kind: "host-fs".to_string(),
            id: m.id,
            guest_path: m.guest_path,
            mode: m.mode.to_string(),
            source: m.host_path,
        });
    }
    for m in policy.network {
        out.push(ListEntry {
            kind: format!("network/{}", m.kind),
            id: m.id,
            guest_path: m.guest_path,
            mode: m.mode.to_string(),
            source: m.source,
        });
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn temp_policy_path() -> (TempDir, PathBuf) {
        let dir = TempDir::new().expect("tempdir");
        let path = dir.path().join("mounts-policy.json");
        (dir, path)
    }

    #[test]
    fn read_missing_returns_default_empty() {
        let (_d, p) = temp_policy_path();
        let pol = read(Some(&p)).expect("read");
        assert!(pol.host_fs.is_empty());
        assert!(pol.network.is_empty());
        assert!(!pol.removable.enabled);
    }

    #[test]
    fn add_host_fs_round_trip() {
        let (_d, p) = temp_policy_path();
        add_host_fs(
            Some(&p),
            "docs",
            "/home/me/Documents",
            "/mnt/deputyos/docs",
            Mode::Rw,
        )
        .expect("add");
        let pol = read(Some(&p)).expect("read");
        assert_eq!(pol.host_fs.len(), 1);
        assert_eq!(pol.host_fs[0].id, "docs");
        assert_eq!(pol.host_fs[0].mode, Mode::Rw);
    }

    #[test]
    fn add_rejects_outside_mnt_deputyos() {
        let (_d, p) = temp_policy_path();
        let err = add_host_fs(Some(&p), "x", "/home/me", "/etc/passwd", Mode::Ro)
            .expect_err("must reject");
        assert!(err.to_string().contains("/mnt/deputyos/"));
    }

    #[test]
    fn add_rejects_dotdot() {
        let (_d, p) = temp_policy_path();
        let err = add_host_fs(Some(&p), "x", "/home/me", "/mnt/deputyos/../etc", Mode::Ro)
            .expect_err("must reject");
        assert!(err.to_string().contains(".."));
    }

    #[test]
    fn add_rejects_duplicate_id() {
        let (_d, p) = temp_policy_path();
        add_host_fs(Some(&p), "docs", "/h", "/mnt/deputyos/docs", Mode::Ro).expect("add");
        let err = add_host_fs(Some(&p), "docs", "/h2", "/mnt/deputyos/docs2", Mode::Ro)
            .expect_err("must reject");
        assert!(err.to_string().contains("already exists"));
    }

    #[test]
    fn remove_by_id_works() {
        let (_d, p) = temp_policy_path();
        add_host_fs(Some(&p), "docs", "/h", "/mnt/deputyos/docs", Mode::Rw).expect("add");
        remove_by_id(Some(&p), "docs").expect("remove");
        let pol = read(Some(&p)).expect("read");
        assert!(pol.host_fs.is_empty());
    }

    #[test]
    fn remove_unknown_id_errors() {
        let (_d, p) = temp_policy_path();
        let err = remove_by_id(Some(&p), "nope").expect_err("must error");
        assert!(err.to_string().contains("nope"));
    }

    #[test]
    fn mode_parse_round_trip() {
        for m in [Mode::Ro, Mode::Rw] {
            assert_eq!(m.to_string().parse::<Mode>().expect("parse"), m);
        }
        assert!("invalid".parse::<Mode>().is_err());
    }

    #[test]
    fn env_override_routes_read_write_to_deputyos_mounts_policy() {
        let _g = crate::env_mutex().lock().expect("env mutex");
        let dir = TempDir::new().expect("tempdir");
        let path = dir.path().join("mounts-policy.json");
        std::env::set_var("DEPUTYOS_MOUNTS_POLICY", &path);

        // read(None) follows the env override; absent file → default empty policy.
        let pol = read(None).expect("read via env override");
        assert!(pol.host_fs.is_empty());

        // write(None) (via add_host_fs) lands at the env-overridden path.
        add_host_fs(
            None,
            "docs",
            "/home/me/Documents",
            "/mnt/deputyos/docs",
            Mode::Ro,
        )
        .expect("add via env override");
        assert!(path.exists(), "policy written at $DEPUTYOS_MOUNTS_POLICY");
        let pol = read(None).expect("read back via env override");
        assert_eq!(pol.host_fs.len(), 1);
        assert_eq!(pol.host_fs[0].id, "docs");

        std::env::remove_var("DEPUTYOS_MOUNTS_POLICY");
    }

    #[test]
    fn apply_mounts_is_noop_in_dev_out() {
        let _g = crate::env_mutex().lock().expect("env mutex");
        let dir = TempDir::new().expect("tempdir");
        std::env::set_var(
            "DEPUTYOS_MOUNTS_POLICY",
            dir.path().join("mounts-policy.json"),
        );
        std::env::set_var("DEPUTYOS_DEV_OUT", dir.path());

        // In dev, apply_mounts must short-circuit before `systemctl restart`
        // and return the configured-mount count without erroring.
        let count = apply_mounts().expect("apply is a dev no-op");
        assert_eq!(count, 0, "empty temp policy → 0 mounts, no systemctl call");

        std::env::remove_var("DEPUTYOS_MOUNTS_POLICY");
        std::env::remove_var("DEPUTYOS_DEV_OUT");
    }
}
