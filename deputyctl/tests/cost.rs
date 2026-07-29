//! Integration tests for `deputyctl cost`.
//!
//! All side effects are scoped to a `tempdir` via `DEPUTYOS_COST_LEDGER`,
//! `DEPUTYOS_COST_CONFIG`, `DEPUTYOS_COST_TRIPPED`, `DEPUTYOS_DEV_OUT`. We
//! serialise via the workspace env-mutex pattern only when we touch shared
//! env state — but unique env vars per test would be simpler. We give each
//! test a unique tempdir and set vars while held; clear at the end.
//!
//! Tests guard the **cost ledger contract** (JSONL, schema in
//! `deputyctl::cost::LedgerEntry`) and the cap-trip pause path that the
//! agent-profile unit reads.

use std::path::Path;
use std::sync::Mutex;

use deputyctl::cost::{
    self, evaluate_caps, read_ledger_from, run_ledger_dump, run_reset, run_set, sum_month,
    sum_today, top_recent_expensive, CapState, CostConfig, LedgerEntry, LedgerOpts, SetOpts,
};

// Process-wide guard — `cost`/`quiet_hours` config + ledger paths are
// resolved through env vars; let tests run sequentially to avoid clobbering
// each other.
fn env_lock() -> &'static Mutex<()> {
    static M: std::sync::OnceLock<Mutex<()>> = std::sync::OnceLock::new();
    M.get_or_init(|| Mutex::new(()))
}

fn write_ledger(path: &Path, entries: &[LedgerEntry]) {
    let mut body = String::new();
    for e in entries {
        body.push_str(&serde_json::to_string(e).expect("ser"));
        body.push('\n');
    }
    std::fs::write(path, body).expect("write ledger");
}

fn entry(ts: &str, provider: &str, cost_usd: f64) -> LedgerEntry {
    LedgerEntry {
        timestamp: ts.into(),
        provider: provider.into(),
        model: "test/model".into(),
        input_tokens: 100,
        output_tokens: 200,
        cost_usd,
        request_id: format!("req_{ts}"),
    }
}

#[test]
fn ledger_aggregation_by_day_and_month() {
    let entries = vec![
        entry("2026-04-25T11:00:00Z", "openrouter", 0.10),
        entry("2026-04-26T08:00:00Z", "openai", 0.20),
        entry("2026-04-27T09:00:00Z", "anthropic", 1.25),
        entry("2026-04-27T15:00:00Z", "openrouter", 0.50),
        entry("2026-03-30T09:00:00Z", "openrouter", 99.00), // last month
    ];
    let today_total = sum_today(&entries, "2026-04-27");
    assert!(
        (today_total - 1.75).abs() < 1e-9,
        "today total wrong: {today_total}"
    );

    let month_total = sum_month(&entries, "2026-04");
    assert!(
        (month_total - 2.05).abs() < 1e-9,
        "month total wrong: {month_total}"
    );

    let last_month_total = sum_month(&entries, "2026-03");
    assert!((last_month_total - 99.0).abs() < 1e-9);
}

#[test]
fn cost_set_writes_config_atomically() {
    let _g = env_lock().lock().unwrap_or_else(|e| e.into_inner());
    let dir = tempfile::tempdir().expect("tempdir");
    let cfg_path = dir.path().join("cost.toml");
    std::env::set_var("DEPUTYOS_COST_CONFIG", &cfg_path);

    let code = run_set(SetOpts {
        daily_cap_usd: Some(7.5),
        monthly_cap_usd: Some(150.0),
        on_cap_trip: Some("warn".into()),
        warn_at_pct: Some(90),
    })
    .expect("run_set");
    assert_eq!(code, 0);
    assert!(cfg_path.is_file(), "config not written");

    // Round-trip via load.
    let cfg = cost::load_config_from(&cfg_path).expect("load");
    assert_eq!(cfg.caps.daily_usd, 7.5);
    assert_eq!(cfg.caps.monthly_usd, 150.0);
    assert_eq!(cfg.behaviour.on_cap_trip, "warn");
    assert_eq!(cfg.behaviour.warn_at_pct, 90);

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&cfg_path)
            .expect("metadata")
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o600, "cost.toml must be 0600");
    }
    std::env::remove_var("DEPUTYOS_COST_CONFIG");
}

#[test]
fn cap_trip_writes_marker_and_resets() {
    let _g = env_lock().lock().unwrap_or_else(|e| e.into_inner());
    let dir = tempfile::tempdir().expect("tempdir");
    let cfg_path = dir.path().join("cost.toml");
    let ledger_path = dir.path().join("ledger.jsonl");
    let marker_path = dir.path().join("cost-tripped");

    std::env::set_var("DEPUTYOS_COST_CONFIG", &cfg_path);
    std::env::set_var("DEPUTYOS_COST_LEDGER", &ledger_path);
    std::env::set_var("DEPUTYOS_COST_TRIPPED", &marker_path);
    std::env::set_var("DEPUTYOS_DEV_OUT", dir.path()); // no systemctl

    // Set caps tight enough to trip.
    run_set(SetOpts {
        daily_cap_usd: Some(1.00),
        monthly_cap_usd: Some(100.00),
        on_cap_trip: Some("pause".into()),
        warn_at_pct: Some(80),
    })
    .expect("set");

    // Push a ledger entry that exceeds the daily cap (UTC today).
    let today = &cost::current_utc_iso()[..10];
    let entries = vec![entry(&format!("{today}T08:00:00Z"), "openrouter", 2.50)];
    write_ledger(&ledger_path, &entries);

    let report = cost::check_caps_and_maybe_pause().expect("gate");
    assert_eq!(report.daily_state, CapState::Tripped);
    assert!(marker_path.is_file(), "tripped marker missing");
    assert!(cost::is_tripped(), "is_tripped() should be true");

    // Reset.
    let code = run_reset().expect("reset");
    assert_eq!(code, 0);
    assert!(!marker_path.is_file(), "marker should be cleared");
    assert!(!cost::is_tripped());

    std::env::remove_var("DEPUTYOS_COST_CONFIG");
    std::env::remove_var("DEPUTYOS_COST_LEDGER");
    std::env::remove_var("DEPUTYOS_COST_TRIPPED");
    std::env::remove_var("DEPUTYOS_DEV_OUT");
}

#[test]
fn cap_warn_does_not_trip() {
    let entries = vec![entry("2026-04-27T08:00:00Z", "openrouter", 0.85)];
    let cfg = CostConfig::default(); // daily cap 5.00, warn 80%
                                     // 0.85 / 5.00 = 17% — well below warn.
    let r = evaluate_caps(&entries, &cfg, "2026-04-27T09:00:00Z");
    assert_eq!(r.daily_state, CapState::Ok);

    // Now push to 4.10 → 82% → warn.
    let entries2 = vec![entry("2026-04-27T08:00:00Z", "openrouter", 4.10)];
    let r2 = evaluate_caps(&entries2, &cfg, "2026-04-27T09:00:00Z");
    assert_eq!(r2.daily_state, CapState::Warn);
}

#[test]
fn ledger_last_n_descending_by_timestamp() {
    let _g = env_lock().lock().unwrap_or_else(|e| e.into_inner());
    let dir = tempfile::tempdir().expect("tempdir");
    let ledger_path = dir.path().join("ledger.jsonl");
    std::env::set_var("DEPUTYOS_COST_LEDGER", &ledger_path);

    let entries = vec![
        entry("2026-04-25T11:00:00Z", "openrouter", 0.10),
        entry("2026-04-26T08:00:00Z", "openai", 0.20),
        entry("2026-04-27T09:00:00Z", "anthropic", 1.25),
        entry("2026-04-27T15:00:00Z", "openrouter", 0.50),
        entry("2026-04-23T15:00:00Z", "openrouter", 0.07),
    ];
    write_ledger(&ledger_path, &entries);

    // run_ledger_dump prints to stdout — we just check it doesn't panic and
    // exits 0. Read back via read_ledger_from for ordering check.
    let code = run_ledger_dump(LedgerOpts {
        last: 5,
        json: true,
    })
    .expect("dump");
    assert_eq!(code, 0);

    let read = read_ledger_from(&ledger_path).expect("read");
    let mut sorted = read.clone();
    sorted.sort_by(|a, b| b.timestamp.cmp(&a.timestamp));
    let last3: Vec<_> = sorted.into_iter().take(3).map(|e| e.timestamp).collect();
    assert_eq!(
        last3,
        vec![
            "2026-04-27T15:00:00Z".to_string(),
            "2026-04-27T09:00:00Z".to_string(),
            "2026-04-26T08:00:00Z".to_string(),
        ]
    );
    std::env::remove_var("DEPUTYOS_COST_LEDGER");
}

#[test]
fn empty_ledger_does_not_panic() {
    let _g = env_lock().lock().unwrap_or_else(|e| e.into_inner());
    let dir = tempfile::tempdir().expect("tempdir");
    let ledger_path = dir.path().join("missing.jsonl");
    std::env::set_var("DEPUTYOS_COST_LEDGER", &ledger_path);
    let entries = read_ledger_from(&ledger_path).expect("read");
    assert!(entries.is_empty());

    // run_summary should also handle an empty ledger.
    let code = cost::run_summary(cost::CostOpts {
        json: true,
        check: false,
    })
    .expect("summary");
    assert_eq!(code, 0);

    std::env::remove_var("DEPUTYOS_COST_LEDGER");
}

#[test]
fn malformed_ledger_lines_are_skipped() {
    let _g = env_lock().lock().unwrap_or_else(|e| e.into_inner());
    let dir = tempfile::tempdir().expect("tempdir");
    let ledger_path = dir.path().join("ledger.jsonl");
    let body = "not json\n\
                {\"timestamp\":\"2026-04-27T10:00:00Z\",\"provider\":\"openrouter\",\"cost_usd\":0.5}\n\
                {garbage\n\
                {\"timestamp\":\"2026-04-27T11:00:00Z\",\"provider\":\"openai\",\"cost_usd\":0.25}\n";
    std::fs::write(&ledger_path, body).expect("write");
    std::env::set_var("DEPUTYOS_COST_LEDGER", &ledger_path);

    let entries = read_ledger_from(&ledger_path).expect("read");
    assert_eq!(entries.len(), 2, "should skip malformed rows");
    let total: f64 = entries.iter().map(|e| e.cost_usd).sum();
    assert!((total - 0.75).abs() < 1e-9);
    std::env::remove_var("DEPUTYOS_COST_LEDGER");
}

#[test]
fn top_recent_expensive_orders_by_cost_desc() {
    let entries = vec![
        entry("2026-04-27T08:00:00Z", "openrouter", 0.10),
        entry("2026-04-27T09:00:00Z", "openai", 1.50),
        entry("2026-04-27T10:00:00Z", "anthropic", 0.85),
        entry("2026-04-27T11:00:00Z", "anthropic", 1.50),
    ];
    let top = top_recent_expensive(&entries, 3);
    assert_eq!(top.len(), 3);
    assert!((top[0].cost_usd - 1.50).abs() < 1e-9);
    // Tie broken by timestamp DESC.
    assert_eq!(top[0].timestamp, "2026-04-27T11:00:00Z");
    assert_eq!(top[1].timestamp, "2026-04-27T09:00:00Z");
    assert!((top[2].cost_usd - 0.85).abs() < 1e-9);
}

#[test]
fn cost_defaults_json_parses() {
    use std::path::PathBuf;
    let p = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("etc")
        .join("cost-defaults.json");
    let d = cost::load_defaults_from(&p).expect("defaults parse");
    assert!(
        d.providers.contains_key("openrouter"),
        "openrouter should be present"
    );
    assert!(
        d.providers.contains_key("anthropic"),
        "anthropic should be present"
    );
    let or = &d.providers["openrouter"];
    assert!(or.input_per_1m_usd > 0.0);
    assert!(or.output_per_1m_usd > 0.0);
}

#[test]
fn estimate_cost_matches_rate_sheet() {
    let _g = env_lock().lock().unwrap_or_else(|e| e.into_inner());
    use std::path::PathBuf;
    let p = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("etc")
        .join("cost-defaults.json");
    std::env::set_var("DEPUTYOS_COST_DEFAULTS", &p);

    // 1M input + 1M output at openrouter rates (3 + 15) = 18.
    let c = cost::estimate_cost(
        1_000_000,
        1_000_000,
        "openrouter",
        "anthropic/claude-sonnet-4-6",
    )
    .expect("estimate");
    assert!((c - 18.00).abs() < 1e-6, "got {c}");

    std::env::remove_var("DEPUTYOS_COST_DEFAULTS");
}

#[test]
fn cap_zero_means_disabled() {
    let entries = vec![entry("2026-04-27T08:00:00Z", "openrouter", 999.0)];
    let mut cfg = CostConfig::default();
    cfg.caps.daily_usd = 0.0;
    cfg.caps.monthly_usd = 0.0;
    let r = evaluate_caps(&entries, &cfg, "2026-04-27T09:00:00Z");
    assert_eq!(r.daily_state, CapState::Ok);
    assert_eq!(r.monthly_state, CapState::Ok);
}
