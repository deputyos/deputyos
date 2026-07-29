//! `deputyctl quiet-hours` — schedule when the agent stops accepting work.
//!
//! Phase 6 Lane M5 (per `docs/11-roadmap.md` §M5 Lane A). Stores schedule in
//! the same `cost.toml` file as the cost guardrails (single config file
//! avoids a second atomic-write surface). The runtime gate is the pure
//! function [`is_active`]; the agent profile calls it before responding.
//!
//! Time handling: schedule is `HH:MM` in **local time**, mirroring how a
//! user thinks about quiet hours. We resolve "local now" by reading the
//! `TZ` env var if set, else `/etc/timezone` (Debian/Ubuntu standard), else
//! UTC. This is intentionally coarse — minute precision; no DST tracking
//! beyond what the system clock applies.
//!
//! Surface extension: `quiet-hours` is NOT in the frozen `deputyctl` surface
//! in `docs/02-profiles.md`. Roadmap §M5 anticipated this; mirrors the
//! `cost` extension.

use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{bail, Result};

use crate::cost::{self, CostConfig, QuietHoursSection};

/// CLI-side options for `deputyctl quiet-hours set`.
#[derive(Debug, Clone, Default)]
pub struct SetOpts {
    pub start: Option<String>,
    pub end: Option<String>,
    pub enable: bool,
    pub disable: bool,
    pub behaviour: Option<String>,
}

/// Show current schedule + whether it's active right now.
pub fn run_show(json: bool) -> Result<u8> {
    let cfg = cost::load_config()?;
    let qh = &cfg.quiet_hours;
    let active = is_active_now(&cfg)?;

    if json {
        let payload = serde_json::json!({
            "enabled": qh.enabled,
            "start": qh.start,
            "end": qh.end,
            "behaviour": qh.behaviour,
            "active_now": active,
            "config_path": cost::config_path().display().to_string(),
        });
        println!("{}", serde_json::to_string_pretty(&payload)?);
        return Ok(0);
    }

    println!("enabled:     {}", qh.enabled);
    println!("start:       {} (local)", qh.start);
    println!("end:         {} (local)", qh.end);
    println!("behaviour:   {}", qh.behaviour);
    println!("active_now:  {active}");
    println!("config:      {}", cost::config_path().display());
    Ok(0)
}

/// `deputyctl quiet-hours set ...` — write schedule into `cost.toml`.
pub fn run_set(opts: SetOpts) -> Result<u8> {
    if opts.enable && opts.disable {
        bail!("--enable and --disable are mutually exclusive");
    }
    let mut cfg = cost::load_config()?;
    let mut changed = false;

    if let Some(s) = &opts.start {
        validate_hhmm(s)?;
        cfg.quiet_hours.start = s.clone();
        changed = true;
    }
    if let Some(e) = &opts.end {
        validate_hhmm(e)?;
        cfg.quiet_hours.end = e.clone();
        changed = true;
    }
    if let Some(b) = &opts.behaviour {
        match b.as_str() {
            "pause" | "refuse" | "nothing" => {
                cfg.quiet_hours.behaviour = b.clone();
                changed = true;
            }
            other => bail!("--behaviour: expected pause|refuse|nothing, got {other}"),
        }
    }
    if opts.enable {
        cfg.quiet_hours.enabled = true;
        changed = true;
    }
    if opts.disable {
        cfg.quiet_hours.enabled = false;
        changed = true;
    }
    if !changed {
        eprintln!(
            "deputyctl quiet-hours set: pass at least one of --start, --end, --enable, --disable, --behaviour"
        );
        return Ok(64);
    }
    cost::save_config(&cfg)?;

    println!("enabled:     {}", cfg.quiet_hours.enabled);
    println!("start:       {}", cfg.quiet_hours.start);
    println!("end:         {}", cfg.quiet_hours.end);
    println!("behaviour:   {}", cfg.quiet_hours.behaviour);
    println!("wrote:       {}", cost::config_path().display());
    Ok(0)
}

// ---------------------------------------------------------------------------
// pure runtime gate
// ---------------------------------------------------------------------------

/// Parse `HH:MM` into total minutes-since-midnight. `HH` is 0..=23,
/// `MM` is 0..=59.
pub fn parse_hhmm(s: &str) -> Result<u32> {
    let parts: Vec<&str> = s.split(':').collect();
    if parts.len() != 2 {
        bail!("expected HH:MM, got {s}");
    }
    let h: u32 = parts[0]
        .parse()
        .map_err(|_| anyhow::anyhow!("bad hour in {s}"))?;
    let m: u32 = parts[1]
        .parse()
        .map_err(|_| anyhow::anyhow!("bad minute in {s}"))?;
    if h > 23 {
        bail!("hour out of range in {s}");
    }
    if m > 59 {
        bail!("minute out of range in {s}");
    }
    Ok(h * 60 + m)
}

fn validate_hhmm(s: &str) -> Result<()> {
    parse_hhmm(s).map(|_| ())
}

/// Is the moment `now_minutes` (minutes-since-midnight, 0..1440) inside the
/// closed-open window `[start, end)`? Handles cross-midnight windows
/// (start > end), in which case the window is `[start, 1440) ∪ [0, end)`.
///
/// If start == end the window is empty (24-hour silence is not what the user
/// intends; they'd disable instead).
pub fn is_active(now_minutes: u32, start_minutes: u32, end_minutes: u32) -> bool {
    if start_minutes == end_minutes {
        return false;
    }
    if start_minutes < end_minutes {
        // simple in-day window
        now_minutes >= start_minutes && now_minutes < end_minutes
    } else {
        // cross-midnight
        now_minutes >= start_minutes || now_minutes < end_minutes
    }
}

/// Is quiet-hours active **right now**? Loads config, resolves local time,
/// returns the gate. Returns false if disabled.
pub fn is_active_now(cfg: &CostConfig) -> Result<bool> {
    if !cfg.quiet_hours.enabled {
        return Ok(false);
    }
    let s = parse_hhmm(&cfg.quiet_hours.start)?;
    let e = parse_hhmm(&cfg.quiet_hours.end)?;
    let now = local_now_minutes()?;
    Ok(is_active(now, s, e))
}

/// Convenience used by callers outside this module.
pub fn schedule(cfg: &CostConfig) -> &QuietHoursSection {
    &cfg.quiet_hours
}

// ---------------------------------------------------------------------------
// local time resolution
// ---------------------------------------------------------------------------

/// Minutes since local midnight, 0..1440. Resolves the timezone offset by:
/// 1. `TZ` env var if it's a fixed-offset string like `UTC`, `UTC+5:30`, `+05:30`.
/// 2. `/etc/timezone` (an IANA name, e.g. `Europe/London`) → looked up via
///    `date +%z` (we shell out, no IANA db in the binary).
/// 3. Best effort: assume UTC.
pub fn local_now_minutes() -> Result<u32> {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let offset_min = local_offset_minutes_signed();
    let local_secs = (secs as i64) + (offset_min as i64) * 60;
    let day_secs = local_secs.rem_euclid(86_400) as u32;
    Ok(day_secs / 60)
}

/// Resolve the local UTC offset in minutes. Positive east of UTC.
fn local_offset_minutes_signed() -> i32 {
    if let Ok(tz) = std::env::var("TZ") {
        if let Some(o) = parse_fixed_offset(&tz) {
            return o;
        }
    }
    // `date +%z` formats as `+HHMM` / `-HHMM`. Cheap, dependency-free, and
    // honours whatever the system thinks "now" is (incl. DST).
    if let Ok(out) = std::process::Command::new("date").arg("+%z").output() {
        if out.status.success() {
            let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
            if let Some(o) = parse_fixed_offset(&s) {
                return o;
            }
        }
    }
    0
}

/// Parse `+HHMM`, `-HHMM`, `+HH:MM`, `-HH:MM`, or `UTC` into minutes-east.
fn parse_fixed_offset(s: &str) -> Option<i32> {
    let t = s.trim();
    if t.eq_ignore_ascii_case("UTC") || t.is_empty() || t == "Z" {
        return Some(0);
    }
    let (sign, rest) = match t.as_bytes().first() {
        Some(b'+') => (1, &t[1..]),
        Some(b'-') => (-1, &t[1..]),
        _ => return None,
    };
    let rest = rest.replace(':', "");
    if rest.len() < 4 {
        return None;
    }
    let hh: i32 = rest[..2].parse().ok()?;
    let mm: i32 = rest[2..4].parse().ok()?;
    Some(sign * (hh * 60 + mm))
}

// ---------------------------------------------------------------------------
// path
// ---------------------------------------------------------------------------

/// Convenience re-export so callers don't need to drill into `cost`.
pub fn config_path() -> PathBuf {
    cost::config_path()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_hhmm_valid() {
        assert_eq!(parse_hhmm("00:00").expect("00:00"), 0);
        assert_eq!(parse_hhmm("23:59").expect("23:59"), 23 * 60 + 59);
        assert_eq!(parse_hhmm("07:30").expect("07:30"), 7 * 60 + 30);
    }

    #[test]
    fn parse_hhmm_invalid() {
        assert!(parse_hhmm("24:00").is_err());
        assert!(parse_hhmm("12:60").is_err());
        assert!(parse_hhmm("12").is_err());
        assert!(parse_hhmm("aa:bb").is_err());
    }

    #[test]
    fn in_day_window() {
        // 09:00 - 17:00
        let s = 9 * 60;
        let e = 17 * 60;
        assert!(!is_active(8 * 60, s, e));
        assert!(is_active(9 * 60, s, e));
        assert!(is_active(12 * 60, s, e));
        assert!(!is_active(17 * 60, s, e)); // exclusive
        assert!(!is_active(20 * 60, s, e));
    }

    #[test]
    fn cross_midnight_window() {
        // 22:00 - 07:00
        let s = 22 * 60;
        let e = 7 * 60;
        assert!(is_active(22 * 60, s, e));
        assert!(is_active(23 * 60, s, e));
        assert!(is_active(2 * 60, s, e));
        assert!(is_active(6 * 60 + 59, s, e));
        assert!(!is_active(7 * 60, s, e));
        assert!(!is_active(12 * 60, s, e));
        assert!(!is_active(20 * 60, s, e));
    }

    #[test]
    fn empty_window_when_equal() {
        assert!(!is_active(12 * 60, 12 * 60, 12 * 60));
    }

    #[test]
    fn parse_fixed_offset_shapes() {
        assert_eq!(parse_fixed_offset("+0530"), Some(330));
        assert_eq!(parse_fixed_offset("+05:30"), Some(330));
        assert_eq!(parse_fixed_offset("-0800"), Some(-480));
        assert_eq!(parse_fixed_offset("-08:00"), Some(-480));
        assert_eq!(parse_fixed_offset("UTC"), Some(0));
        assert_eq!(parse_fixed_offset("Z"), Some(0));
        assert_eq!(parse_fixed_offset(""), Some(0));
        assert_eq!(parse_fixed_offset("garbage"), None);
    }
}
