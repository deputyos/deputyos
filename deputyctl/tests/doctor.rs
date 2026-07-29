//! Unit-style tests for the doctor module: outcome semantics + parsing helpers.

use deputyctl::doctor::{parse_meminfo_total_kb, parse_sysctl_int, CheckOutcome};

#[test]
fn outcome_labels_and_fail_predicate() {
    assert_eq!(CheckOutcome::Pass.label(), "PASS");
    assert_eq!(CheckOutcome::Warn("x".into()).label(), "WARN");
    assert_eq!(CheckOutcome::Fail("x".into()).label(), "FAIL");
    assert_eq!(CheckOutcome::Skip("x".into()).label(), "SKIP");

    assert!(!CheckOutcome::Pass.is_fail());
    assert!(!CheckOutcome::Warn("x".into()).is_fail());
    assert!(CheckOutcome::Fail("x".into()).is_fail());
    assert!(!CheckOutcome::Skip("x".into()).is_fail());

    assert_eq!(CheckOutcome::Pass.detail(), "");
    assert_eq!(CheckOutcome::Warn("hi".into()).detail(), "hi");
}

#[test]
fn sysctl_int_parses_leading_token() {
    assert_eq!(parse_sysctl_int("1\n"), Some(1));
    assert_eq!(parse_sysctl_int("  2  \n"), Some(2));
    assert_eq!(parse_sysctl_int("1 2 3\n"), Some(1));
    assert_eq!(parse_sysctl_int("\n"), None);
    assert_eq!(parse_sysctl_int("hello\n"), None);
}

#[test]
fn meminfo_parser_finds_total() {
    let raw = "MemTotal:        4030428 kB\nMemFree:         1232980 kB\n";
    assert_eq!(parse_meminfo_total_kb(raw), Some(4030428));
    assert_eq!(parse_meminfo_total_kb("MemFree: 1 kB\n"), None);
    assert_eq!(parse_meminfo_total_kb(""), None);
}

#[test]
fn run_all_does_not_panic() {
    // The whole point: even on a non-Linux dev host or a fresh box, this
    // returns a structured report. No panics. No unwraps that escape.
    let report = deputyctl::doctor::run_all();
    assert_eq!(report.checks.len(), deputyctl::doctor::all_checks().len());
}
