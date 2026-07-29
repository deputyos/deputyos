//! Thin wrapper around `systemctl` and `journalctl`.
//!
//! We deliberately shell out rather than bind libsystemd or DBus: the deputyctl
//! binary needs to run from any user context (system or `--user`), and the
//! command-line tools normalise that. Calls are sync; this is a CLI.

use std::process::{Command, ExitStatus, Stdio};

use anyhow::{Context, Result};

/// Returned by `systemctl is-active <unit>`. The man page enumerates these.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ActiveState {
    Active,
    Inactive,
    Failed,
    Activating,
    Deactivating,
    Reloading,
    Unknown(String),
}

impl ActiveState {
    pub fn as_str(&self) -> &str {
        match self {
            ActiveState::Active => "active",
            ActiveState::Inactive => "inactive",
            ActiveState::Failed => "failed",
            ActiveState::Activating => "activating",
            ActiveState::Deactivating => "deactivating",
            ActiveState::Reloading => "reloading",
            ActiveState::Unknown(s) => s.as_str(),
        }
    }

    fn parse(raw: &str) -> Self {
        match raw.trim() {
            "active" => Self::Active,
            "inactive" => Self::Inactive,
            "failed" => Self::Failed,
            "activating" => Self::Activating,
            "deactivating" => Self::Deactivating,
            "reloading" => Self::Reloading,
            other => Self::Unknown(other.to_string()),
        }
    }
}

/// Returns true on Linux hosts. Every other call below assumes systemd is
/// present; gate on this when running on macOS / WSL builders.
pub fn available() -> bool {
    cfg!(target_os = "linux")
}

/// Run `systemctl is-active <unit>` and parse the output.
///
/// Failure to spawn returns `Err`; a non-zero exit (which is normal for
/// inactive/failed) returns `Ok(ActiveState)` because the stdout is the
/// canonical signal.
pub fn is_active(unit: &str) -> Result<ActiveState> {
    let out = Command::new("systemctl")
        .args(["is-active", unit])
        .output()
        .with_context(|| format!("spawning systemctl is-active {unit}"))?;
    let stdout = String::from_utf8_lossy(&out.stdout);
    Ok(ActiveState::parse(&stdout))
}

/// Returns true when `systemctl is-enabled <unit>` reports `enabled`.
/// Missing/static/disabled units are normal operational states and map false.
pub fn is_enabled(unit: &str) -> bool {
    Command::new("systemctl")
        .args(["is-enabled", unit])
        .output()
        .map(|out| out.status.success() && String::from_utf8_lossy(&out.stdout).trim() == "enabled")
        .unwrap_or(false)
}

/// Run `systemctl <verb> <unit>` and surface its exit status.
pub fn run(verb: &str, unit: &str) -> Result<ExitStatus> {
    tracing::info!(verb, unit, "systemctl");
    Command::new("systemctl")
        .arg(verb)
        .arg(unit)
        .stdin(Stdio::null())
        .status()
        .with_context(|| format!("spawning systemctl {verb} {unit}"))
}

/// Run `systemctl status <unit> --no-pager` inheriting stdio. For `deputyctl status`.
pub fn status_print(unit: &str) -> Result<ExitStatus> {
    Command::new("systemctl")
        .args(["status", unit, "--no-pager"])
        .status()
        .with_context(|| format!("spawning systemctl status {unit}"))
}

/// Stream a unit's journal with stdio inherited. `-f` if `follow`.
pub fn journal(unit: &str, follow: bool) -> Result<ExitStatus> {
    let mut cmd = Command::new("journalctl");
    cmd.args(["-u", unit, "--no-pager"]);
    if follow {
        cmd.arg("-f");
    }
    cmd.status()
        .with_context(|| format!("spawning journalctl -u {unit}"))
}

/// Returns true if `apparmor_parser` is on PATH.
pub fn apparmor_parser_available() -> bool {
    Command::new("apparmor_parser")
        .arg("--help")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Reload an AppArmor profile from disk via `apparmor_parser -r <path>`.
pub fn apparmor_reload(profile_path: &str) -> Result<ExitStatus> {
    Command::new("apparmor_parser")
        .args(["-r", profile_path])
        .status()
        .with_context(|| format!("spawning apparmor_parser -r {profile_path}"))
}
