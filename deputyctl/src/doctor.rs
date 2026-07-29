//! `deputyctl doctor` — verify every default-on control from `docs/09-security.md`.
//!
//! Each [`Check`] is independent and total: it returns a [`CheckOutcome`]
//! rather than panicking, so a fresh Debian box with nothing installed
//! produces a well-formed report (mostly Fails) and exits non-zero.
//!
//! Adding a check: append to [`all_checks`]. The runner runs every check
//! (no short-circuit) so the user sees the full picture in one pass.

use std::collections::BTreeMap;
use std::path::Path;
use std::process::Command;
use std::time::Duration;

use anyhow::Result;
use serde::Serialize;

use crate::profile;
use crate::systemd;

use crate::network;

/// Outcome of a single check.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(tag = "kind", content = "detail")]
pub enum CheckOutcome {
    Pass,
    Warn(String),
    Fail(String),
    Skip(String),
}

impl CheckOutcome {
    pub fn label(&self) -> &'static str {
        match self {
            CheckOutcome::Pass => "PASS",
            CheckOutcome::Warn(_) => "WARN",
            CheckOutcome::Fail(_) => "FAIL",
            CheckOutcome::Skip(_) => "SKIP",
        }
    }

    pub fn detail(&self) -> &str {
        match self {
            CheckOutcome::Pass => "",
            CheckOutcome::Warn(s) | CheckOutcome::Fail(s) | CheckOutcome::Skip(s) => s.as_str(),
        }
    }

    pub fn is_fail(&self) -> bool {
        matches!(self, CheckOutcome::Fail(_))
    }
}

/// One check + its remediation hint.
pub struct Check {
    pub name: &'static str,
    /// Short fix hint, surfaced on Warn/Fail. References `deputyctl limits`
    /// for device-tier-related limitations per docs/14-limitations.md.
    pub fix: &'static str,
    pub run: fn() -> Result<CheckOutcome>,
}

/// Result of running one check.
#[derive(Debug, Clone, Serialize)]
pub struct CheckReport {
    pub name: &'static str,
    pub outcome: CheckOutcome,
    pub fix: &'static str,
}

/// Top-level structured report (used by `--json`).
#[derive(Debug, Clone, Serialize)]
pub struct DoctorReport {
    pub linux: bool,
    pub checks: Vec<CheckReport>,
    pub fails: usize,
    pub warns: usize,
    pub passes: usize,
    pub skips: usize,
}

/// All checks, in display order.
pub fn all_checks() -> Vec<Check> {
    vec![
        Check {
            name: "active-profile-manifest",
            fix: "ensure /etc/deputyos/active-profile names an installed profile",
            run: check_active_profile_manifest,
        },
        Check {
            name: "apparmor-enforcing",
            fix: "enable AppArmor and load the deputyos.<profile> policy in enforce mode",
            run: check_apparmor,
        },
        Check {
            name: "ufw-default-deny",
            fix: "sudo ufw default deny incoming && sudo ufw enable",
            run: check_ufw,
        },
        Check {
            name: "fail2ban-running",
            fix: "sudo systemctl enable --now fail2ban",
            run: check_fail2ban,
        },
        Check {
            name: "clamav-healthy",
            fix: "sudo systemctl enable --now clamav-daemon (or see `deputyctl limits` if RAM-tier disables clamd)",
            run: check_clamav,
        },
        Check {
            name: "magika-present",
            fix: "install magika; baked into image at /opt/deputyos by Lane B",
            run: check_magika,
        },
        Check {
            name: "ssh-key-only",
            fix: "set PasswordAuthentication no and PermitRootLogin no in /etc/ssh/sshd_config",
            run: check_ssh,
        },
        Check {
            name: "hardened-sysctls",
            fix: "see /etc/sysctl.d/90-deputyos.conf in docs/09-security.md",
            run: check_sysctls,
        },
        Check {
            name: "zram-swap",
            fix: "no zram device — see `deputyctl limits` for your device tier",
            run: check_zram,
        },
        Check {
            name: "active-profile-unit",
            fix: "deputyctl up — or check `systemctl status <unit>`",
            run: check_active_unit,
        },
        Check {
            name: "profile-health-endpoint",
            fix: "deputyctl up; if persistent, check `deputyctl logs`",
            run: check_health_endpoint,
        },
        Check {
            name: "data-partition-free",
            fix: "free space on /home/agent (or run deputyctl backup + prune)",
            run: check_disk_free,
        },
        Check {
            name: "time-sync",
            fix: "sudo timedatectl set-ntp true",
            run: check_time_sync,
        },
        Check {
            name: "mounts-health",
            fix: "deputyctl mounts list — confirm each guest path resolves; for SMB/NFS check secrets.env",
            run: check_mounts_health,
        },
        Check {
            name: "network-policy",
            fix: "deputyctl network apply — re-render nftables from /etc/deputyos/network-policy.json (mode drift)",
            run: check_network_policy,
        },
    ]
}

/// Run every check and aggregate. Linux-only checks return `Skip` on others.
pub fn run_all() -> DoctorReport {
    let linux = cfg!(target_os = "linux");
    let mut reports = Vec::new();
    let mut fails = 0;
    let mut warns = 0;
    let mut passes = 0;
    let mut skips = 0;
    for c in all_checks() {
        tracing::debug!(check = c.name, "running");
        let outcome = match (c.run)() {
            Ok(o) => o,
            Err(e) => CheckOutcome::Fail(format!("internal error: {e}")),
        };
        match &outcome {
            CheckOutcome::Pass => passes += 1,
            CheckOutcome::Warn(_) => warns += 1,
            CheckOutcome::Fail(_) => fails += 1,
            CheckOutcome::Skip(_) => skips += 1,
        }
        reports.push(CheckReport {
            name: c.name,
            outcome,
            fix: c.fix,
        });
    }
    DoctorReport {
        linux,
        checks: reports,
        fails,
        warns,
        passes,
        skips,
    }
}

// ---------------------------------------------------------------------------
// Individual checks
// ---------------------------------------------------------------------------

fn linux_only<F: FnOnce() -> Result<CheckOutcome>>(f: F) -> Result<CheckOutcome> {
    if !cfg!(target_os = "linux") {
        return Ok(CheckOutcome::Skip("non-Linux host".into()));
    }
    f()
}

fn check_active_profile_manifest() -> Result<CheckOutcome> {
    match profile::load_active() {
        Ok(_) => Ok(CheckOutcome::Pass),
        // Tolerate the "missing active-profile file" case as a Fail
        // because doctor must run on a fresh box without panicking.
        Err(e) => Ok(CheckOutcome::Fail(format!("{e}"))),
    }
}

fn check_apparmor() -> Result<CheckOutcome> {
    linux_only(|| {
        let path = Path::new("/sys/kernel/security/apparmor/profiles");
        if !path.exists() {
            return Ok(CheckOutcome::Fail(
                "AppArmor securityfs missing — kernel module not loaded".into(),
            ));
        }
        let body = match std::fs::read_to_string(path) {
            Ok(s) => s,
            Err(e) => return Ok(CheckOutcome::Fail(format!("read apparmor profiles: {e}"))),
        };
        // Determine which profile name to look for (active-profile-aware).
        let want_id = crate::paths::read_active_profile_id().unwrap_or_else(|| "openclaw".into());
        let want_profile = format!("deputyos.{want_id}");
        let mut found_enforce = false;
        for line in body.lines() {
            // Format: "<profile-name> (mode)"
            if let Some((name, rest)) = line.rsplit_once(' ') {
                if name == want_profile && rest.contains("enforce") {
                    found_enforce = true;
                    break;
                }
            }
        }
        if found_enforce {
            Ok(CheckOutcome::Pass)
        } else {
            Ok(CheckOutcome::Fail(format!(
                "{want_profile} not loaded in enforce mode"
            )))
        }
    })
}

fn check_ufw() -> Result<CheckOutcome> {
    linux_only(|| {
        let out = match Command::new("ufw").args(["status", "verbose"]).output() {
            Ok(o) => o,
            Err(e) => return Ok(CheckOutcome::Fail(format!("ufw not available: {e}"))),
        };
        let text = String::from_utf8_lossy(&out.stdout);
        let active = text.lines().any(|l| l.trim() == "Status: active");
        let default_deny = text.lines().any(|l| l.contains("deny (incoming)"));
        if active && default_deny {
            Ok(CheckOutcome::Pass)
        } else if !active {
            Ok(CheckOutcome::Fail("ufw not active".into()))
        } else {
            Ok(CheckOutcome::Fail(
                "ufw default policy is not deny incoming".into(),
            ))
        }
    })
}

fn check_fail2ban() -> Result<CheckOutcome> {
    linux_only(|| {
        let state = systemd::is_active("fail2ban")?;
        if state == systemd::ActiveState::Active {
            Ok(CheckOutcome::Pass)
        } else {
            Ok(CheckOutcome::Fail(format!(
                "fail2ban systemd state: {}",
                state.as_str()
            )))
        }
    })
}

fn check_clamav() -> Result<CheckOutcome> {
    linux_only(|| {
        // RAM-tier gate: if device is below ~2GB RAM, clamd is not expected.
        if let Some(total_kb) = read_meminfo_total_kb() {
            if total_kb < 2 * 1024 * 1024 {
                return Ok(CheckOutcome::Skip(
                    "RAM < 2GB; clamd disabled by tier — see `deputyctl limits`".into(),
                ));
            }
        }
        let state = systemd::is_active("clamav-daemon")?;
        if state != systemd::ActiveState::Active {
            return Ok(CheckOutcome::Fail(format!(
                "clamav-daemon state: {}",
                state.as_str()
            )));
        }
        let v = Command::new("clamdscan").arg("--version").output();
        match v {
            Ok(o) if o.status.success() => Ok(CheckOutcome::Pass),
            Ok(o) => Ok(CheckOutcome::Fail(format!(
                "clamdscan exit {:?}",
                o.status.code()
            ))),
            Err(e) => Ok(CheckOutcome::Fail(format!("clamdscan missing: {e}"))),
        }
    })
}

fn check_magika() -> Result<CheckOutcome> {
    linux_only(|| {
        match Command::new("sh")
            .args(["-c", "command -v magika"])
            .status()
        {
            Ok(s) if s.success() => Ok(CheckOutcome::Pass),
            Ok(_) => Ok(CheckOutcome::Fail("magika not in PATH".into())),
            Err(e) => Ok(CheckOutcome::Fail(format!("spawn sh: {e}"))),
        }
    })
}

fn check_ssh() -> Result<CheckOutcome> {
    linux_only(|| {
        let mut password_no = false;
        let mut root_ok = false;
        let mut sources: Vec<String> = Vec::new();
        if let Ok(s) = std::fs::read_to_string("/etc/ssh/sshd_config") {
            sources.push(s);
        }
        if let Ok(rd) = std::fs::read_dir("/etc/ssh/sshd_config.d") {
            for e in rd.flatten() {
                if let Ok(s) = std::fs::read_to_string(e.path()) {
                    sources.push(s);
                }
            }
        }
        if sources.is_empty() {
            return Ok(CheckOutcome::Fail("no sshd_config readable".into()));
        }
        for src in &sources {
            for line in src.lines() {
                let l = line.trim();
                if l.starts_with('#') || l.is_empty() {
                    continue;
                }
                let lower = l.to_ascii_lowercase();
                if let Some(v) = lower.strip_prefix("passwordauthentication") {
                    if v.trim() == "no" {
                        password_no = true;
                    }
                }
                if let Some(v) = lower.strip_prefix("permitrootlogin") {
                    let v = v.trim();
                    if v == "no" || v == "prohibit-password" {
                        root_ok = true;
                    }
                }
            }
        }
        if password_no && root_ok {
            Ok(CheckOutcome::Pass)
        } else if !password_no {
            Ok(CheckOutcome::Fail(
                "PasswordAuthentication is not 'no'".into(),
            ))
        } else {
            Ok(CheckOutcome::Fail(
                "PermitRootLogin must be 'no' or 'prohibit-password'".into(),
            ))
        }
    })
}

/// Parse a `/proc/sys/...` integer file. Public so tests can exercise it
/// against fixtures without scaffolding `/proc`.
pub fn parse_sysctl_int(raw: &str) -> Option<i64> {
    raw.split_ascii_whitespace().next()?.parse::<i64>().ok()
}

fn read_sysctl(path: &str) -> Option<i64> {
    let raw = std::fs::read_to_string(path).ok()?;
    parse_sysctl_int(&raw)
}

/// Predicate type for sysctl checks: factored out to satisfy clippy's
/// `type_complexity` lint.
type SysctlSpec = (&'static str, fn(i64) -> bool, &'static str);

fn check_sysctls() -> Result<CheckOutcome> {
    linux_only(|| {
        // (path, predicate, human label)
        let want: &[SysctlSpec] = &[
            (
                "/proc/sys/net/ipv4/tcp_syncookies",
                |v| v == 1,
                "tcp_syncookies=1",
            ),
            (
                "/proc/sys/kernel/kptr_restrict",
                |v| v >= 1,
                "kptr_restrict>=1",
            ),
            (
                "/proc/sys/kernel/dmesg_restrict",
                |v| v == 1,
                "dmesg_restrict=1",
            ),
            (
                "/proc/sys/net/ipv4/conf/all/rp_filter",
                |v| v == 1,
                "rp_filter=1",
            ),
        ];
        let mut bad: Vec<String> = Vec::new();
        for (p, pred, label) in want {
            match read_sysctl(p) {
                Some(v) if pred(v) => {}
                Some(v) => bad.push(format!("{label} (got {v})")),
                None => bad.push(format!("{label} (unreadable)")),
            }
        }
        if bad.is_empty() {
            Ok(CheckOutcome::Pass)
        } else {
            Ok(CheckOutcome::Warn(bad.join(", ")))
        }
    })
}

fn check_zram() -> Result<CheckOutcome> {
    linux_only(|| {
        let raw = match std::fs::read_to_string("/proc/swaps") {
            Ok(s) => s,
            Err(e) => return Ok(CheckOutcome::Fail(format!("read /proc/swaps: {e}"))),
        };
        let has_zram = raw.lines().any(|l| l.starts_with("/dev/zram"));
        if has_zram {
            Ok(CheckOutcome::Pass)
        } else {
            Ok(CheckOutcome::Fail(
                "no zram swap device (see `deputyctl limits` for your tier)".into(),
            ))
        }
    })
}

fn check_active_unit() -> Result<CheckOutcome> {
    linux_only(|| {
        let (_id, m) = match profile::load_active() {
            Ok(x) => x,
            Err(e) => return Ok(CheckOutcome::Fail(format!("no active profile: {e}"))),
        };
        let state = systemd::is_active(&m.service.unit)?;
        if state == systemd::ActiveState::Active {
            Ok(CheckOutcome::Pass)
        } else {
            Ok(CheckOutcome::Fail(format!(
                "{} state: {}",
                m.service.unit,
                state.as_str()
            )))
        }
    })
}

fn check_health_endpoint() -> Result<CheckOutcome> {
    linux_only(|| {
        let url = match profile::load_active() {
            Ok((_, m)) => m.health.http_check.clone(),
            Err(e) => return Ok(CheckOutcome::Fail(format!("no active profile: {e}"))),
        };
        if url.is_empty() {
            return Ok(CheckOutcome::Skip("no http_check configured".into()));
        }
        let agent = ureq::AgentBuilder::new()
            .timeout_connect(Duration::from_secs(2))
            .timeout_read(Duration::from_secs(2))
            .timeout_write(Duration::from_secs(2))
            .build();
        match agent.get(&url).call() {
            Ok(resp) if resp.status() == 200 => Ok(CheckOutcome::Pass),
            Ok(resp) => Ok(CheckOutcome::Fail(format!(
                "{url} returned HTTP {}",
                resp.status()
            ))),
            Err(e) => Ok(CheckOutcome::Fail(format!("{url}: {e}"))),
        }
    })
}

fn check_disk_free() -> Result<CheckOutcome> {
    linux_only(|| {
        // Try /home/agent first, fall back to /home, then /.
        let candidates = ["/home/agent", "/home", "/"];
        for path in candidates {
            if !Path::new(path).exists() {
                continue;
            }
            match free_bytes(path) {
                Ok(free) => {
                    let mb = free / (1024 * 1024);
                    return if mb < 50 {
                        Ok(CheckOutcome::Fail(format!("{path}: {mb} MB free")))
                    } else if mb < 500 {
                        Ok(CheckOutcome::Warn(format!("{path}: {mb} MB free")))
                    } else {
                        Ok(CheckOutcome::Pass)
                    };
                }
                Err(e) => return Ok(CheckOutcome::Fail(format!("statvfs {path}: {e}"))),
            }
        }
        Ok(CheckOutcome::Fail("no candidate path exists".into()))
    })
}

#[cfg(unix)]
fn free_bytes(path: &str) -> Result<u64> {
    use rustix::fs::statvfs;
    let st = statvfs(path)?;
    // f_bavail is blocks available to non-root in fragments; f_frsize is
    // fragment size in bytes. Multiplying gives bytes free.
    Ok(st.f_bavail as u64 * st.f_frsize as u64)
}

#[cfg(not(unix))]
fn free_bytes(_path: &str) -> Result<u64> {
    anyhow::bail!("statvfs unsupported on this platform")
}

fn check_time_sync() -> Result<CheckOutcome> {
    linux_only(|| {
        let out = match Command::new("timedatectl")
            .args(["show", "-p", "NTPSynchronized", "--value"])
            .output()
        {
            Ok(o) => o,
            Err(e) => return Ok(CheckOutcome::Fail(format!("timedatectl missing: {e}"))),
        };
        let v = String::from_utf8_lossy(&out.stdout).trim().to_string();
        match v.as_str() {
            "yes" => Ok(CheckOutcome::Pass),
            other => Ok(CheckOutcome::Fail(format!("NTPSynchronized={other}"))),
        }
    })
}

/// `mounts-health`: every configured mount has a guest path that resolves,
/// plus a non-empty source. Network mounts are reported as `Skip` when the
/// kernel module isn't loaded (cifs/nfs); the user can install the module
/// or remove the mount. Pure data-layer check — no live `mount` calls.
fn check_mounts_health() -> Result<CheckOutcome> {
    let entries = match crate::mounts::list(None) {
        Ok(e) => e,
        Err(err) => return Ok(CheckOutcome::Fail(format!("read mounts policy: {err}"))),
    };
    if entries.is_empty() {
        return Ok(CheckOutcome::Skip(
            "no mounts configured (this is fine; deputyctl mounts add to attach one)".to_string(),
        ));
    }
    let mut bad: Vec<String> = Vec::new();
    for e in &entries {
        if !e.guest_path.starts_with("/mnt/deputyos/") {
            bad.push(format!("{}: guest_path outside /mnt/deputyos/", e.id));
        }
        if e.source.trim().is_empty() {
            bad.push(format!("{}: empty source", e.id));
        }
    }
    if bad.is_empty() {
        Ok(CheckOutcome::Pass)
    } else {
        Ok(CheckOutcome::Fail(bad.join("; ")))
    }
}

/// Verify the on-disk network policy agrees with the live nftables ruleset.
///
/// - `open`: nothing to enforce (ufw owns inbound) → Pass.
/// - `whitelist`/`airgap`: the live ruleset must contain the `deputyos` table
///   with a `policy drop` output chain, else Warn (drift — policy says drop but
///   the kernel isn't enforcing it; run `deputyctl network apply`).
///
/// Best-effort on `nft`: a missing `nft` binary is a Warn, not a Fail, so the
/// check is useful on a dev host (where nftables may be absent) without
/// panicking.
fn check_network_policy() -> Result<CheckOutcome> {
    linux_only(|| {
        let policy = match network::read(None) {
            Ok(p) => p,
            Err(e) => return Ok(CheckOutcome::Fail(format!("read network policy: {e}"))),
        };
        if policy.mode == network::Mode::Open {
            return Ok(CheckOutcome::Pass);
        }
        // whitelist or airgap: confirm the live ruleset enforces it.
        let out = match Command::new("nft").args(["list", "ruleset"]).output() {
            Ok(o) => o,
            Err(_) => {
                return Ok(CheckOutcome::Warn(format!(
                    "mode={}: nft not available to verify the live ruleset",
                    policy.mode
                )));
            }
        };
        if !out.status.success() {
            return Ok(CheckOutcome::Warn(format!(
                "mode={}: `nft list ruleset` exited {} — ruleset not verifiable",
                policy.mode,
                out.status.code().unwrap_or(-1)
            )));
        }
        let text = String::from_utf8_lossy(&out.stdout);
        let has_table = text.contains("table inet deputyos");
        let has_drop = text.contains("policy drop");
        if has_table && has_drop {
            Ok(CheckOutcome::Pass)
        } else {
            Ok(CheckOutcome::Warn(format!(
                "mode={}: nftables ruleset drift — no deputyos table/policy drop (run `deputyctl network apply`)",
                policy.mode
            )))
        }
    })
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Read MemTotal in KB from `/proc/meminfo`. None on parse failure.
fn read_meminfo_total_kb() -> Option<u64> {
    let raw = std::fs::read_to_string("/proc/meminfo").ok()?;
    parse_meminfo_total_kb(&raw)
}

/// Pure parser for /proc/meminfo's `MemTotal:` line. Public for tests.
pub fn parse_meminfo_total_kb(raw: &str) -> Option<u64> {
    for line in raw.lines() {
        if let Some(rest) = line.strip_prefix("MemTotal:") {
            return rest.split_whitespace().next()?.parse::<u64>().ok();
        }
    }
    None
}

/// Render the report as a fixed-width text table.
pub fn format_table(report: &DoctorReport) -> String {
    let mut s = String::new();
    s.push_str("deputyctl doctor — security & health\n");
    s.push_str("------------------------------------\n");
    let name_w = report
        .checks
        .iter()
        .map(|c| c.name.len())
        .max()
        .unwrap_or(0)
        .max(4);
    for r in &report.checks {
        s.push_str(&format!(
            "  {:<width$}  {}  {}\n",
            r.name,
            r.outcome.label(),
            r.outcome.detail(),
            width = name_w
        ));
        if matches!(r.outcome, CheckOutcome::Fail(_) | CheckOutcome::Warn(_)) {
            s.push_str(&format!(
                "  {:<width$}  fix: {}\n",
                "",
                r.fix,
                width = name_w
            ));
        }
    }
    s.push_str(&format!(
        "\nsummary: {} pass / {} warn / {} fail / {} skip\n",
        report.passes, report.warns, report.fails, report.skips
    ));
    s
}

/// Helper used by `deputyctl status` for the one-line health summary.
pub fn quick_fail_count() -> usize {
    let mut counts: BTreeMap<&'static str, usize> = BTreeMap::new();
    for c in all_checks() {
        if let Ok(o) = (c.run)() {
            if o.is_fail() {
                *counts.entry(c.name).or_insert(0) += 1;
            }
        }
    }
    counts.values().sum()
}
