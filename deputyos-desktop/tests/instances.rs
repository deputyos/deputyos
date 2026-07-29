//! Multi-instance integration test for the Linux driver.
//!
//! Companion to `tests/integration.rs` (which exercises the single-instance
//! env-var path). Here we create **two** named instances, each with its own
//! port pair + cache dir + runtime dir, and assert both VMs run concurrently
//! and independently — `stop` on one does not affect the other. This is the
//! load-bearing behaviour the desktop console relies on.
//!
//! Linux-only (the only driver with working `start`/`stop`), x86_64-only
//! (the fake qemu shim is named `qemu-system-x86_64`). No-op skip otherwise.

#![cfg(target_os = "linux")]

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use deputyos_desktop::driver::{Driver, VmStatus};
use deputyos_desktop::drivers::linux::LinuxDriver;
use deputyos_desktop::instance::{InstanceConfig, Registry};

/// Process-wide guard for env-var mutation (the registry + driver resolve
/// paths from `DEPUTYOS_DESKTOP_*` env vars).
fn env_lock() -> &'static Mutex<()> {
    use std::sync::OnceLock;
    static M: OnceLock<Mutex<()>> = OnceLock::new();
    M.get_or_init(|| Mutex::new(()))
}

fn host_is_x86_64() -> bool {
    std::env::consts::ARCH == "x86_64"
}

/// Stage a fake `qemu-system-x86_64` shim on PATH that sleeps until SIGTERM
/// (mirrors `tests/integration.rs::fake_qemu`).
fn fake_qemu(dir: &Path) -> PathBuf {
    let path = dir.join("qemu-system-x86_64");
    let mut f = fs::File::create(&path).expect("create");
    writeln!(f, "#!/usr/bin/env bash").expect("w");
    writeln!(f, "# fake qemu — sleeps until SIGTERM").expect("w");
    writeln!(f, "trap 'exit 0' TERM").expect("w");
    writeln!(f, "sleep 30 &").expect("w");
    writeln!(f, "wait $!").expect("w");
    drop(f);
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).expect("chmod");
    path
}

/// Stage an image so `start_with` finds `deputyos-<target>.qcow2` in the
/// instance's cache dir.
fn stage_image(cache_dir: &Path) {
    fs::create_dir_all(cache_dir).expect("cache");
    let image = cache_dir.join("deputyos-qemu-x86_64.qcow2");
    fs::write(&image, b"fake image bytes").expect("img");
}

/// Build an `InstanceConfig` for a named instance with distinct ports and a
/// temp cache/runtime dir, plus a staged image. Mirrors what the console's
/// `create_instance` produces.
fn make_cfg(dir: &Path, label: &str, wizard: u16, gateway: u16) -> InstanceConfig {
    let cache_dir = dir.join(label).join("cache");
    let runtime_dir = dir.join(label).join("run");
    stage_image(&cache_dir);
    InstanceConfig {
        wizard_port: wizard,
        gateway_port: gateway,
        cache_dir,
        runtime_dir,
        seed_iso: None,
        resources: deputyos_desktop::instance::ResourceSpec::default(),
    }
}

#[test]
fn two_instances_run_independently_and_stop_is_isolated() {
    if !host_is_x86_64() {
        eprintln!("skip: not x86_64");
        return;
    }
    let _g = env_lock().lock().expect("env_lock poisoned");

    let dir = tempfile::tempdir().expect("tempdir");

    // Fake qemu on PATH + the sh/kill/sleep the driver's stop/status need.
    let bin_dir = dir.path().join("bin");
    fs::create_dir(&bin_dir).expect("bin");
    fake_qemu(&bin_dir);
    for tool in &["sh", "bash", "kill", "sleep"] {
        let real = ["/usr/bin", "/bin"]
            .iter()
            .map(|p| PathBuf::from(p).join(tool))
            .find(|p| p.exists());
        if let Some(real) = real {
            let _ = std::os::unix::fs::symlink(&real, bin_dir.join(tool));
        }
    }
    let prev_path = std::env::var("PATH").unwrap_or_default();
    std::env::set_var(
        "PATH",
        format!("{}:{prev_path}", bin_dir.to_str().expect("utf8")),
    );

    let driver = LinuxDriver::new();

    // Two instances with distinct ports + cache/runtime dirs.
    let cfg_a = make_cfg(dir.path(), "a", 27088, 27080);
    let cfg_b = make_cfg(dir.path(), "b", 27188, 27180);

    // Both stopped initially.
    assert!(matches!(
        driver.status_with(&cfg_a).expect("status a"),
        VmStatus::Stopped
    ));
    assert!(matches!(
        driver.status_with(&cfg_b).expect("status b"),
        VmStatus::Stopped
    ));

    // Start A, then B.
    let h_a = driver.start_with(&cfg_a).expect("start a");
    let h_b = driver.start_with(&cfg_b).expect("start b");
    assert_ne!(h_a.id, h_b.id, "two instances must have distinct PIDs");
    std::thread::sleep(std::time::Duration::from_millis(150));

    // Both running, with distinct wizard URLs reflecting their ports.
    match driver.status_with(&cfg_a).expect("status a running") {
        VmStatus::Running { handle, urls } => {
            assert_eq!(handle.id, h_a.id);
            assert!(urls.iter().any(|u| u.contains("27088")), "a urls: {urls:?}");
        }
        VmStatus::Stopped | VmStatus::Paused { .. } => panic!("a should be running"),
    }
    match driver.status_with(&cfg_b).expect("status b running") {
        VmStatus::Running { handle, urls } => {
            assert_eq!(handle.id, h_b.id);
            assert!(urls.iter().any(|u| u.contains("27188")), "b urls: {urls:?}");
        }
        VmStatus::Stopped | VmStatus::Paused { .. } => panic!("b should be running"),
    }

    // Stop A; B must still be running.
    driver.stop_with(&cfg_a).expect("stop a");
    std::thread::sleep(std::time::Duration::from_millis(150));
    assert!(
        matches!(
            driver.status_with(&cfg_a).expect("status a after stop"),
            VmStatus::Stopped
        ),
        "a should be stopped"
    );
    assert!(
        matches!(
            driver.status_with(&cfg_b).expect("status b still running"),
            VmStatus::Running { .. }
        ),
        "b must be unaffected by stopping a"
    );

    // Teardown B.
    driver.stop_with(&cfg_b).expect("stop b");

    std::env::set_var("PATH", prev_path);
}

#[test]
fn registry_create_instance_allocates_distinct_ports_and_dirs() {
    let _g = env_lock().lock().expect("env_lock poisoned");
    let dir = tempfile::tempdir().expect("tempdir");
    // Point the data dir at a tempdir so the registry + per-instance dirs
    // land under it and don't touch the user's real data dir.
    std::env::set_var(
        "DEPUTYOS_DESKTOP_DATA_DIR",
        dir.path().to_str().expect("utf8"),
    );

    let reg = Registry::load().expect("empty registry");
    assert!(reg.instances.is_empty());

    let a = reg
        .create_instance("alpha", "qemu-x86_64", None, None, None)
        .expect("create alpha");
    let b = reg
        .create_instance("beta", "qemu-x86_64", Some("openclaw".into()), None, None)
        .expect("create beta");

    // Distinct ids, names, ports.
    assert_ne!(a.id, b.id);
    assert_ne!(a.name, b.name);
    assert_ne!(a.wizard_port, b.wizard_port);
    assert_ne!(a.gateway_port, b.gateway_port);
    assert_ne!(a.wizard_port, a.gateway_port);
    // Distinct per-instance dirs under data_dir/instances/<id>/.
    assert_ne!(a.cache_dir, b.cache_dir);
    assert_ne!(a.runtime_dir, b.runtime_dir);
    assert!(
        a.cache_dir.starts_with(dir.path()),
        "a cache under data dir"
    );
    assert!(b.runtime_dir.starts_with(dir.path()));
    assert_eq!(a.profile, None);
    assert_eq!(b.profile.as_deref(), Some("openclaw"));

    // Insert a + b, then a duplicate-name create must be rejected. (create_instance
    // is &self and does NOT insert — we upsert explicitly, then exercise the
    // collision path against a populated registry.)
    let mut reg2 = reg.clone();
    reg2.upsert(a.clone());
    reg2.upsert(b.clone());
    let err = reg2
        .create_instance("alpha", "qemu-x86_64", None, None, None)
        .expect_err("duplicate name must error");
    assert!(
        format!("{err:#}").contains("already exists"),
        "got: {err:#}"
    );

    // Round-trip through save/load.
    reg2.save().expect("save");
    let back = Registry::load().expect("load");
    assert_eq!(back.instances.len(), 2);
    assert!(back.get(&a.id).is_some());
    assert!(back.get(&b.id).is_some());

    // remove works.
    let mut reg3 = back.clone();
    assert!(reg3.remove(&a.id));
    assert!(reg3.get(&a.id).is_none());
    assert!(reg3.get(&b.id).is_some());

    std::env::remove_var("DEPUTYOS_DESKTOP_DATA_DIR");
}
