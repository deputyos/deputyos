//! `deputyctl cost` — daily/monthly spend caps with auto-pause.
//!
//! Phase 6 Lane M5 (per `docs/11-roadmap.md` §M5 Lane A). Surfaces the cost
//! ledger that the agent profile (openclaw/hermes) writes to, evaluates caps,
//! fires `HookKind::CostAlert` at warn/trip thresholds, and (optionally)
//! pauses the active profile by writing a tripped-marker that the unit's
//! `ExecStartPre` reads.
//!
//! Surface extension: `cost` is NOT in the frozen surface in
//! `docs/02-profiles.md` lines 94-123. The roadmap anticipated this; the
//! parent task explicitly authorized adding it. Mirrors `quiet-hours`.
//!
//! Architectural notes:
//! * The agent profile is the **producer** of ledger entries. deputyctl is
//!   read-only on the ledger (except `cost reset`, which only clears the
//!   tripped-marker — never touches the ledger). The contract is JSONL,
//!   one entry per LLM request, format documented in
//!   [`LedgerEntry`].
//! * Date math is intentionally std-only. The ledger's `timestamp` is RFC3339
//!   UTC; we parse the `YYYY-MM-DD` prefix for "today" and the `YYYY-MM`
//!   prefix for "this month". Drift across midnight UTC is acceptable for
//!   cap-trip purposes; daily caps reset at UTC midnight.
//! * Atomic writes: tmp + rename, mode 0600 owner agent:agent (root:root in
//!   dev mode — kernel doesn't let us chown without privilege).

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{anyhow, bail, Context, Result};
use serde::{Deserialize, Serialize};

use crate::hooks::{self, HookKind};
use crate::paths;

// ---------------------------------------------------------------------------
// public option structs (mirror clap-side)
// ---------------------------------------------------------------------------

/// Subcommand-level options parsed from the CLI.
#[derive(Debug, Clone, Default)]
pub struct CostOpts {
    pub json: bool,
    /// `cost --check`: run the gate now; nonzero exit if tripped.
    pub check: bool,
}

/// `cost set` knobs. All optional — caller may set one or more at once.
#[derive(Debug, Clone, Default)]
pub struct SetOpts {
    pub daily_cap_usd: Option<f64>,
    pub monthly_cap_usd: Option<f64>,
    pub on_cap_trip: Option<String>,
    pub warn_at_pct: Option<u32>,
}

/// `cost ledger` tail-like options.
#[derive(Debug, Clone, Default)]
pub struct LedgerOpts {
    pub last: usize,
    pub json: bool,
}

// ---------------------------------------------------------------------------
// config schema (`/etc/deputyos/cost.toml`)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CostConfig {
    #[serde(default)]
    pub caps: Caps,
    #[serde(default)]
    pub behaviour: Behaviour,
    #[serde(default)]
    pub quiet_hours: QuietHoursSection,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Caps {
    #[serde(default = "default_daily_cap")]
    pub daily_usd: f64,
    #[serde(default = "default_monthly_cap")]
    pub monthly_usd: f64,
}

fn default_daily_cap() -> f64 {
    5.00
}
fn default_monthly_cap() -> f64 {
    100.00
}

impl Default for Caps {
    fn default() -> Self {
        Self {
            daily_usd: default_daily_cap(),
            monthly_usd: default_monthly_cap(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Behaviour {
    #[serde(default = "default_on_cap_trip")]
    pub on_cap_trip: String, // "pause" | "warn" | "nothing"
    #[serde(default = "default_warn_at_pct")]
    pub warn_at_pct: u32,
}

fn default_on_cap_trip() -> String {
    "pause".into()
}
fn default_warn_at_pct() -> u32 {
    80
}

impl Default for Behaviour {
    fn default() -> Self {
        Self {
            on_cap_trip: default_on_cap_trip(),
            warn_at_pct: default_warn_at_pct(),
        }
    }
}

/// Mirrors the `[quiet_hours]` section so a single config file holds both.
/// `quiet_hours::*` is the runtime API; this struct is just the on-disk shape.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QuietHoursSection {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default = "default_qh_start")]
    pub start: String, // "HH:MM" local TZ
    #[serde(default = "default_qh_end")]
    pub end: String,
    #[serde(default = "default_qh_behaviour")]
    pub behaviour: String, // "pause" | "refuse" | "nothing"
}

impl Default for QuietHoursSection {
    fn default() -> Self {
        Self {
            enabled: false,
            start: default_qh_start(),
            end: default_qh_end(),
            behaviour: default_qh_behaviour(),
        }
    }
}

fn default_qh_start() -> String {
    "22:00".into()
}
fn default_qh_end() -> String {
    "07:00".into()
}
fn default_qh_behaviour() -> String {
    "pause".into()
}

// ---------------------------------------------------------------------------
// ledger schema (JSONL, one entry per line)
// ---------------------------------------------------------------------------

/// One LLM request's cost record. The agent profile writes these; deputyctl
/// only reads. Schema is the contract; missing fields default to zero so a
/// partial entry isn't fatal.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LedgerEntry {
    /// RFC3339 UTC, e.g. `2026-04-27T12:34:56Z`.
    pub timestamp: String,
    pub provider: String,
    #[serde(default)]
    pub model: String,
    #[serde(default)]
    pub input_tokens: u64,
    #[serde(default)]
    pub output_tokens: u64,
    /// USD cost. If omitted by the producer, callers may fill it via
    /// [`estimate_cost`] using `deputyctl/etc/cost-defaults.json`.
    #[serde(default)]
    pub cost_usd: f64,
    #[serde(default)]
    pub request_id: String,
}

// ---------------------------------------------------------------------------
// path discovery
// ---------------------------------------------------------------------------

/// Path to the cost config file.
pub fn config_path() -> PathBuf {
    if let Ok(p) = std::env::var("DEPUTYOS_COST_CONFIG") {
        return PathBuf::from(p);
    }
    if let Some(dev) = paths::dev_out_dir() {
        return dev.join("cost.toml");
    }
    PathBuf::from("/etc/deputyos/cost.toml")
}

/// Path to the JSONL ledger.
///
/// In production: `~/.<active_profile>/cost-ledger.jsonl`. In dev: the path
/// pointed to by `DEPUTYOS_COST_LEDGER`, or a stub under `dev-out/`.
pub fn ledger_path() -> PathBuf {
    if let Ok(p) = std::env::var("DEPUTYOS_COST_LEDGER") {
        return PathBuf::from(p);
    }
    if let Some(dev) = paths::dev_out_dir() {
        return dev.join("cost-ledger.jsonl");
    }
    let id = paths::read_active_profile_id().unwrap_or_else(|| "agent".into());
    let home = std::env::var("HOME").unwrap_or_else(|_| "/var/lib/deputyos".into());
    PathBuf::from(home)
        .join(format!(".{id}"))
        .join("cost-ledger.jsonl")
}

/// Path to the "tripped" marker. Existence means the caps gate has fired and
/// the active profile must refuse to start until `cost reset` clears it.
pub fn tripped_marker_path() -> PathBuf {
    if let Ok(p) = std::env::var("DEPUTYOS_COST_TRIPPED") {
        return PathBuf::from(p);
    }
    if let Some(dev) = paths::dev_out_dir() {
        return dev.join("cost-tripped");
    }
    PathBuf::from("/var/lib/deputyos/cost-tripped")
}

// ---------------------------------------------------------------------------
// config IO
// ---------------------------------------------------------------------------

/// Load the cost config; returns defaults if the file is missing.
pub fn load_config() -> Result<CostConfig> {
    load_config_from(&config_path())
}

pub fn load_config_from(path: &Path) -> Result<CostConfig> {
    if !path.is_file() {
        return Ok(CostConfig::default());
    }
    let raw = std::fs::read_to_string(path)
        .with_context(|| format!("reading cost config {}", path.display()))?;
    let cfg: CostConfig =
        toml::from_str(&raw).with_context(|| format!("parsing cost config {}", path.display()))?;
    Ok(cfg)
}

/// Atomically write the cost config (tmp + rename + chmod 0600).
pub fn save_config(cfg: &CostConfig) -> Result<()> {
    save_config_to(cfg, &config_path())
}

pub fn save_config_to(cfg: &CostConfig, path: &Path) -> Result<()> {
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() && !parent.is_dir() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("creating {}", parent.display()))?;
        }
    }
    let body = toml::to_string_pretty(cfg).context("serialising cost config")?;
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, body.as_bytes()).with_context(|| format!("writing {}", tmp.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let perm = std::fs::Permissions::from_mode(0o600);
        std::fs::set_permissions(&tmp, perm)
            .with_context(|| format!("chmod 0600 {}", tmp.display()))?;
    }
    std::fs::rename(&tmp, path)
        .with_context(|| format!("renaming {} -> {}", tmp.display(), path.display()))?;
    Ok(())
}

// ---------------------------------------------------------------------------
// ledger IO
// ---------------------------------------------------------------------------

/// Read all ledger entries; missing file is not an error (returns empty vec).
/// Malformed lines are skipped with a `tracing::warn!`. We tolerate partial
/// writes since the producer is a separate process.
pub fn read_ledger() -> Result<Vec<LedgerEntry>> {
    read_ledger_from(&ledger_path())
}

pub fn read_ledger_from(path: &Path) -> Result<Vec<LedgerEntry>> {
    if !path.is_file() {
        return Ok(Vec::new());
    }
    let raw = std::fs::read_to_string(path)
        .with_context(|| format!("reading ledger {}", path.display()))?;
    let mut out = Vec::new();
    for (i, line) in raw.lines().enumerate() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        match serde_json::from_str::<LedgerEntry>(trimmed) {
            Ok(e) => out.push(e),
            Err(e) => {
                tracing::warn!(line = i + 1, err = %e, "skipping malformed ledger row");
            }
        }
    }
    Ok(out)
}

// ---------------------------------------------------------------------------
// aggregation
// ---------------------------------------------------------------------------

/// Today (UTC) total in USD, summed from `cost_usd` field.
pub fn sum_today(entries: &[LedgerEntry], today: &str) -> f64 {
    entries
        .iter()
        .filter(|e| e.timestamp.starts_with(today))
        .map(|e| e.cost_usd)
        .sum()
}

/// Month-to-date (UTC) total in USD. `month_prefix` is `YYYY-MM`.
pub fn sum_month(entries: &[LedgerEntry], month_prefix: &str) -> f64 {
    entries
        .iter()
        .filter(|e| e.timestamp.starts_with(month_prefix))
        .map(|e| e.cost_usd)
        .sum()
}

// ---------------------------------------------------------------------------
// caps gate
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CapState {
    Ok,
    Warn,
    Tripped,
}

#[derive(Debug, Clone)]
pub struct CapsReport {
    pub today_usd: f64,
    pub month_usd: f64,
    pub daily_state: CapState,
    pub monthly_state: CapState,
    pub config: CostConfig,
}

impl CapsReport {
    pub fn worst_state(&self) -> CapState {
        match (&self.daily_state, &self.monthly_state) {
            (CapState::Tripped, _) | (_, CapState::Tripped) => CapState::Tripped,
            (CapState::Warn, _) | (_, CapState::Warn) => CapState::Warn,
            _ => CapState::Ok,
        }
    }
}

/// Compute the cap state without firing hooks or pausing.
pub fn evaluate_caps(entries: &[LedgerEntry], cfg: &CostConfig, now_utc: &str) -> CapsReport {
    let today = &now_utc[..10.min(now_utc.len())]; // YYYY-MM-DD
    let month = &now_utc[..7.min(now_utc.len())]; // YYYY-MM
    let today_usd = sum_today(entries, today);
    let month_usd = sum_month(entries, month);

    let warn_pct = cfg.behaviour.warn_at_pct as f64 / 100.0;
    let daily_state = classify(today_usd, cfg.caps.daily_usd, warn_pct);
    let monthly_state = classify(month_usd, cfg.caps.monthly_usd, warn_pct);
    CapsReport {
        today_usd,
        month_usd,
        daily_state,
        monthly_state,
        config: cfg.clone(),
    }
}

fn classify(current: f64, cap: f64, warn_pct: f64) -> CapState {
    if cap <= 0.0 {
        return CapState::Ok; // disabled cap
    }
    if current >= cap {
        CapState::Tripped
    } else if current >= cap * warn_pct {
        CapState::Warn
    } else {
        CapState::Ok
    }
}

/// Evaluate caps; fire hooks; honour `on_cap_trip` (pause writes the marker
/// + best-effort `systemctl stop`). Returns the report.
///
/// In dev mode (`DEPUTYOS_DEV_OUT` set), we never spawn `systemctl`; the
/// marker file is the only side effect.
pub fn check_caps_and_maybe_pause() -> Result<CapsReport> {
    let cfg = load_config()?;
    let entries = read_ledger()?;
    let now = current_utc_iso();
    let report = evaluate_caps(&entries, &cfg, &now);

    fire_for_scope(
        "daily",
        &report.daily_state,
        report.today_usd,
        cfg.caps.daily_usd,
    );
    fire_for_scope(
        "monthly",
        &report.monthly_state,
        report.month_usd,
        cfg.caps.monthly_usd,
    );

    if matches!(report.worst_state(), CapState::Tripped) {
        match cfg.behaviour.on_cap_trip.as_str() {
            "pause" => {
                write_tripped_marker(&report)?;
                if paths::dev_out_dir().is_none() {
                    // Best-effort stop. We swallow errors — the marker is the
                    // gate of record.
                    if let Ok((_, m)) = crate::profile::load_active() {
                        let _ = crate::systemd::run("stop", &m.service.unit);
                    }
                }
            }
            "warn" | "nothing" => {
                tracing::info!(behaviour = %cfg.behaviour.on_cap_trip, "cap tripped; not pausing");
            }
            other => {
                tracing::warn!(behaviour = %other, "unknown on_cap_trip; treating as 'warn'");
            }
        }
    }
    Ok(report)
}

fn fire_for_scope(scope: &str, state: &CapState, current: f64, cap: f64) {
    let level = match state {
        CapState::Warn => "warn",
        CapState::Tripped => "trip",
        CapState::Ok => return,
    };
    let payload = serde_json::json!({
        "level": level,
        "scope": scope,
        "current_usd": current,
        "cap_usd": cap,
    });
    let _ = hooks::fire_hook(HookKind::CostAlert, &payload);
}

fn write_tripped_marker(report: &CapsReport) -> Result<()> {
    let path = tripped_marker_path();
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() && !parent.is_dir() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("creating {}", parent.display()))?;
        }
    }
    let body = format!(
        "tripped_at={}\ntoday_usd={:.4}\nmonth_usd={:.4}\ndaily_cap={:.2}\nmonthly_cap={:.2}\n",
        current_utc_iso(),
        report.today_usd,
        report.month_usd,
        report.config.caps.daily_usd,
        report.config.caps.monthly_usd,
    );
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, body.as_bytes()).with_context(|| format!("writing {}", tmp.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let perm = std::fs::Permissions::from_mode(0o644);
        std::fs::set_permissions(&tmp, perm)
            .with_context(|| format!("chmod 0644 {}", tmp.display()))?;
    }
    std::fs::rename(&tmp, &path)
        .with_context(|| format!("renaming {} -> {}", tmp.display(), path.display()))?;
    Ok(())
}

/// Clear the tripped marker. Idempotent.
pub fn clear_tripped_marker() -> Result<()> {
    let path = tripped_marker_path();
    if path.is_file() {
        std::fs::remove_file(&path).with_context(|| format!("removing {}", path.display()))?;
    }
    Ok(())
}

pub fn is_tripped() -> bool {
    tripped_marker_path().is_file()
}

// ---------------------------------------------------------------------------
// CLI entry points
// ---------------------------------------------------------------------------

/// `deputyctl cost` (default invocation).
pub fn run_summary(opts: CostOpts) -> Result<u8> {
    let cfg = load_config()?;
    let entries = read_ledger()?;
    let now = current_utc_iso();
    let report = evaluate_caps(&entries, &cfg, &now);

    if opts.check {
        // Still fire hooks/pause if we were called as the gate.
        let _ = check_caps_and_maybe_pause();
        return Ok(if matches!(report.worst_state(), CapState::Tripped) {
            1
        } else {
            0
        });
    }

    if opts.json {
        let recent = top_recent_expensive(&entries, 5);
        let payload = serde_json::json!({
            "today_usd": report.today_usd,
            "month_usd": report.month_usd,
            "daily_cap_usd": cfg.caps.daily_usd,
            "monthly_cap_usd": cfg.caps.monthly_usd,
            "daily_pct": pct(report.today_usd, cfg.caps.daily_usd),
            "monthly_pct": pct(report.month_usd, cfg.caps.monthly_usd),
            "daily_state": state_str(&report.daily_state),
            "monthly_state": state_str(&report.monthly_state),
            "tripped": is_tripped(),
            "on_cap_trip": cfg.behaviour.on_cap_trip,
            "warn_at_pct": cfg.behaviour.warn_at_pct,
            "ledger_path": ledger_path().display().to_string(),
            "config_path": config_path().display().to_string(),
            "recent_expensive": recent,
        });
        println!("{}", serde_json::to_string_pretty(&payload)?);
        return Ok(0);
    }

    if entries.is_empty() {
        println!("no costs recorded yet");
        println!("ledger:        {}", ledger_path().display());
        println!("config:        {}", config_path().display());
        println!(
            "daily cap:     ${:.2}     monthly cap: ${:.2}",
            cfg.caps.daily_usd, cfg.caps.monthly_usd
        );
        return Ok(0);
    }

    println!(
        "today:         ${:.4} / ${:.2}     ({:.0}% — {})",
        report.today_usd,
        cfg.caps.daily_usd,
        pct(report.today_usd, cfg.caps.daily_usd),
        state_str(&report.daily_state),
    );
    println!(
        "month:         ${:.4} / ${:.2}     ({:.0}% — {})",
        report.month_usd,
        cfg.caps.monthly_usd,
        pct(report.month_usd, cfg.caps.monthly_usd),
        state_str(&report.monthly_state),
    );
    println!("on_cap_trip:   {}", cfg.behaviour.on_cap_trip);
    println!("warn_at_pct:   {}%", cfg.behaviour.warn_at_pct);
    println!("tripped:       {}", is_tripped());
    println!();
    println!("recent (top 5 by cost):");
    for e in top_recent_expensive(&entries, 5) {
        println!(
            "  ${:>7.4}  {:<19}  {:<14}  {}",
            e.cost_usd, e.timestamp, e.provider, e.model
        );
    }
    Ok(0)
}

/// `deputyctl cost set --daily-cap ... --monthly-cap ... --on-cap-trip ...`
pub fn run_set(opts: SetOpts) -> Result<u8> {
    let mut cfg = load_config()?;
    let mut changed = false;
    if let Some(d) = opts.daily_cap_usd {
        if d < 0.0 {
            bail!("--daily-cap must be >= 0");
        }
        cfg.caps.daily_usd = d;
        changed = true;
    }
    if let Some(m) = opts.monthly_cap_usd {
        if m < 0.0 {
            bail!("--monthly-cap must be >= 0");
        }
        cfg.caps.monthly_usd = m;
        changed = true;
    }
    if let Some(b) = opts.on_cap_trip {
        match b.as_str() {
            "pause" | "warn" | "nothing" => {
                cfg.behaviour.on_cap_trip = b;
                changed = true;
            }
            other => bail!("--on-cap-trip: expected pause|warn|nothing, got {other}"),
        }
    }
    if let Some(w) = opts.warn_at_pct {
        if w > 100 {
            bail!("--warn-at-pct must be 0..=100");
        }
        cfg.behaviour.warn_at_pct = w;
        changed = true;
    }
    if !changed {
        eprintln!("deputyctl cost set: pass at least one of --daily-cap, --monthly-cap, --on-cap-trip, --warn-at-pct");
        return Ok(64);
    }
    save_config(&cfg)?;
    println!("daily_cap_usd:    ${:.2}", cfg.caps.daily_usd);
    println!("monthly_cap_usd:  ${:.2}", cfg.caps.monthly_usd);
    println!("on_cap_trip:      {}", cfg.behaviour.on_cap_trip);
    println!("warn_at_pct:      {}%", cfg.behaviour.warn_at_pct);
    println!("wrote:            {}", config_path().display());
    Ok(0)
}

/// `deputyctl cost reset` — clears the tripped marker only.
pub fn run_reset() -> Result<u8> {
    let was = is_tripped();
    clear_tripped_marker()?;
    if was {
        println!("cost reset: tripped marker cleared");
    } else {
        println!("cost reset: no tripped marker (already clear)");
    }
    Ok(0)
}

/// `deputyctl cost ledger [--last N] [--json]`.
pub fn run_ledger_dump(opts: LedgerOpts) -> Result<u8> {
    let entries = read_ledger()?;
    if entries.is_empty() {
        println!("no costs recorded yet");
        return Ok(0);
    }
    let n = if opts.last == 0 { 20 } else { opts.last };
    // Sort DESC by timestamp (string compare works on RFC3339 UTC).
    let mut sorted = entries.clone();
    sorted.sort_by(|a, b| b.timestamp.cmp(&a.timestamp));
    sorted.truncate(n);

    if opts.json {
        println!("{}", serde_json::to_string_pretty(&sorted)?);
        return Ok(0);
    }

    println!(
        "{:<22} {:<14} {:<28} {:>10} {:>10} {:>9}",
        "timestamp", "provider", "model", "in_tok", "out_tok", "usd",
    );
    for e in &sorted {
        println!(
            "{:<22} {:<14} {:<28} {:>10} {:>10} {:>9.4}",
            e.timestamp, e.provider, e.model, e.input_tokens, e.output_tokens, e.cost_usd,
        );
    }
    Ok(0)
}

// ---------------------------------------------------------------------------
// helpers
// ---------------------------------------------------------------------------

/// Return the top-N most expensive entries (by `cost_usd` desc, recent ties
/// broken by timestamp desc).
pub fn top_recent_expensive(entries: &[LedgerEntry], n: usize) -> Vec<LedgerEntry> {
    let mut v = entries.to_vec();
    v.sort_by(|a, b| {
        b.cost_usd
            .partial_cmp(&a.cost_usd)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| b.timestamp.cmp(&a.timestamp))
    });
    v.truncate(n);
    v
}

fn pct(current: f64, cap: f64) -> f64 {
    if cap <= 0.0 {
        0.0
    } else {
        (current / cap) * 100.0
    }
}

fn state_str(s: &CapState) -> &'static str {
    match s {
        CapState::Ok => "ok",
        CapState::Warn => "warn",
        CapState::Tripped => "tripped",
    }
}

// ---------------------------------------------------------------------------
// cost-defaults.json (provider rates) + estimate_cost
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderRate {
    #[serde(default)]
    pub default_model: String,
    pub input_per_1m_usd: f64,
    pub output_per_1m_usd: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CostDefaults {
    pub providers: std::collections::BTreeMap<String, ProviderRate>,
}

pub fn defaults_path() -> PathBuf {
    if let Ok(p) = std::env::var("DEPUTYOS_COST_DEFAULTS") {
        return PathBuf::from(p);
    }
    let etc = PathBuf::from("/etc/deputyos/cost-defaults.json");
    if etc.is_file() {
        return etc;
    }
    PathBuf::from("deputyctl/etc/cost-defaults.json")
}

pub fn load_defaults() -> Result<CostDefaults> {
    load_defaults_from(&defaults_path())
}

pub fn load_defaults_from(path: &Path) -> Result<CostDefaults> {
    let raw = std::fs::read_to_string(path)
        .with_context(|| format!("reading cost-defaults from {}", path.display()))?;
    let d: CostDefaults = serde_json::from_str(&raw)
        .with_context(|| format!("parsing cost-defaults from {}", path.display()))?;
    Ok(d)
}

/// Compute USD cost from token counts using `cost-defaults.json`. Used as a
/// fallback when a ledger entry is missing `cost_usd`.
pub fn estimate_cost(
    input_tokens: u64,
    output_tokens: u64,
    provider_id: &str,
    _model: &str,
) -> Result<f64> {
    let d = load_defaults()?;
    let rate = d.providers.get(provider_id).ok_or_else(|| {
        anyhow!("no rate sheet for provider {provider_id}; update cost-defaults.json",)
    })?;
    let in_cost = (input_tokens as f64 / 1_000_000.0) * rate.input_per_1m_usd;
    let out_cost = (output_tokens as f64 / 1_000_000.0) * rate.output_per_1m_usd;
    Ok(in_cost + out_cost)
}

// ---------------------------------------------------------------------------
// time
// ---------------------------------------------------------------------------

/// Current UTC as RFC3339 (`YYYY-MM-DDTHH:MM:SSZ`). std-only — we only need
/// the `YYYY-MM-DD` and `YYYY-MM` prefixes; minute precision is plenty.
pub fn current_utc_iso() -> String {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let (y, mo, d, h, mi, s) = epoch_to_utc_ymdhms(secs);
    format!("{y:04}-{mo:02}-{d:02}T{h:02}:{mi:02}:{s:02}Z")
}

/// Decompose unix-epoch seconds into UTC (year, month, day, hour, minute, second).
/// Algorithm: Howard Hinnant's days_from_civil. No leap-second handling
/// (UTC convention; ledger truncates to minute precision anyway).
pub fn epoch_to_utc_ymdhms(secs: u64) -> (i32, u32, u32, u32, u32, u32) {
    let s = (secs % 60) as u32;
    let m = ((secs / 60) % 60) as u32;
    let h = ((secs / 3600) % 24) as u32;
    let days = (secs / 86_400) as i64;
    // 0 = 1970-01-01.
    let z = days + 719_468; // shift epoch to 0000-03-01
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64; // [0, 146096]
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365; // [0, 399]
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
    let mp = (5 * doy + 2) / 153; // [0, 11]
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32; // [1, 31]
    let mo = if mp < 10 { mp + 3 } else { mp - 9 } as u32; // [1, 12]
    let yy = (y + if mo <= 2 { 1 } else { 0 }) as i32;
    (yy, mo, d, h, m, s)
}

// ---------------------------------------------------------------------------
// unit tests (path/string algebra)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classify_thresholds() {
        assert_eq!(classify(0.0, 5.0, 0.8), CapState::Ok);
        assert_eq!(classify(3.99, 5.0, 0.8), CapState::Ok);
        assert_eq!(classify(4.0, 5.0, 0.8), CapState::Warn);
        assert_eq!(classify(4.99, 5.0, 0.8), CapState::Warn);
        assert_eq!(classify(5.0, 5.0, 0.8), CapState::Tripped);
        assert_eq!(classify(7.5, 5.0, 0.8), CapState::Tripped);
        // disabled cap
        assert_eq!(classify(99.0, 0.0, 0.8), CapState::Ok);
    }

    #[test]
    fn epoch_decomposition_known_points() {
        // 2026-04-27T00:00:00Z = 1777248000
        assert_eq!(
            super::epoch_to_utc_ymdhms(1_777_248_000),
            (2026, 4, 27, 0, 0, 0)
        );
        // 1970-01-01T00:00:00Z
        assert_eq!(super::epoch_to_utc_ymdhms(0), (1970, 1, 1, 0, 0, 0));
        // 2000-02-29 (leap) 12:34:56  = 951827696
        assert_eq!(
            super::epoch_to_utc_ymdhms(951_827_696),
            (2000, 2, 29, 12, 34, 56)
        );
    }

    #[test]
    fn current_utc_iso_format() {
        let s = current_utc_iso();
        assert_eq!(s.len(), 20);
        assert!(s.ends_with('Z'));
        assert_eq!(&s[4..5], "-");
        assert_eq!(&s[7..8], "-");
        assert_eq!(&s[10..11], "T");
    }

    #[test]
    fn config_round_trip_defaults() {
        let dir = tempfile::tempdir().expect("tempdir");
        let p = dir.path().join("cost.toml");
        let cfg = CostConfig::default();
        save_config_to(&cfg, &p).expect("write");
        let back = load_config_from(&p).expect("read");
        assert_eq!(back.caps.daily_usd, cfg.caps.daily_usd);
        assert_eq!(back.caps.monthly_usd, cfg.caps.monthly_usd);
        assert_eq!(back.behaviour.on_cap_trip, cfg.behaviour.on_cap_trip);
        assert_eq!(back.behaviour.warn_at_pct, cfg.behaviour.warn_at_pct);
    }
}
