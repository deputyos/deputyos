//! Integration test: every real profile manifest in `profiles/` must parse.
//!
//! This is the load-bearing test of the manifest schema: if it fails, the
//! build pipeline cannot trust the structs in `deputyctl::manifest` to read
//! real manifests at runtime on a device.

use std::path::PathBuf;

fn profiles_dir() -> PathBuf {
    // CARGO_MANIFEST_DIR is the deputyctl/ dir; the profiles/ tree lives
    // one level up at the workspace root.
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("deputyctl/ has a parent")
        .join("profiles")
}

#[test]
fn openclaw_profile_parses() {
    let path = profiles_dir().join("openclaw.toml");
    let m = deputyctl::manifest::load(&path).expect("openclaw.toml must parse");
    assert_eq!(m.profile.id, "openclaw");
    assert_eq!(m.runtime.language, "node");
    assert_eq!(m.service.unit, "openclaw-gateway.service");
    assert!(m.service.ports.contains(&8080));
    // M4.5 Lane F: every shipped profile carries an [airgap] default provider.
    let airgap = m.airgap.expect("openclaw.toml has an [airgap] section");
    assert_eq!(airgap.default_provider, "local-llamacpp-airgap");
}

#[test]
fn hermes_profile_parses() {
    let path = profiles_dir().join("hermes.toml");
    let m = deputyctl::manifest::load(&path).expect("hermes.toml must parse");
    assert_eq!(m.profile.id, "hermes");
    assert_eq!(m.runtime.language, "python");
    assert_eq!(m.service.unit, "hermes-gateway.service");
    let kernel = m.kernel.expect("hermes has a [kernel] section");
    assert_eq!(
        kernel
            .required_sysctls
            .get("kernel.unprivileged_userns_clone"),
        Some(&"1".to_string())
    );
    // M3.5: profiles without a [mounts] section still load; mounts is None.
    assert!(m.mounts.is_none(), "hermes.toml has no [mounts] → None");
}

#[test]
fn mounts_section_parses_and_defaults_mode() {
    // A minimal manifest exercising the new optional [mounts] section (M3.5
    // Lane F). default_mode defaults to "ro" when omitted; suggested_paths
    // round-trips. Required sections are present so the whole manifest parses.
    let raw = r#"
[profile]
id = "test"
display_name = "Test"
upstream_repo = "https://example.com/repo"
release_channel = "stable"
min_ram_mb = 1024
pinned_version = "1.0.0"

[paths]
install_root = "/opt/deputyos/profiles/test"
data_dir = "/home/agent/.test"
binary = "/usr/local/bin/test"

[runtime]
language = "node"
package_manager = "npm"

[service]
unit = "test-gateway.service"
entrypoint = "/usr/local/bin/test"
ports = [8080]

[health]
http_check = "http://localhost:8080/health"
journal_unit = "test-gateway.service"
startup_grace_s = 30

[mounts]
suggested_paths = ["/mnt/deputyos/documents", "/mnt/deputyos/code"]
"#;
    let m = deputyctl::manifest::parse(raw).expect("manifest with [mounts] must parse");
    let mounts = m.mounts.expect("has [mounts]");
    assert_eq!(
        mounts.default_mode, "ro",
        "default_mode defaults to ro when omitted"
    );
    assert_eq!(
        mounts.suggested_paths,
        vec![
            "/mnt/deputyos/documents".to_string(),
            "/mnt/deputyos/code".to_string()
        ]
    );
}

#[test]
fn airgap_section_parses() {
    // M4.5 Lane F: the optional [airgap] section carries the default provider
    // the wizard pre-selects on an air-gapped build. Required sections are
    // present so the whole manifest parses.
    let raw = r#"
[profile]
id = "test"
display_name = "Test"
upstream_repo = "https://example.com/repo"
release_channel = "stable"
min_ram_mb = 1024
pinned_version = "1.0.0"

[paths]
install_root = "/opt/deputyos/profiles/test"
data_dir = "/home/agent/.test"
binary = "/usr/local/bin/test"

[runtime]
language = "node"
package_manager = "npm"

[service]
unit = "test-gateway.service"
entrypoint = "/usr/local/bin/test"
ports = [8080]

[health]
http_check = "http://localhost:8080/health"
journal_unit = "test-gateway.service"
startup_grace_s = 30

[airgap]
default_provider = "local-llamacpp-airgap"
"#;
    let m = deputyctl::manifest::parse(raw).expect("manifest with [airgap] must parse");
    let airgap = m.airgap.expect("has [airgap]");
    assert_eq!(airgap.default_provider, "local-llamacpp-airgap");
    // Profiles without [airgap] still load (None).
    let raw2 = raw.replace(
        "\n[airgap]\ndefault_provider = \"local-llamacpp-airgap\"\n",
        "",
    );
    let m2 = deputyctl::manifest::parse(&raw2).expect("manifest without [airgap] must parse");
    assert!(m2.airgap.is_none(), "no [airgap] -> None");
}

#[test]
fn egress_section_parses_and_defaults_mode() {
    // M5.5 Lane F: the optional [default_egress] section carries the mode the
    // wizard pre-selects + a starter allow-list shown as hints. mode defaults
    // to "open" when omitted. Required sections present so the whole parses.
    let raw = r#"
[profile]
id = "test"
display_name = "Test"
upstream_repo = "https://example.com/repo"
release_channel = "stable"
min_ram_mb = 1024
pinned_version = "1.0.0"

[paths]
install_root = "/opt/deputyos/profiles/test"
data_dir = "/home/agent/.test"
binary = "/usr/local/bin/test"

[runtime]
language = "node"
package_manager = "npm"

[service]
unit = "test-gateway.service"
entrypoint = "/usr/local/bin/test"
ports = [8080]

[health]
http_check = "http://localhost:8080/health"
journal_unit = "test-gateway.service"
startup_grace_s = 30

[default_egress]
allow_hosts = ["api.openai.com", "api.anthropic.com"]
"#;
    let m = deputyctl::manifest::parse(raw).expect("manifest with [default_egress] must parse");
    let egress = m.default_egress.expect("has [default_egress]");
    assert_eq!(egress.mode, "open", "mode defaults to open when omitted");
    assert_eq!(
        egress.allow_hosts,
        vec![
            "api.openai.com".to_string(),
            "api.anthropic.com".to_string()
        ]
    );

    // With an explicit mode it round-trips.
    let raw2 = raw.replace(
        "\n[default_egress]",
        "\n[default_egress]\nmode = \"whitelist\"",
    );
    let m2 = deputyctl::manifest::parse(&raw2).expect("manifest with explicit mode must parse");
    assert_eq!(
        m2.default_egress.as_ref().expect("has default_egress").mode,
        "whitelist"
    );

    // Profiles without [default_egress] still load (None).
    let raw3 = raw.replace(
        "\n[default_egress]\nallow_hosts = [\"api.openai.com\", \"api.anthropic.com\"]\n",
        "",
    );
    let m3 =
        deputyctl::manifest::parse(&raw3).expect("manifest without [default_egress] must parse");
    assert!(m3.default_egress.is_none(), "no [default_egress] -> None");
}
