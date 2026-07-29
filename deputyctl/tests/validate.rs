//! Integration tests for `deputyctl profile validate`.
//!
//! Covers both happy-path (real profiles in `profiles/`) and the semantic
//! invariants that the struct-level deserializer can't reject on its own.

use std::path::PathBuf;

use deputyctl::validate::{validate_profile_file, ValidationError};

fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("deputyctl/ has a parent")
        .to_path_buf()
}

fn write_tmp(name: &str, contents: &str) -> tempfile::TempDir {
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::write(dir.path().join(name), contents).expect("write tmp profile");
    dir
}

fn has_field(errs: &[ValidationError], field: &str) -> bool {
    errs.iter().any(|e| e.field == field)
}

const VALID_OPENCLAW: &str = r#"
[profile]
id              = "openclaw"
display_name    = "OpenClaw"
upstream_repo   = "openclaw/openclaw"
release_channel = "stable"
min_ram_mb      = 4096
pinned_version  = "2026.4.25"

[paths]
install_root = "/opt/deputyos/profiles/openclaw"
data_dir     = "/home/agent/.openclaw"
binary       = "/opt/deputyos/profiles/openclaw/node_modules/.bin/openclaw"

[runtime]
language        = "node"
node_version    = "24"
package_manager = "npm"

[service]
unit       = "openclaw-gateway.service"
entrypoint = "openclaw onboard --daemon"
ports      = [8080]

[health]
http_check      = "http://127.0.0.1:8080/healthz"
journal_unit    = "openclaw-gateway.service"
startup_grace_s = 30

[apparmor]
profile = "/etc/apparmor.d/deputyos.openclaw"

[channels]
supported = ["telegram"]
"#;

#[test]
fn validates_real_profiles_pass() {
    for id in ["openclaw", "hermes"] {
        let path = workspace_root().join("profiles").join(format!("{id}.toml"));
        let errs = validate_profile_file(&path);
        assert!(
            errs.is_empty(),
            "real profile {id} must validate cleanly; errors: {errs:?}"
        );
    }
}

#[test]
fn rejects_bad_id() {
    let bad = VALID_OPENCLAW.replace(
        r#"id              = "openclaw""#,
        r#"id              = "BadID""#,
    );
    // Filename must match the (bad) id, otherwise we'd also see the filename mismatch error.
    let dir = write_tmp("BadID.toml", &bad);
    let errs = validate_profile_file(&dir.path().join("BadID.toml"));
    assert!(has_field(&errs, "profile.id"), "errors: {errs:?}");
}

#[test]
fn rejects_mismatched_filename() {
    // id = "openclaw" but file is foo.toml
    let dir = write_tmp("foo.toml", VALID_OPENCLAW);
    let errs = validate_profile_file(&dir.path().join("foo.toml"));
    assert!(
        errs.iter()
            .any(|e| e.field == "profile.id" && e.reason.contains("doesn't match filename")),
        "errors: {errs:?}",
    );
}

#[test]
fn rejects_unsupported_language() {
    let bad = VALID_OPENCLAW.replace(r#"language        = "node""#, r#"language        = "ruby""#);
    let dir = write_tmp("openclaw.toml", &bad);
    let errs = validate_profile_file(&dir.path().join("openclaw.toml"));
    assert!(has_field(&errs, "runtime.language"), "errors: {errs:?}");
}

#[test]
fn rejects_empty_channels() {
    let bad = VALID_OPENCLAW.replace(r#"supported = ["telegram"]"#, r#"supported = []"#);
    let dir = write_tmp("openclaw.toml", &bad);
    let errs = validate_profile_file(&dir.path().join("openclaw.toml"));
    assert!(has_field(&errs, "channels.supported"), "errors: {errs:?}");
}

#[test]
fn rejects_install_root_outside_convention() {
    let bad = VALID_OPENCLAW.replace(
        r#"install_root = "/opt/deputyos/profiles/openclaw""#,
        r#"install_root = "/usr/local/openclaw""#,
    );
    let dir = write_tmp("openclaw.toml", &bad);
    let errs = validate_profile_file(&dir.path().join("openclaw.toml"));
    assert!(has_field(&errs, "paths.install_root"), "errors: {errs:?}");
}

#[test]
fn rejects_node_without_version() {
    let bad = VALID_OPENCLAW.replace(r#"node_version    = "24""#, "");
    let dir = write_tmp("openclaw.toml", &bad);
    let errs = validate_profile_file(&dir.path().join("openclaw.toml"));
    assert!(has_field(&errs, "runtime.node_version"), "errors: {errs:?}");
}
