//! Integration tests for `deputyctl quiet-hours`.
//!
//! Schedule lives in the same `cost.toml` as the cost guardrails, so we
//! exercise the round-trip through `cost::save_config` /
//! `cost::load_config`. The pure gate `quiet_hours::is_active` is the
//! load-bearing function and gets the most coverage.

use std::sync::Mutex;

use deputyctl::cost::{self, CostConfig};
use deputyctl::quiet_hours::{self, is_active, parse_hhmm, run_set, SetOpts};

fn env_lock() -> &'static Mutex<()> {
    static M: std::sync::OnceLock<Mutex<()>> = std::sync::OnceLock::new();
    M.get_or_init(|| Mutex::new(()))
}

#[test]
fn schedule_round_trip_via_config() {
    let _g = env_lock().lock().unwrap_or_else(|e| e.into_inner());
    let dir = tempfile::tempdir().expect("tempdir");
    let cfg_path = dir.path().join("cost.toml");
    std::env::set_var("DEPUTYOS_COST_CONFIG", &cfg_path);

    let code = run_set(SetOpts {
        start: Some("22:00".into()),
        end: Some("07:00".into()),
        enable: true,
        disable: false,
        behaviour: Some("pause".into()),
    })
    .expect("run_set");
    assert_eq!(code, 0);

    let cfg = cost::load_config_from(&cfg_path).expect("load");
    assert!(cfg.quiet_hours.enabled);
    assert_eq!(cfg.quiet_hours.start, "22:00");
    assert_eq!(cfg.quiet_hours.end, "07:00");
    assert_eq!(cfg.quiet_hours.behaviour, "pause");

    std::env::remove_var("DEPUTYOS_COST_CONFIG");
}

#[test]
fn schedule_disable_clears_enabled() {
    let _g = env_lock().lock().unwrap_or_else(|e| e.into_inner());
    let dir = tempfile::tempdir().expect("tempdir");
    let cfg_path = dir.path().join("cost.toml");
    std::env::set_var("DEPUTYOS_COST_CONFIG", &cfg_path);

    run_set(SetOpts {
        start: Some("22:00".into()),
        end: Some("07:00".into()),
        enable: true,
        disable: false,
        behaviour: None,
    })
    .expect("enable");
    run_set(SetOpts {
        start: None,
        end: None,
        enable: false,
        disable: true,
        behaviour: None,
    })
    .expect("disable");

    let cfg = cost::load_config_from(&cfg_path).expect("load");
    assert!(!cfg.quiet_hours.enabled);
    // Schedule should still be remembered.
    assert_eq!(cfg.quiet_hours.start, "22:00");
    assert_eq!(cfg.quiet_hours.end, "07:00");

    std::env::remove_var("DEPUTYOS_COST_CONFIG");
}

#[test]
fn is_active_in_day() {
    // 09:00 - 17:00 → active during the workday.
    let s = parse_hhmm("09:00").expect("hhmm");
    let e = parse_hhmm("17:00").expect("hhmm");
    assert!(is_active(parse_hhmm("12:00").expect("hhmm"), s, e));
    assert!(is_active(parse_hhmm("09:01").expect("hhmm"), s, e));
    assert!(!is_active(parse_hhmm("08:59").expect("hhmm"), s, e));
    assert!(!is_active(parse_hhmm("17:00").expect("hhmm"), s, e));
    assert!(!is_active(parse_hhmm("23:00").expect("hhmm"), s, e));
}

#[test]
fn is_active_cross_midnight() {
    // 22:00 - 07:00 → active overnight.
    let s = parse_hhmm("22:00").expect("hhmm");
    let e = parse_hhmm("07:00").expect("hhmm");
    assert!(is_active(parse_hhmm("02:00").expect("hhmm"), s, e));
    assert!(is_active(parse_hhmm("23:30").expect("hhmm"), s, e));
    assert!(is_active(parse_hhmm("06:59").expect("hhmm"), s, e));
    assert!(!is_active(parse_hhmm("12:00").expect("hhmm"), s, e));
    assert!(!is_active(parse_hhmm("07:00").expect("hhmm"), s, e));
    assert!(!is_active(parse_hhmm("21:59").expect("hhmm"), s, e));
}

#[test]
fn is_active_now_off_when_disabled() {
    let mut cfg = CostConfig::default();
    cfg.quiet_hours.enabled = false;
    cfg.quiet_hours.start = "00:00".into();
    cfg.quiet_hours.end = "23:59".into();
    // Even with a 24h window, disabled = inactive.
    assert!(!quiet_hours::is_active_now(&cfg).expect("active"));
}

#[test]
fn rejects_bad_hhmm() {
    let _g = env_lock().lock().unwrap_or_else(|e| e.into_inner());
    let dir = tempfile::tempdir().expect("tempdir");
    let cfg_path = dir.path().join("cost.toml");
    std::env::set_var("DEPUTYOS_COST_CONFIG", &cfg_path);

    let res = run_set(SetOpts {
        start: Some("25:00".into()),
        end: None,
        enable: false,
        disable: false,
        behaviour: None,
    });
    assert!(res.is_err(), "should reject 25:00");

    std::env::remove_var("DEPUTYOS_COST_CONFIG");
}

#[test]
fn rejects_bad_behaviour() {
    let _g = env_lock().lock().unwrap_or_else(|e| e.into_inner());
    let dir = tempfile::tempdir().expect("tempdir");
    let cfg_path = dir.path().join("cost.toml");
    std::env::set_var("DEPUTYOS_COST_CONFIG", &cfg_path);

    let res = run_set(SetOpts {
        start: None,
        end: None,
        enable: false,
        disable: false,
        behaviour: Some("nope".into()),
    });
    assert!(res.is_err(), "should reject unknown behaviour");

    std::env::remove_var("DEPUTYOS_COST_CONFIG");
}

#[test]
fn enable_and_disable_mutually_exclusive() {
    let _g = env_lock().lock().unwrap_or_else(|e| e.into_inner());
    let dir = tempfile::tempdir().expect("tempdir");
    let cfg_path = dir.path().join("cost.toml");
    std::env::set_var("DEPUTYOS_COST_CONFIG", &cfg_path);

    let res = run_set(SetOpts {
        start: None,
        end: None,
        enable: true,
        disable: true,
        behaviour: None,
    });
    assert!(res.is_err());

    std::env::remove_var("DEPUTYOS_COST_CONFIG");
}
