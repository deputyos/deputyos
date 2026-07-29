//! `deputyctl network` — read + mutate `/etc/deputyos/network-policy.json`.
//!
//! Three modes:
//! - `open` — outbound network unrestricted (the default for non-airgap builds).
//! - `whitelist` — allow only the hosts in `allow_hosts`, enforced by an
//!   nftables `output` chain `policy drop` plus `ip daddr <resolved> accept`
//!   per host. The generator resolves each hostname to IPv4 at apply time and
//!   pins those IPs — see the [egress threat model](../../documentation/docs/concepts/threat-model-egress.md)
//!   for the DNS-only (not SNI) limitation. Switching to `whitelist` with an
//!   empty `allow_hosts` seeds from `/etc/deputyos/network-defaults.json` if
//!   present (idempotent).
//! - `airgap` — deny everything except RFC1918 + mDNS + local DNS resolver.
//!
//! Writes are atomic-rename onto `/etc/deputyos/network-policy.json`. Applying
//! to the live device renders the nftables ruleset (`generate_nftables_ruleset`)
//! to `/etc/nftables.conf` and shells out to `nft -f`. A boot oneshot
//! (`deputyos-network-apply.service`) re-applies from the policy on every boot.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};

use crate::paths;

/// Resolve the policy path: an explicit test arg, else `paths::network_policy_file()`
/// (which honours `DEPUTYOS_NETWORK_POLICY` for hermetic tests, mirroring mounts).
fn resolve_path(path: Option<&Path>) -> PathBuf {
    path.map(Path::to_path_buf)
        .unwrap_or_else(paths::network_policy_file)
}

/// Path to the per-profile network-defaults seed (`/etc/deputyos/network-defaults.json`
/// by default, or `DEPUTYOS_NETWORK_DEFAULTS` for hermetic tests). Returns `None`
/// when no seed file exists — seeding is then a no-op.
fn defaults_file() -> Option<PathBuf> {
    let p = std::env::var("DEPUTYOS_NETWORK_DEFAULTS")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("/etc/deputyos/network-defaults.json"));
    if p.exists() {
        Some(p)
    } else {
        None
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    Open,
    Whitelist,
    Airgap,
}

impl std::str::FromStr for Mode {
    type Err = anyhow::Error;
    fn from_str(s: &str) -> Result<Self> {
        match s.to_ascii_lowercase().as_str() {
            "open" => Ok(Mode::Open),
            "whitelist" => Ok(Mode::Whitelist),
            "airgap" => Ok(Mode::Airgap),
            other => bail!("unknown mode: {other:?} (expected open|whitelist|airgap)"),
        }
    }
}

impl std::fmt::Display for Mode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Mode::Open => f.write_str("open"),
            Mode::Whitelist => f.write_str("whitelist"),
            Mode::Airgap => f.write_str("airgap"),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Policy {
    /// Identifies the schema version; current is 1.
    #[serde(default = "default_schema", rename = "$schema")]
    pub schema: String,
    pub mode: Mode,
    #[serde(default)]
    pub allow_hosts: Vec<String>,
    /// True if the policy was written by the image bake; flipped to false
    /// once the user has touched it via `deputyctl network …`.
    #[serde(default)]
    pub set_at_build_time: bool,
    pub tier: Option<String>,
    pub hw: Option<String>,
    pub profile: Option<String>,
}

/// The shape of `/etc/deputyos/network-defaults.json` — a per-profile curated
/// seed list, baked at image time from `roles/deputyos/files/network-defaults.<profile>.json`.
/// Only `allow_hosts` is read when seeding; `mode`/`profile` are informational.
#[derive(Debug, Deserialize)]
struct DefaultsFile {
    #[serde(default)]
    allow_hosts: Vec<String>,
}

fn default_schema() -> String {
    "https://www.deputyos.com/schemas/network-policy-v1.json".to_string()
}

impl Policy {
    pub fn open() -> Self {
        Self {
            schema: default_schema(),
            mode: Mode::Open,
            allow_hosts: Vec::new(),
            set_at_build_time: false,
            tier: None,
            hw: None,
            profile: None,
        }
    }
}

/// Read the live policy file. If it doesn't exist (e.g. on a non-airgap dev
/// host), synthesise a default `open` policy.
pub fn read(path: Option<&Path>) -> Result<Policy> {
    let p = resolve_path(path);
    if !p.exists() {
        return Ok(Policy::open());
    }
    let body = fs::read_to_string(&p).with_context(|| format!("reading {}", p.display()))?;
    let policy: Policy =
        serde_json::from_str(&body).with_context(|| format!("parsing JSON in {}", p.display()))?;
    Ok(policy)
}

/// Atomic-rename onto the policy path (default `/etc/deputyos/network-policy.json`,
/// or `DEPUTYOS_NETWORK_POLICY` if set).
pub fn write(path: Option<&Path>, policy: &Policy) -> Result<PathBuf> {
    let p = resolve_path(path);
    let parent = p.parent().context("network-policy path has no parent")?;
    fs::create_dir_all(parent).with_context(|| format!("ensuring {} exists", parent.display()))?;
    let tmp = parent.join(".network-policy.json.tmp");
    let body = serde_json::to_string_pretty(policy).context("serialising policy")?;
    fs::write(&tmp, body.as_bytes()).with_context(|| format!("writing {}", tmp.display()))?;
    fs::rename(&tmp, &p).with_context(|| format!("renaming → {}", p.display()))?;
    Ok(p)
}

/// `deputyctl network status [--json]`.
pub fn status_json(path: Option<&Path>) -> Result<serde_json::Value> {
    let policy = read(path)?;
    Ok(serde_json::json!({
        "mode": policy.mode.to_string(),
        "allow_hosts": policy.allow_hosts,
        "set_at_build_time": policy.set_at_build_time,
        "tier": policy.tier,
        "hw": policy.hw,
        "profile": policy.profile,
        "policy_path": path.map(|p| p.to_string_lossy().into_owned()).unwrap_or_else(|| resolve_path(None).to_string_lossy().into_owned()),
    }))
}

/// `deputyctl network mode <mode>` (or `--unlock`/`--lock --airgap`).
///
/// Switching to `whitelist` with an empty `allow_hosts` seeds the list from
/// `/etc/deputyos/network-defaults.json` (the per-profile curated seed) when one
/// exists — idempotent: a non-empty, user-curated list is never clobbered.
pub fn set_mode(new_mode: Mode, path: Option<&Path>) -> Result<Policy> {
    let mut policy = read(path)?;
    policy.mode = new_mode;
    policy.set_at_build_time = false;

    if new_mode == Mode::Whitelist && policy.allow_hosts.is_empty() {
        if let Some(dp) = defaults_file() {
            if let Ok(body) = fs::read_to_string(&dp) {
                if let Ok(defaults) = serde_json::from_str::<DefaultsFile>(&body) {
                    if !defaults.allow_hosts.is_empty() {
                        policy.allow_hosts = defaults.allow_hosts;
                        policy.allow_hosts.sort();
                        policy.allow_hosts.dedup();
                        eprintln!(
                            "network: seeded {} allow-host{} from {} (run `deputyctl network apply`)",
                            policy.allow_hosts.len(),
                            if policy.allow_hosts.len() == 1 { "" } else { "s" },
                            dp.display()
                        );
                    }
                }
            }
        }
    }

    write(path, &policy)?;
    Ok(policy)
}

/// `deputyctl network allow add <host>` / `remove <host>`.
///
/// Mutating allow_hosts is permitted in any mode so an operator can pre-stage
/// hosts. The list has no effect on egress until `mode=whitelist`, at which
/// point `deputyctl network apply` resolves each host to an IP and pins it.
pub fn allow_mutate<F>(path: Option<&Path>, mutate: F) -> Result<Policy>
where
    F: FnOnce(&mut Vec<String>),
{
    let mut policy = read(path)?;
    mutate(&mut policy.allow_hosts);
    policy.allow_hosts.sort();
    policy.allow_hosts.dedup();
    policy.set_at_build_time = false;
    write(path, &policy)?;
    Ok(policy)
}

/// Generate an nftables ruleset from the current policy.
///
/// - `open`: minimal ruleset (ufw handles ingress; outbound is unrestricted).
/// - `whitelist`: DNS-resolves each `allow_host`, emits `ip daddr { ... } accept`
///   rules, sets output chain policy to `drop`.
/// - `airgap`: deny-all outbound except loopback, established/related, RFC1918,
///   mDNS (5353/udp), and local DNS (127.0.0.53).
pub fn generate_nftables_ruleset(policy: &Policy) -> Result<String> {
    let mut rules = String::new();
    rules.push_str("#!/usr/sbin/nft -f\n");
    rules.push_str("# Generated by deputyctl network apply. Do not edit.\n");
    rules.push_str("flush ruleset\n\n");
    rules.push_str("table inet deputyos {\n");

    match policy.mode {
        Mode::Open => {
            rules.push_str("  chain output {\n");
            rules.push_str("    type filter hook output priority filter; policy accept;\n");
            rules.push_str("  }\n");
        }
        Mode::Airgap | Mode::Whitelist => {
            rules.push_str("  chain output {\n");
            rules.push_str("    type filter hook output priority filter; policy drop;\n");
            rules.push_str("    ct state established,related accept\n");
            rules.push_str("    iif lo accept\n");
            rules.push_str("    ip daddr 224.0.0.251 udp dport 5353 accept\n");
            rules.push_str("    ip6 daddr ff02::fb udp dport 5353 accept\n");
            rules.push_str("    ip daddr 127.0.0.0/8 accept\n");
            rules.push_str("    ip daddr 10.0.0.0/8 accept\n");
            rules.push_str("    ip daddr 172.16.0.0/12 accept\n");
            rules.push_str("    ip daddr 192.168.0.0/16 accept\n");
            rules.push_str("    ip daddr 169.254.0.0/16 accept\n");
            rules.push_str("    ip6 daddr fe80::/10 accept\n");

            if policy.mode == Mode::Whitelist {
                for host in &policy.allow_hosts {
                    if let Ok(addrs) = resolve_host(host) {
                        for addr in addrs {
                            rules.push_str(&format!("    ip daddr {addr} accept\n"));
                        }
                    } else {
                        rules.push_str(&format!("    # host could not be resolved: {host}\n"));
                    }
                }
            }

            rules.push_str("    counter drop\n");
            rules.push_str("  }\n");
        }
    }

    rules.push_str("}\n");
    Ok(rules)
}

/// Resolve a hostname to its IPv4 addresses. Returns empty on failure.
fn resolve_host(host: &str) -> Result<Vec<String>> {
    // Strip any port suffix.
    let host = host.split(':').next().unwrap_or(host);
    if host.is_empty() {
        return Ok(Vec::new());
    }
    let mut addrs = Vec::new();
    for addr in std::net::ToSocketAddrs::to_socket_addrs(&(host, 0))? {
        match addr.ip() {
            std::net::IpAddr::V4(ip) => {
                addrs.push(ip.to_string());
            }
            std::net::IpAddr::V6(_) => {} // nftables rules above are v4-focused
        }
    }
    addrs.sort();
    addrs.dedup();
    Ok(addrs)
}

/// Apply the current policy: generate ruleset, write to `/etc/nftables.conf`,
/// and reload nftables.
pub fn apply_ruleset(path: Option<&Path>) -> Result<()> {
    let policy = read(path)?;
    let rules = generate_nftables_ruleset(&policy)?;

    let nft_conf = Path::new("/etc/nftables.conf");
    let tmp = nft_conf.with_extension("conf.tmp");
    std::fs::write(&tmp, rules.as_bytes()).with_context(|| format!("writing {}", tmp.display()))?;
    std::fs::rename(&tmp, nft_conf)
        .with_context(|| format!("renaming -> {}", nft_conf.display()))?;

    let status = std::process::Command::new("nft")
        .args(["-f", "/etc/nftables.conf"])
        .status()
        .context("running nft -f /etc/nftables.conf")?;

    if !status.success() {
        anyhow::bail!("nft -f /etc/nftables.conf exited non-zero");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn temp_policy_path() -> (TempDir, PathBuf) {
        let dir = TempDir::new().expect("tempdir");
        let path = dir.path().join("network-policy.json");
        (dir, path)
    }

    #[test]
    fn read_missing_returns_open() {
        let (_d, p) = temp_policy_path();
        let pol = read(Some(&p)).expect("read");
        assert_eq!(pol.mode, Mode::Open);
    }

    #[test]
    fn write_then_read_roundtrip() {
        let (_d, p) = temp_policy_path();
        let mut pol = Policy::open();
        pol.mode = Mode::Airgap;
        pol.allow_hosts.push("api.openai.com".into());
        write(Some(&p), &pol).expect("write");
        let read_back = read(Some(&p)).expect("read");
        assert_eq!(read_back.mode, Mode::Airgap);
        assert_eq!(read_back.allow_hosts, vec!["api.openai.com"]);
    }

    #[test]
    fn whitelist_mode_accepted() {
        let (_d, p) = temp_policy_path();
        let pol = set_mode(Mode::Whitelist, Some(&p)).expect("whitelist accepted");
        assert_eq!(pol.mode, Mode::Whitelist);
    }

    /// Switching to `whitelist` with an empty list seeds `allow_hosts` from the
    /// per-profile `network-defaults.json`; a non-empty list is never clobbered.
    #[test]
    fn whitelist_seeds_from_defaults_when_empty() {
        let _g = crate::env_mutex().lock().unwrap_or_else(|e| e.into_inner());
        let dir = TempDir::new().expect("tempdir");
        let policy_path = dir.path().join("policy.json");
        let defaults_path = dir.path().join("network-defaults.json");
        std::env::set_var("DEPUTYOS_NETWORK_DEFAULTS", &defaults_path);

        // No defaults file → stays empty (no-op seed).
        let pol = set_mode(Mode::Whitelist, Some(&policy_path)).expect("set");
        assert!(pol.allow_hosts.is_empty(), "no defaults → no seed");

        // Defaults file present → seeds, sorted + deduped.
        std::fs::write(
            &defaults_path,
            r#"{"$schema":"x","profile":"openclaw","mode":"open","allow_hosts":["b.example","a.example","a.example"]}"#,
        )
        .expect("write defaults");
        // Reset to empty for the seeding to trigger.
        let mut fresh = Policy::open();
        fresh.mode = Mode::Open;
        write(Some(&policy_path), &fresh).expect("reset empty");
        let pol = set_mode(Mode::Whitelist, Some(&policy_path)).expect("set");
        assert_eq!(pol.allow_hosts, vec!["a.example", "b.example"]);

        // Non-empty, user-curated list is never clobbered.
        let mut curated = Policy::open();
        curated.mode = Mode::Open;
        curated.allow_hosts = vec!["user.curated.example".into()];
        write(Some(&policy_path), &curated).expect("write curated");
        let pol = set_mode(Mode::Whitelist, Some(&policy_path)).expect("set");
        assert_eq!(
            pol.allow_hosts,
            vec!["user.curated.example"],
            "curated list must not be clobbered"
        );

        std::env::remove_var("DEPUTYOS_NETWORK_DEFAULTS");
    }

    #[test]
    fn generate_airgap_ruleset_drops_egress() {
        let pol = Policy {
            mode: Mode::Airgap,
            ..Policy::open()
        };
        let rules = generate_nftables_ruleset(&pol).expect("generate");
        assert!(
            rules.contains("policy drop"),
            "airgap must drop by default: {rules}"
        );
        assert!(
            rules.contains("ct state established"),
            "must allow established: {rules}"
        );
    }

    #[test]
    fn generate_whitelist_ruleset_includes_resolved_hosts() {
        let pol = Policy {
            mode: Mode::Whitelist,
            allow_hosts: vec!["127.0.0.1".into()],
            ..Policy::open()
        };
        let rules = generate_nftables_ruleset(&pol).expect("generate");
        assert!(
            rules.contains("policy drop"),
            "whitelist must drop by default: {rules}"
        );
        assert!(
            rules.contains("127.0.0.1"),
            "must include resolved host: {rules}"
        );
    }

    #[test]
    fn allow_add_dedupes_and_sorts() {
        let (_d, p) = temp_policy_path();
        allow_mutate(Some(&p), |hosts| hosts.push("b.example".into())).expect("add");
        allow_mutate(Some(&p), |hosts| hosts.push("a.example".into())).expect("add");
        allow_mutate(Some(&p), |hosts| hosts.push("a.example".into())).expect("add");
        let pol = read(Some(&p)).expect("read");
        assert_eq!(pol.allow_hosts, vec!["a.example", "b.example"]);
    }

    #[test]
    fn mode_parse_round_trip() {
        for m in [Mode::Open, Mode::Whitelist, Mode::Airgap] {
            let s = m.to_string();
            assert_eq!(s.parse::<Mode>().expect("parse"), m);
        }
    }

    /// `DEPUTYOS_NETWORK_POLICY` routes read/write to an override path, so the
    /// policy surface is hermetic-testable exactly like mounts (M1).
    #[test]
    fn env_override_routes_to_override_path() {
        let _g = crate::env_mutex().lock().unwrap_or_else(|e| e.into_inner());
        let dir = TempDir::new().expect("tempdir");
        let override_path = dir.path().join("policy.json");

        // Point the override at our temp file and write+read through `None`
        // (the path the CLI uses in production).
        std::env::set_var("DEPUTYOS_NETWORK_POLICY", &override_path);
        let mut pol = Policy::open();
        pol.mode = Mode::Whitelist;
        pol.allow_hosts.push("api.openai.com".into());
        write(None, &pol).expect("write via env override");
        let read_back = read(None).expect("read via env override");
        assert_eq!(read_back.mode, Mode::Whitelist);
        assert_eq!(read_back.allow_hosts, vec!["api.openai.com"]);
        assert!(override_path.exists(), "override file was written");
        std::env::remove_var("DEPUTYOS_NETWORK_POLICY");
    }
}
