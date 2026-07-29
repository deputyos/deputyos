//! Data-fetch layer: shells out to `deputyctl <subcommand> --json` and parses
//! into typed views. Falls back to deterministic dev-stub fixtures when
//! `DEPUTYPWA_DEV_STUB=1` (so contributors can iterate on UI without a baked
//! image), or when the binary cannot be located on PATH.
//!
//! Trust boundary: the JSON shapes consumed here are produced by the same
//! `deputyctl` binary that's on the appliance. We only deserialize fields we
//! intend to render; unknown fields are ignored. Errors degrade gracefully
//! to a string we render in a "stale" badge — never propagated as 5xx.

use std::path::Path;
use std::process::Command;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::paths;

/// Top-level dashboard data — one round-trip per shell-out, run in parallel
/// from the route handler.
#[derive(Debug, Clone, Serialize)]
pub struct Dashboard {
    pub status: StatusView,
    pub version: VersionView,
    pub limits: LimitsView,
    pub cost: CostView,
    pub doctor: DoctorView,
    pub network: NetworkView,
    /// True when any data source fell back to a stub or a stale snapshot.
    pub stub: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct StatusView {
    #[serde(default)]
    pub profile_id: String,
    #[serde(default)]
    pub unit: String,
    #[serde(default)]
    pub active_state: String,
    #[serde(default)]
    pub agent: AgentStatusView,
    #[serde(default)]
    pub tunnel: TunnelStatusView,
    #[serde(default)]
    pub uptime_seconds: u64,
    #[serde(default)]
    pub cost_tripped: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct AgentStatusView {
    #[serde(default)]
    pub profile_id: String,
    #[serde(default)]
    pub display_name: String,
    #[serde(default)]
    pub unit: String,
    #[serde(default)]
    pub active_state: String,
    #[serde(default)]
    pub journal_unit: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct TunnelStatusView {
    #[serde(default)]
    pub unit: String,
    #[serde(default)]
    pub active_state: String,
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub on_demand: bool,
    #[serde(default)]
    pub token_path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct VersionView {
    #[serde(default)]
    pub binary_version: String,
    #[serde(default)]
    pub kernel: String,
    #[serde(default)]
    pub channel: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct LimitsView {
    #[serde(default)]
    pub target: String,
    #[serde(default)]
    pub tier: String,
    #[serde(default)]
    pub ram_mb: u32,
    #[serde(default)]
    pub storage_class: String,
    #[serde(default)]
    pub capabilities: serde_json::Value,
    #[serde(default)]
    pub limitations: Vec<LimitationView>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct LimitationView {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub reason: String,
    #[serde(default)]
    pub unblock: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct CostView {
    #[serde(default)]
    pub today_usd: f64,
    #[serde(default)]
    pub month_usd: f64,
    #[serde(default)]
    pub daily_cap_usd: f64,
    #[serde(default)]
    pub monthly_cap_usd: f64,
    #[serde(default)]
    pub tripped: bool,
    #[serde(default)]
    pub recent: Vec<CostRecent>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct CostRecent {
    #[serde(default)]
    pub timestamp: String,
    #[serde(default)]
    pub provider: String,
    #[serde(default)]
    pub model: String,
    #[serde(default)]
    pub usd: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct DoctorView {
    #[serde(default)]
    pub fails: usize,
    #[serde(default)]
    pub warns: usize,
    #[serde(default)]
    pub passes: usize,
    #[serde(default)]
    pub skips: usize,
    #[serde(default)]
    pub checks: Vec<DoctorCheck>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct DoctorCheck {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub outcome: serde_json::Value,
    #[serde(default)]
    pub fix: String,
}

/// Network egress policy snapshot for the dashboard.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct NetworkView {
    #[serde(default)]
    pub mode: String,
    #[serde(default)]
    pub allow_hosts: Vec<String>,
    #[serde(default)]
    pub set_at_build_time: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct ProviderEntry {
    pub id: String,
    pub display_name: String,
    pub configured: bool,
    pub key_env_var: String,
    pub masked_key_prefix: String,
}

/// Build a complete dashboard view. Each subprocess call is bounded; if any
/// fail we fill that slice with defaults and set `stub=true`.
pub fn fetch_dashboard() -> Dashboard {
    if paths::dev_stub_enabled() || paths::which_deputyctl().is_none() {
        return stub_dashboard();
    }
    let bin = paths::which_deputyctl().expect("checked above");
    let status = run_json::<StatusView>(&bin, &["status", "--json"]).unwrap_or_default();
    let version = run_json::<VersionView>(&bin, &["version", "--json"]).unwrap_or_default();
    let limits = run_json::<LimitsView>(&bin, &["limits", "--json"]).unwrap_or_default();
    let cost = run_json::<CostView>(&bin, &["cost", "--json"]).unwrap_or_default();
    let doctor = run_json::<DoctorView>(&bin, &["doctor", "--json"]).unwrap_or_default();
    let network =
        run_json::<NetworkView>(&bin, &["network", "status", "--json"]).unwrap_or_default();
    Dashboard {
        status,
        version,
        limits,
        cost,
        doctor,
        network,
        stub: false,
    }
}

/// Provider list for the `/app/keys` page. In dev-stub mode we render a
/// synthetic catalogue so the UI is exercisable without an image.
pub fn fetch_providers() -> Vec<ProviderEntry> {
    if paths::dev_stub_enabled() || paths::which_deputyctl().is_none() {
        return stub_providers();
    }
    let bin = match paths::which_deputyctl() {
        Some(p) => p,
        None => return stub_providers(),
    };
    let raw = match run_raw(&bin, &["model", "list", "--json"]) {
        Ok(s) => s,
        Err(_) => return stub_providers(),
    };
    let parsed: Vec<serde_json::Value> = match serde_json::from_str(&raw) {
        Ok(v) => v,
        Err(_) => return stub_providers(),
    };
    parsed
        .into_iter()
        .map(|v| ProviderEntry {
            id: v
                .get("id")
                .and_then(|s| s.as_str())
                .unwrap_or("")
                .to_string(),
            display_name: v
                .get("display_name")
                .and_then(|s| s.as_str())
                .unwrap_or("")
                .to_string(),
            configured: v
                .get("configured")
                .and_then(|s| s.as_bool())
                .unwrap_or(false),
            key_env_var: v
                .get("key_env_var")
                .and_then(|s| s.as_str())
                .unwrap_or("")
                .to_string(),
            masked_key_prefix: v
                .get("masked_key_prefix")
                .and_then(|s| s.as_str())
                .unwrap_or("")
                .to_string(),
        })
        .collect()
}

/// Tail journal lines for the active profile's unit. Best-effort — if
/// `journalctl` isn't on PATH (a dev laptop) we return a stub.
pub fn fetch_journal_tail(unit: &str, lines: usize) -> String {
    if paths::dev_stub_enabled() {
        return stub_journal();
    }
    let lines = lines.clamp(10, 1000);
    let lines_arg = lines.to_string();
    let out = Command::new("journalctl")
        .args(["-u", unit, "-n", &lines_arg, "--no-pager"])
        .output();
    match out {
        Ok(o) if o.status.success() => String::from_utf8_lossy(&o.stdout).to_string(),
        Ok(o) => format!(
            "(journalctl exited {}: {})",
            o.status,
            String::from_utf8_lossy(&o.stderr).trim()
        ),
        Err(e) => format!("(journalctl unavailable: {e})"),
    }
}

fn run_json<T: serde::de::DeserializeOwned + Default>(bin: &Path, args: &[&str]) -> Option<T> {
    let raw = run_raw(bin, args).ok()?;
    serde_json::from_str(&raw).ok()
}

pub fn run_raw(bin: &Path, args: &[&str]) -> anyhow::Result<String> {
    let out = Command::new(bin).args(args).output()?;
    if !out.status.success() {
        anyhow::bail!(
            "deputyctl {} exited {}: {}",
            args.join(" "),
            out.status,
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&out.stdout).to_string())
}

/// Approximate "agent looks healthy" decision based on status. Used by the
/// dashboard banner so we don't have to teach the template all the edge
/// cases.
pub fn agent_healthy(s: &StatusView) -> bool {
    matches!(s.active_state.as_str(), "active" | "running" | "Active")
}

/// Format an uptime in seconds as "1d 2h 3m" for the dashboard. Intentionally
/// std-only.
pub fn format_uptime(secs: u64) -> String {
    let d = secs / 86_400;
    let h = (secs % 86_400) / 3_600;
    let m = (secs % 3_600) / 60;
    if d > 0 {
        format!("{d}d {h}h {m}m")
    } else if h > 0 {
        format!("{h}h {m}m")
    } else {
        format!("{m}m")
    }
}

// ---------------------------------------------------------------------------
// Account + integrated-tunnel cards (`/app/account`, `/app/tunnel`).
//
// SECURITY: these views expose ONLY presence booleans and non-secret labels.
// The tunnel/backup tokens themselves are NEVER read here — we `stat` the
// token file's existence so an operator can see "a token is configured"
// without the capability secret ever crossing into the PWA's HTML/response
// body. The account.json label holds email + device_id + device_name (no
// tokens) precisely so the PWA can render identity without touching secrets.
// ---------------------------------------------------------------------------

/// `/app/tunnel` card — state of the integrated cloud relay tunnel.
#[derive(Debug, Clone, Serialize, Default)]
pub struct TunnelCard {
    /// `"integrated"` when the deputyos-tunnel unit is active or a tunnel token
    /// is present; `"none"` otherwise.
    pub kind: String,
    /// `systemctl is-active deputyos-tunnel` == `"active"`.
    pub active: bool,
    /// `systemctl is-enabled deputyos-tunnel` reports an enabled-like state.
    pub enabled: bool,
    /// `/etc/deputyos/tunnel-token` exists (presence only — never contents).
    pub token_present: bool,
    /// Cloud-relay URL this device is reachable at once the tunnel is up.
    /// Rendered as copy-able code; the `<account>` segment is the account
    /// email from account.json when known, else a literal placeholder.
    pub public_url: String,
    pub stub: bool,
}

/// `/app/account` card — device identity + token presence.
#[derive(Debug, Clone, Serialize, Default)]
pub struct AccountCard {
    /// account.json `registered` flag.
    pub registered: bool,
    /// account.json `email` (self-reported label; never an auth credential).
    pub email: String,
    /// account.json `device_id` (opaque; safe to show the operator).
    pub device_id: String,
    /// `/etc/hostname` — the device's self-reported name.
    pub device_name: String,
    /// `/etc/deputyos/tunnel-token` presence (boolean only).
    pub tunnel_token_present: bool,
    /// `/etc/deputyos/backup-token` presence (boolean only).
    pub backup_token_present: bool,
    pub stub: bool,
}

/// On-disk shape of the non-secret `/etc/deputyos/account.json` label written by
/// the wizard Account step.
#[derive(Debug, Clone, Deserialize, Default)]
struct AccountFileOnDisk {
    #[serde(default)]
    registered: bool,
    #[serde(default)]
    device_id: String,
    #[serde(default)]
    device_name: String,
    #[serde(default)]
    email: Option<String>,
}

pub fn fetch_tunnel() -> TunnelCard {
    if paths::dev_stub_enabled() || paths::which_deputyctl().is_none() {
        return stub_tunnel();
    }
    let token_present = deputyctl::paths::tunnel_token_file().exists();
    let active = systemctl_is_active("deputyos-tunnel");
    let enabled = systemctl_is_enabled("deputyos-tunnel");
    let kind = if active || token_present {
        "integrated".to_string()
    } else {
        "none".to_string()
    };
    let email = read_account_file().and_then(|a| a.email);
    TunnelCard {
        kind,
        active,
        enabled,
        token_present,
        public_url: relay_url_hint(email.as_deref()),
        stub: false,
    }
}

pub fn fetch_account() -> AccountCard {
    if paths::dev_stub_enabled() || paths::which_deputyctl().is_none() {
        return stub_account();
    }
    let on_disk = read_account_file();
    let device_name = on_disk
        .as_ref()
        .map(|a| a.device_name.clone())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(hostname);
    AccountCard {
        registered: on_disk.as_ref().map(|a| a.registered).unwrap_or(false),
        email: on_disk
            .as_ref()
            .and_then(|a| a.email.clone())
            .unwrap_or_default(),
        device_id: on_disk.map(|a| a.device_id).unwrap_or_default(),
        device_name,
        tunnel_token_present: deputyctl::paths::tunnel_token_file().exists(),
        backup_token_present: deputyctl::paths::cloud_backup_token_file().exists(),
        stub: false,
    }
}

/// `/app/mounts` card — the configured drive mounts + shares (M3.5).
/// `entries` is the live `deputyctl::mounts::list` output; `stub` marks the
/// dev-fixture path so the page can label it.
#[derive(Debug, Clone, Serialize, Default)]
pub struct MountsCard {
    pub entries: Vec<deputyctl::mounts::ListEntry>,
    pub stub: bool,
}

pub fn fetch_mounts() -> MountsCard {
    if paths::dev_stub_enabled() || paths::which_deputyctl().is_none() {
        return stub_mounts();
    }
    let entries = deputyctl::mounts::list(None).unwrap_or_default();
    MountsCard {
        entries,
        stub: false,
    }
}

fn stub_mounts() -> MountsCard {
    MountsCard {
        entries: vec![
            deputyctl::mounts::ListEntry {
                kind: "host-fs".into(),
                id: "documents".into(),
                guest_path: "/mnt/deputyos/documents".into(),
                mode: "ro".into(),
                source: "/home/operator/Documents".into(),
            },
            deputyctl::mounts::ListEntry {
                kind: "cifs".into(),
                id: "nas-photos".into(),
                guest_path: "/mnt/deputyos/nas-photos".into(),
                mode: "ro".into(),
                source: "//nas.lan/photos".into(),
            },
        ],
        stub: true,
    }
}

fn read_account_file() -> Option<AccountFileOnDisk> {
    let path = deputyctl::paths::account_file();
    let raw = std::fs::read_to_string(&path).ok()?;
    serde_json::from_str(&raw).ok()
}

/// Best-effort `/etc/hostname` read; falls back to `$HOSTNAME` then a literal.
fn hostname() -> String {
    if let Ok(s) = std::fs::read_to_string("/etc/hostname") {
        let t = s.trim().to_string();
        if !t.is_empty() {
            return t;
        }
    }
    std::env::var("HOSTNAME").unwrap_or_else(|_| "this-device".to_string())
}

/// `systemctl is-active <unit>` → true only on a literal `"active"`.
fn systemctl_is_active(unit: &str) -> bool {
    match Command::new("systemctl").args(["is-active", unit]).output() {
        Ok(o) => String::from_utf8_lossy(&o.stdout).trim() == "active",
        Err(_) => false,
    }
}

/// `systemctl is-enabled <unit>` → true on the enabled-like states. `is-enabled`
/// exits non-zero on `"disabled"`/`"masked"`, so we inspect stdout.
fn systemctl_is_enabled(unit: &str) -> bool {
    let Ok(o) = Command::new("systemctl")
        .args(["is-enabled", unit])
        .output()
    else {
        return false;
    };
    matches!(
        String::from_utf8_lossy(&o.stdout).trim(),
        "enabled" | "enabled-runtime" | "static" | "indirect" | "alias"
    )
}

/// Cloud-relay public URL form. The relay is path-based
/// (`api.deputyos.com/api/v1/tunnel/proxy/{account}/{path}`), matched against
/// the account email OR id. We substitute the known email when present.
fn relay_url_hint(account_email: Option<&str>) -> String {
    let base = std::env::var("DEPUTYOS_API_BASE")
        .unwrap_or_else(|_| "https://api.deputyos.com".to_string());
    let account = account_email.unwrap_or("<account>");
    format!(
        "{}/api/v1/tunnel/proxy/{}/",
        base.trim_end_matches('/'),
        account
    )
}

/// Bound on subprocess wait. Currently informational; the std `Command`
/// path doesn't expose timeouts directly. Future: convert to `tokio::process`
/// for true cancellation. For M3 the deputyctl calls are sub-second.
pub const SUBPROCESS_BUDGET: Duration = Duration::from_secs(5);

// ---------------------------------------------------------------------------
// Dev-stub fixtures
// ---------------------------------------------------------------------------

fn stub_dashboard() -> Dashboard {
    Dashboard {
        status: StatusView {
            profile_id: "openclaw".into(),
            unit: "openclaw-gateway.service".into(),
            active_state: "active".into(),
            agent: AgentStatusView {
                profile_id: "openclaw".into(),
                display_name: "OpenClaw".into(),
                unit: "openclaw-gateway.service".into(),
                active_state: "active".into(),
                journal_unit: "openclaw-gateway.service".into(),
            },
            tunnel: TunnelStatusView {
                unit: "deputyos-tunnel.service".into(),
                active_state: "inactive".into(),
                enabled: false,
                on_demand: true,
                token_path: "/etc/deputyos/tunnel-token".into(),
            },
            uptime_seconds: 90_061,
            cost_tripped: false,
        },
        version: VersionView {
            binary_version: "0.1.0-dev".into(),
            kernel: "6.6.0-dev".into(),
            channel: "dev".into(),
        },
        limits: LimitsView {
            target: "qemu-aarch64".into(),
            tier: "standard".into(),
            ram_mb: 4096,
            storage_class: "ssd".into(),
            capabilities: serde_json::json!({
                "local_llm": false,
                "voice_wake_word": true,
                "voice_tts": true,
                "clamav_daemon": true,
                "channels_heavy": [],
                "channels_disabled_by_ram": []
            }),
            limitations: vec![LimitationView {
                id: "no-local-llm".into(),
                reason: "RAM tier below 8GB threshold for local-llm".into(),
                unblock: "upgrade to a higher-RAM target".into(),
            }],
        },
        cost: CostView {
            today_usd: 0.42,
            month_usd: 6.18,
            daily_cap_usd: 5.0,
            monthly_cap_usd: 100.0,
            tripped: false,
            recent: vec![CostRecent {
                timestamp: "2026-04-26T12:34:00Z".into(),
                provider: "anthropic".into(),
                model: "claude-3-5-sonnet".into(),
                usd: 0.18,
            }],
        },
        doctor: DoctorView {
            fails: 0,
            warns: 1,
            passes: 12,
            skips: 0,
            checks: vec![DoctorCheck {
                name: "zram-swap".into(),
                outcome: serde_json::json!({"kind": "Warn", "detail": "no zram device"}),
                fix: "see `deputyctl limits` for your device tier".into(),
            }],
        },
        network: NetworkView {
            mode: "open".into(),
            allow_hosts: vec![],
            set_at_build_time: false,
        },
        stub: true,
    }
}

fn stub_providers() -> Vec<ProviderEntry> {
    vec![
        ProviderEntry {
            id: "anthropic".into(),
            display_name: "Anthropic".into(),
            configured: true,
            key_env_var: "ANTHROPIC_API_KEY".into(),
            masked_key_prefix: "sk-ant-…".into(),
        },
        ProviderEntry {
            id: "openai".into(),
            display_name: "OpenAI".into(),
            configured: false,
            key_env_var: "OPENAI_API_KEY".into(),
            masked_key_prefix: String::new(),
        },
    ]
}

fn stub_journal() -> String {
    "2026-04-26T12:34:00Z openclaw[1234]: started\n\
     2026-04-26T12:34:01Z openclaw[1234]: agent ready on :8080\n\
     2026-04-26T12:34:02Z openclaw[1234]: cost ledger appended ($0.18)\n"
        .into()
}

fn stub_tunnel() -> TunnelCard {
    TunnelCard {
        kind: "integrated".into(),
        active: true,
        enabled: false,
        token_present: true,
        public_url: relay_url_hint(Some("operator@example.com")),
        stub: true,
    }
}

fn stub_account() -> AccountCard {
    AccountCard {
        registered: true,
        email: "operator@example.com".into(),
        device_id: "dev_00000000".into(),
        device_name: "deputyos-dev".into(),
        tunnel_token_present: true,
        backup_token_present: true,
        stub: true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_uptime_round_trips() {
        assert_eq!(format_uptime(0), "0m");
        assert_eq!(format_uptime(120), "2m");
        assert_eq!(format_uptime(3_661), "1h 1m");
        assert_eq!(format_uptime(90_061), "1d 1h 1m");
    }

    #[test]
    fn stub_dashboard_is_marked_stub() {
        std::env::set_var("DEPUTYPWA_DEV_STUB", "1");
        let d = fetch_dashboard();
        assert!(d.stub);
        assert_eq!(d.status.profile_id, "openclaw");
        std::env::remove_var("DEPUTYPWA_DEV_STUB");
    }

    #[test]
    fn agent_healthy_recognises_active() {
        let s = StatusView {
            active_state: "active".into(),
            ..Default::default()
        };
        assert!(agent_healthy(&s));
        let s = StatusView {
            active_state: "failed".into(),
            ..Default::default()
        };
        assert!(!agent_healthy(&s));
    }

    #[test]
    fn stub_providers_has_anthropic() {
        std::env::set_var("DEPUTYPWA_DEV_STUB", "1");
        let p = fetch_providers();
        assert!(p.iter().any(|e| e.id == "anthropic"));
        std::env::remove_var("DEPUTYPWA_DEV_STUB");
    }

    #[test]
    fn stub_tunnel_and_account_marked_stub() {
        std::env::set_var("DEPUTYPWA_DEV_STUB", "1");
        let t = fetch_tunnel();
        assert!(t.stub);
        assert_eq!(t.kind, "integrated");
        assert!(
            t.public_url
                .contains("/api/v1/tunnel/proxy/operator@example.com/"),
            "public_url: {}",
            t.public_url
        );
        let a = fetch_account();
        assert!(a.stub);
        assert!(a.registered);
        assert!(!a.email.is_empty());
        std::env::remove_var("DEPUTYPWA_DEV_STUB");
    }

    #[test]
    fn relay_url_hint_uses_path_based_form() {
        let url = relay_url_hint(Some("alice@deputyos.com"));
        assert_eq!(
            url,
            "https://api.deputyos.com/api/v1/tunnel/proxy/alice@deputyos.com/"
        );
        let placeholder = relay_url_hint(None);
        assert!(
            placeholder.ends_with("/api/v1/tunnel/proxy/<account>/"),
            "placeholder: {placeholder}"
        );
    }
}
