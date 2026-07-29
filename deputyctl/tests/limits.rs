//! Integration test: the sample qemu-aarch64 limits file deserializes cleanly.
//!
//! Lane B will copy a target-specific JSON to /etc/deputyos/limits.json at bake
//! time. This test guards the schema for the dev/smoke fallback we ship in
//! `deputyctl/etc/limits.qemu-aarch64.json`.

use std::path::PathBuf;

fn sample_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("etc")
        .join("limits.qemu-aarch64.json")
}

#[test]
fn qemu_aarch64_sample_parses() {
    let path = sample_path();
    let l = deputyctl::limits::load_from(&path).expect("sample limits.json must parse");
    assert_eq!(l.target, "qemu-aarch64");
    assert_eq!(l.tier, "standard");
    assert_eq!(l.ram_mb, 4096);
    assert!(!l.capabilities.local_llm);
    assert!(l.limitations.iter().any(|lim| lim.id == "no-local-llm"));
}

#[test]
fn human_format_mentions_target_and_unblock() {
    let l = deputyctl::limits::load_from(&sample_path()).expect("parse");
    let s = deputyctl::limits::format_human(&l);
    assert!(s.contains("qemu-aarch64"));
    assert!(s.contains("Unblock:"));
    assert!(s.contains("CANNOT do"));
}
