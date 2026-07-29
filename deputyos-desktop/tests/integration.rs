//! End-to-end smoke for the Linux driver.
//!
//! Strategy: build a synthetic minisign keypair, sign a synthetic 1-MB
//! "image" + a generated manifest, serve them via `httpmock`, and stand
//! up a fake `qemu-system-x86_64` shell-script on PATH that just sleeps
//! and exits 0. Drive the launcher's library API end-to-end and assert
//! every observable side-effect.
//!
//! These tests only run on Linux (the only driver implemented today) and
//! only when `minisign` is on PATH (everywhere `make doctor` certifies).
//! They no-op with a `eprintln` skip on hosts that lack either.

#![cfg(target_os = "linux")]

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Mutex;

use deputyos_desktop::driver::{Driver, VmStatus};
use deputyos_desktop::drivers::linux::LinuxDriver;
use deputyos_desktop::{config, download, manifest as launcher_manifest, selfupdate};

/// Process-wide guard for env-var mutation. Tests in this file all touch
/// `DEPUTYOS_DESKTOP_*` env vars, so they must serialize.
fn env_lock() -> &'static Mutex<()> {
    use std::sync::OnceLock;
    static M: OnceLock<Mutex<()>> = OnceLock::new();
    M.get_or_init(|| Mutex::new(()))
}

/// Skip-marker — returned when minisign isn't available.
fn minisign_available() -> bool {
    Command::new("minisign")
        .arg("-v")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|_| true)
        .unwrap_or(false)
        || Command::new("minisign")
            .arg("-h")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .map(|_| true)
            .unwrap_or(false)
}

fn gen_keypair(dir: &Path) -> (PathBuf, PathBuf) {
    let pubkey = dir.join("k.pub");
    let seckey = dir.join("k.key");
    let status = Command::new("minisign")
        .args(["-G", "-W", "-p"])
        .arg(&pubkey)
        .arg("-s")
        .arg(&seckey)
        .status()
        .expect("minisign keygen");
    assert!(status.success(), "keygen");
    (pubkey, seckey)
}

fn sign_file(seckey: &Path, msg: &Path) {
    let status = Command::new("minisign")
        .args(["-S", "-W", "-s"])
        .arg(seckey)
        .arg("-m")
        .arg(msg)
        .status()
        .expect("minisign sign");
    assert!(status.success(), "sign");
}

fn sha256_hex(path: &Path) -> String {
    let out = Command::new("sha256sum")
        .arg(path)
        .output()
        .expect("sha256sum");
    let s = String::from_utf8_lossy(&out.stdout);
    s.split_whitespace().next().expect("hex").to_string()
}

/// Stage a minimal-but-valid `deputyctl::release::Manifest` with one
/// artefact whose target matches `target`.
fn write_manifest(
    dir: &Path,
    target: &str,
    image_filename: &str,
    image_sha256: &str,
    image_size: u64,
) -> PathBuf {
    let path = dir.join("manifest.json");
    let m = serde_json::json!({
        "schema_version": 1,
        "release_version": "2026.4.27",
        "channel": "dev",
        "released_at": "2026-04-27T00:00:00Z",
        "artefacts": [{
            "target": target,
            "profile": "openclaw",
            "filename": image_filename,
            "format": "qcow2",
            "size_bytes": image_size,
            "sha256": image_sha256,
            "minisig_url": format!("{image_filename}.minisig"),
            "url": image_filename,
        }],
    });
    fs::write(&path, serde_json::to_string_pretty(&m).expect("ser")).expect("write manifest");
    path
}

/// Make a fake qemu shim on PATH that just sleeps. Returns (path, dir guard).
fn fake_qemu(dir: &Path) -> PathBuf {
    let path = dir.join("qemu-system-x86_64");
    let mut f = fs::File::create(&path).expect("create");
    writeln!(f, "#!/usr/bin/env bash").expect("w");
    writeln!(f, "# fake qemu — sleeps until SIGTERM").expect("w");
    writeln!(f, "trap 'exit 0' TERM").expect("w");
    writeln!(f, "sleep 30 &").expect("w");
    writeln!(f, "wait $!").expect("w");
    drop(f);
    let mode = 0o755;
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(&path, fs::Permissions::from_mode(mode)).expect("chmod");
    path
}

fn host_is_x86_64() -> bool {
    std::env::consts::ARCH == "x86_64"
}

#[test]
fn install_downloads_and_verifies() {
    if !minisign_available() {
        eprintln!("skip: minisign not installed");
        return;
    }
    if !host_is_x86_64() {
        eprintln!("skip: host arch is not x86_64");
        return;
    }
    let _g = env_lock().lock().expect("env_lock poisoned");

    let dir = tempfile::tempdir().expect("tempdir");
    let keys_dir = dir.path().join("keys");
    fs::create_dir(&keys_dir).expect("keys");
    let (pubkey, seckey) = gen_keypair(&keys_dir);

    // Synthetic 1 MB image.
    let dist_dir = dir.path().join("dist");
    fs::create_dir(&dist_dir).expect("dist");
    let image_filename = "deputyos-openclaw-qemu-x86_64-2026.4.27-dev.qcow2";
    let image_path = dist_dir.join(image_filename);
    let mut f = fs::File::create(&image_path).expect("create image");
    let buf = vec![0xABu8; 1024 * 1024];
    f.write_all(&buf).expect("write image");
    drop(f);
    let img_sha = sha256_hex(&image_path);
    sign_file(&seckey, &image_path);

    // Manifest + manifest signature.
    let manifest_path = write_manifest(
        &dist_dir,
        "qemu-x86_64",
        image_filename,
        &img_sha,
        1024 * 1024,
    );
    sign_file(&seckey, &manifest_path);

    // Spin up httpmock serving everything in dist/.
    let server = httpmock::MockServer::start();
    let manifest_body = fs::read(&manifest_path).expect("read manifest");
    let manifest_sig = fs::read(format!("{}.minisig", manifest_path.display())).expect("sig");
    let image_body = fs::read(&image_path).expect("read image");
    let image_sig = fs::read(format!("{}.minisig", image_path.display())).expect("img sig");

    let _m1 = server.mock(|when, then| {
        when.method(httpmock::Method::GET).path("/manifest.json");
        then.status(200).body(manifest_body);
    });
    let _m2 = server.mock(|when, then| {
        when.method(httpmock::Method::GET)
            .path("/manifest.json.minisig");
        then.status(200).body(manifest_sig);
    });
    let _m3 = server.mock(|when, then| {
        when.method(httpmock::Method::GET)
            .path(format!("/{image_filename}"));
        then.status(200).body(image_body);
    });
    let _m4 = server.mock(|when, then| {
        when.method(httpmock::Method::GET)
            .path(format!("/{image_filename}.minisig"));
        then.status(200).body(image_sig);
    });

    let cache_dir = dir.path().join("cache");
    let runtime_dir = dir.path().join("run");
    let manifest_url = server.url("/manifest.json");

    std::env::set_var("DEPUTYOS_DESKTOP_PUBKEY", &pubkey);
    std::env::set_var("DEPUTYOS_DESKTOP_CACHE_DIR", &cache_dir);
    std::env::set_var("DEPUTYOS_DESKTOP_RUNTIME_DIR", &runtime_dir);
    std::env::set_var("DEPUTYOS_DESKTOP_MANIFEST_URL", &manifest_url);

    // Run the install pipeline directly via the library API.
    let src = launcher_manifest::fetch_and_verify(&manifest_url, &pubkey).expect("fetch+verify");
    let driver = LinuxDriver::new();
    let target = <LinuxDriver as Driver>::target_for_host(&driver);
    assert_eq!(target, "qemu-x86_64");
    let artefact = launcher_manifest::pick_artefact(&src, target, None).expect("artefact");
    let (img_url, sig_url) = launcher_manifest::artefact_urls(&src, artefact).expect("urls");
    let img_dest = cache_dir.join(format!("deputyos-{target}.qcow2"));
    let sig_dest = cache_dir.join(format!("deputyos-{target}.qcow2.minisig"));
    download::download_and_verify(
        &img_url,
        &sig_url,
        &img_dest,
        &sig_dest,
        &artefact.sha256,
        &pubkey,
    )
    .expect("download_and_verify");

    assert!(img_dest.is_file(), "image should be cached");
    assert!(sig_dest.is_file(), "sig should be cached");
    assert_eq!(sha256_hex(&img_dest), img_sha);

    // Cleanup env vars.
    std::env::remove_var("DEPUTYOS_DESKTOP_PUBKEY");
    std::env::remove_var("DEPUTYOS_DESKTOP_CACHE_DIR");
    std::env::remove_var("DEPUTYOS_DESKTOP_RUNTIME_DIR");
    std::env::remove_var("DEPUTYOS_DESKTOP_MANIFEST_URL");
}

#[test]
fn install_rejects_tampered_manifest() {
    if !minisign_available() {
        eprintln!("skip: minisign not installed");
        return;
    }
    let _g = env_lock().lock().expect("env_lock poisoned");

    let dir = tempfile::tempdir().expect("tempdir");
    let (pubkey, seckey) = gen_keypair(dir.path());
    let dist_dir = dir.path().join("dist");
    fs::create_dir(&dist_dir).expect("dist");

    // Sign a manifest, then tamper it AFTER signing.
    let image_filename = "deputyos-test.qcow2";
    let image_path = dist_dir.join(image_filename);
    fs::write(&image_path, b"x").expect("img");
    let img_sha = sha256_hex(&image_path);
    sign_file(&seckey, &image_path);

    let manifest_path = write_manifest(&dist_dir, "qemu-x86_64", image_filename, &img_sha, 1);
    sign_file(&seckey, &manifest_path);

    // Tamper.
    let mut tampered = fs::read(&manifest_path).expect("read");
    tampered.push(b' ');
    fs::write(&manifest_path, tampered).expect("write tampered");

    let server = httpmock::MockServer::start();
    let manifest_body = fs::read(&manifest_path).expect("read manifest");
    let manifest_sig = fs::read(format!("{}.minisig", manifest_path.display())).expect("sig");
    let _m1 = server.mock(|when, then| {
        when.method(httpmock::Method::GET).path("/manifest.json");
        then.status(200).body(manifest_body);
    });
    let _m2 = server.mock(|when, then| {
        when.method(httpmock::Method::GET)
            .path("/manifest.json.minisig");
        then.status(200).body(manifest_sig);
    });

    let manifest_url = server.url("/manifest.json");
    let err = launcher_manifest::fetch_and_verify(&manifest_url, &pubkey)
        .expect_err("tampered manifest must fail");
    let msg = format!("{err:#}");
    assert!(
        msg.contains("signature verification failed"),
        "expected sig-fail, got: {msg}"
    );
}

#[test]
fn check_prereq_errors_when_qemu_missing() {
    if !host_is_x86_64() {
        eprintln!("skip: not x86_64");
        return;
    }
    let _g = env_lock().lock().expect("env_lock poisoned");
    // Replace PATH with a minimal one missing qemu.
    let prev_path = std::env::var("PATH").unwrap_or_default();
    let dir = tempfile::tempdir().expect("tempdir");
    // Directory contains only sh + kill (need them for status/stop) — but
    // explicitly NO qemu-system-*.
    let bin_dir = dir.path().join("bin");
    fs::create_dir(&bin_dir).expect("bin");
    // Symlink common tools from /usr/bin so the rest of the test-suite
    // process keeps working.
    for tool in &["sh", "bash", "kill", "sleep"] {
        let src = PathBuf::from("/usr/bin").join(tool);
        let real = if src.exists() {
            src
        } else {
            PathBuf::from("/bin").join(tool)
        };
        if real.exists() {
            let _ = std::os::unix::fs::symlink(&real, bin_dir.join(tool));
        }
    }

    std::env::set_var("PATH", &bin_dir);
    let driver = LinuxDriver::new();
    let err = driver.check_prereq().expect_err("missing qemu must error");
    let msg = format!("{err:#}");
    assert!(
        msg.contains("qemu-system") && msg.contains("install"),
        "got: {msg}"
    );

    std::env::set_var("PATH", prev_path);
}

#[test]
fn start_stop_status_lifecycle_with_fake_qemu() {
    if !host_is_x86_64() {
        eprintln!("skip: not x86_64");
        return;
    }
    let _g = env_lock().lock().expect("env_lock poisoned");

    let dir = tempfile::tempdir().expect("tempdir");
    let bin_dir = dir.path().join("bin");
    fs::create_dir(&bin_dir).expect("bin");
    fake_qemu(&bin_dir);
    // Pull in /bin tools we still need.
    for tool in &["sh", "bash", "kill", "sleep"] {
        let real = ["/usr/bin", "/bin"]
            .iter()
            .map(|p| PathBuf::from(p).join(tool))
            .find(|p| p.exists());
        if let Some(real) = real {
            let _ = std::os::unix::fs::symlink(&real, bin_dir.join(tool));
        }
    }

    // Stage cache + image so start finds something.
    let cache_dir = dir.path().join("cache");
    fs::create_dir_all(&cache_dir).expect("cache");
    let image = cache_dir.join("deputyos-qemu-x86_64.qcow2");
    fs::write(&image, b"fake image bytes").expect("img");

    let runtime_dir = dir.path().join("run");

    let prev_path = std::env::var("PATH").unwrap_or_default();
    std::env::set_var(
        "PATH",
        format!("{}:{prev_path}", bin_dir.to_str().expect("utf8")),
    );
    std::env::set_var("DEPUTYOS_DESKTOP_CACHE_DIR", &cache_dir);
    std::env::set_var("DEPUTYOS_DESKTOP_RUNTIME_DIR", &runtime_dir);

    let driver = LinuxDriver::new();

    // 1. Status: stopped.
    let s = driver.status().expect("status");
    assert!(matches!(s, VmStatus::Stopped));

    // 2. Prereq passes (fake qemu on PATH).
    driver.check_prereq().expect("prereq ok");

    // 3. Start.
    let h = driver.start().expect("start");
    assert!(!h.id.is_empty());

    // Give qemu a beat to actually run.
    std::thread::sleep(std::time::Duration::from_millis(100));

    // 4. Status: running.
    let s = driver.status().expect("status running");
    match s {
        VmStatus::Running { handle, urls } => {
            assert_eq!(handle.id, h.id);
            assert!(urls.iter().any(|u| u.contains("8088")));
        }
        VmStatus::Stopped | VmStatus::Paused { .. } => panic!("expected running"),
    }

    // 5. Idempotent start: returns the same handle.
    let h2 = driver.start().expect("start again");
    assert_eq!(h2.id, h.id);

    // 6. Stop.
    driver.stop().expect("stop");
    // Stop is idempotent — calling again must not fail.
    driver.stop().expect("stop again (idempotent)");

    // 7. Final status: stopped.
    std::thread::sleep(std::time::Duration::from_millis(100));
    let s = driver.status().expect("status final");
    assert!(matches!(s, VmStatus::Stopped));

    std::env::set_var("PATH", prev_path);
    std::env::remove_var("DEPUTYOS_DESKTOP_CACHE_DIR");
    std::env::remove_var("DEPUTYOS_DESKTOP_RUNTIME_DIR");
}

#[test]
fn start_errors_when_image_not_installed() {
    if !host_is_x86_64() {
        eprintln!("skip: not x86_64");
        return;
    }
    let _g = env_lock().lock().expect("env_lock poisoned");
    let dir = tempfile::tempdir().expect("tempdir");
    let bin_dir = dir.path().join("bin");
    fs::create_dir(&bin_dir).expect("bin");
    fake_qemu(&bin_dir);
    let prev_path = std::env::var("PATH").unwrap_or_default();
    std::env::set_var(
        "PATH",
        format!("{}:{prev_path}", bin_dir.to_str().expect("utf8")),
    );
    std::env::set_var(
        "DEPUTYOS_DESKTOP_CACHE_DIR",
        dir.path().join("nonexistent-cache"),
    );
    std::env::set_var("DEPUTYOS_DESKTOP_RUNTIME_DIR", dir.path().join("run"));

    let driver = LinuxDriver::new();
    let err = driver.start().expect_err("no installed image");
    let msg = format!("{err:#}");
    assert!(msg.contains("install"), "got: {msg}");

    std::env::set_var("PATH", prev_path);
    std::env::remove_var("DEPUTYOS_DESKTOP_CACHE_DIR");
    std::env::remove_var("DEPUTYOS_DESKTOP_RUNTIME_DIR");
}

#[test]
fn stop_with_no_pidfile_is_noop() {
    let _g = env_lock().lock().expect("env_lock poisoned");
    let dir = tempfile::tempdir().expect("tempdir");
    std::env::set_var("DEPUTYOS_DESKTOP_RUNTIME_DIR", dir.path());
    let driver = LinuxDriver::new();
    driver.stop().expect("stop with no pid is fine");
    std::env::remove_var("DEPUTYOS_DESKTOP_RUNTIME_DIR");
}

#[test]
fn status_with_stale_pidfile_self_heals() {
    let _g = env_lock().lock().expect("env_lock poisoned");
    let dir = tempfile::tempdir().expect("tempdir");
    fs::write(dir.path().join("deputyos-desktop.pid"), "999999999").expect("write");
    std::env::set_var("DEPUTYOS_DESKTOP_RUNTIME_DIR", dir.path());
    let driver = LinuxDriver::new();
    let s = driver.status().expect("status");
    assert!(matches!(s, VmStatus::Stopped));
    // Self-heal: PID file should be gone.
    assert!(!dir.path().join("deputyos-desktop.pid").exists());
    std::env::remove_var("DEPUTYOS_DESKTOP_RUNTIME_DIR");
}

#[test]
fn pick_artefact_reports_target_mismatch() {
    if !minisign_available() {
        eprintln!("skip: minisign not installed");
        return;
    }
    let _g = env_lock().lock().expect("env_lock poisoned");
    let dir = tempfile::tempdir().expect("tempdir");
    let (pubkey, seckey) = gen_keypair(dir.path());
    let dist_dir = dir.path().join("dist");
    fs::create_dir(&dist_dir).expect("dist");
    let image_path = dist_dir.join("img.qcow2");
    fs::write(&image_path, b"x").expect("img");
    sign_file(&seckey, &image_path);

    // Manifest only has wsl2 — host wants qemu-x86_64.
    let manifest_path = write_manifest(&dist_dir, "wsl2", "img.qcow2", &sha256_hex(&image_path), 1);
    sign_file(&seckey, &manifest_path);

    let server = httpmock::MockServer::start();
    let mb = fs::read(&manifest_path).expect("");
    let ms = fs::read(format!("{}.minisig", manifest_path.display())).expect("");
    let _a = server.mock(|w, t| {
        w.method(httpmock::Method::GET).path("/manifest.json");
        t.status(200).body(mb);
    });
    let _b = server.mock(|w, t| {
        w.method(httpmock::Method::GET)
            .path("/manifest.json.minisig");
        t.status(200).body(ms);
    });
    let url = server.url("/manifest.json");
    let src = launcher_manifest::fetch_and_verify(&url, &pubkey).expect("fetch ok");
    let err = launcher_manifest::pick_artefact(&src, "qemu-x86_64", None).expect_err("miss");
    let msg = format!("{err:#}");
    assert!(
        msg.contains("wsl2"),
        "should list available targets; got: {msg}"
    );
}

#[test]
fn config_paths_respect_env_vars() {
    let _g = env_lock().lock().expect("env_lock poisoned");
    std::env::set_var("DEPUTYOS_DESKTOP_CACHE_DIR", "/tmp/deputyos-it-cache");
    std::env::set_var("DEPUTYOS_DESKTOP_DATA_DIR", "/tmp/deputyos-it-data");
    std::env::set_var("DEPUTYOS_DESKTOP_RUNTIME_DIR", "/tmp/deputyos-it-run");
    assert_eq!(config::cache_dir(), Path::new("/tmp/deputyos-it-cache"));
    assert_eq!(config::data_dir(), Path::new("/tmp/deputyos-it-data"));
    assert_eq!(config::runtime_dir(), Path::new("/tmp/deputyos-it-run"));
    std::env::remove_var("DEPUTYOS_DESKTOP_CACHE_DIR");
    std::env::remove_var("DEPUTYOS_DESKTOP_DATA_DIR");
    std::env::remove_var("DEPUTYOS_DESKTOP_RUNTIME_DIR");
}

/// Stage a manifest that carries a `desktop_launchers[<triple>]` entry
/// pointing at `launcher_filename` (relative URL — resolved against the
/// manifest's httpmock origin). One throwaway artefact keeps the manifest
/// structurally valid.
fn write_manifest_with_launcher(
    dir: &Path,
    triple: &str,
    launcher_filename: &str,
    launcher_sha256: &str,
) -> PathBuf {
    let path = dir.join("manifest.json");
    let m = serde_json::json!({
        "schema_version": 1,
        "release_version": "2026.6.22",
        "channel": "dev",
        "released_at": "2026-06-22T00:00:00Z",
        "artefacts": [{
            "target": "qemu-x86_64",
            "profile": "openclaw",
            "filename": "deputyos-openclaw-qemu-x86_64-2026.6.22-dev.qcow2",
            "format": "qcow2",
            "size_bytes": 1024,
            "sha256": "0".repeat(64),
            "minisig_url": "img.qcow2.minisig",
            "url": "img.qcow2",
        }],
        "desktop_launchers": {
            triple: {
                "triple": triple,
                "filename": launcher_filename,
                "url": launcher_filename,
                "sha256": launcher_sha256,
                "minisig_url": format!("{launcher_filename}.minisig"),
            }
        },
    });
    fs::write(&path, serde_json::to_string_pretty(&m).expect("ser")).expect("write manifest");
    path
}

/// `selfupdate::apply` E2E: serve a signed launcher blob + manifest over
/// httpmock, point `DEPUTYOS_DESKTOP_SELF_EXE` at a fake "current exe",
/// run `check` (must report an update) then `apply`, and assert the on-disk
/// binary was atomically swapped to the signed blob's bytes. Mirrors
/// `install_downloads_and_verifies`'s minisign + httpmock harness. Runs on
/// any Linux host with `minisign` (no qemu needed) whose `host_triple()`
/// resolves.
#[test]
fn selfupdate_apply_swaps_signed_launcher_into_place() {
    if !minisign_available() {
        eprintln!("skip: minisign not installed");
        return;
    }
    let triple = match config::host_triple() {
        Some(t) => t,
        None => {
            eprintln!("skip: host triple unknown");
            return;
        }
    };
    let _g = env_lock().lock().expect("env_lock poisoned");

    let dir = tempfile::tempdir().expect("tempdir");
    let keys_dir = dir.path().join("keys");
    fs::create_dir(&keys_dir).expect("keys");
    let (pubkey, seckey) = gen_keypair(&keys_dir);

    // A fake "new launcher binary" — small, but real bytes + a real sig.
    let dist_dir = dir.path().join("dist");
    fs::create_dir(&dist_dir).expect("dist");
    let launcher_filename = format!("deputyos-desktop-{triple}");
    let launcher_path = dist_dir.join(&launcher_filename);
    let blob = b"#!this is the NEW launcher binary\n";
    fs::write(&launcher_path, blob).expect("write launcher blob");
    let launcher_sha = sha256_hex(&launcher_path);
    sign_file(&seckey, &launcher_path);

    // Manifest + manifest signature advertising the launcher for our triple.
    let manifest_path =
        write_manifest_with_launcher(&dist_dir, triple, &launcher_filename, &launcher_sha);
    sign_file(&seckey, &manifest_path);

    // The fake "current exe" the swap targets — different bytes from the
    // blob, so `check` sees an update. Lives under the same tempdir as the
    // cache so the POSIX `rename` is same-filesystem (atomic).
    let current_exe = dir.path().join("current-launcher");
    fs::write(&current_exe, b"#!old launcher\n").expect("write current exe");

    // Cache dir under the same tempdir (same FS as current_exe → rename ok).
    let cache_dir = dir.path().join("cache");
    fs::create_dir(&cache_dir).expect("cache");

    // httpmock serves manifest + sig + launcher blob + blob sig.
    let server = httpmock::MockServer::start();
    let manifest_body = fs::read(&manifest_path).expect("read manifest");
    let manifest_sig = fs::read(format!("{}.minisig", manifest_path.display())).expect("m sig");
    let launcher_body = fs::read(&launcher_path).expect("read launcher");
    let launcher_sig = fs::read(format!("{}.minisig", launcher_path.display())).expect("l sig");

    let _m1 = server.mock(|when, then| {
        when.method(httpmock::Method::GET).path("/manifest.json");
        then.status(200).body(manifest_body);
    });
    let _m2 = server.mock(|when, then| {
        when.method(httpmock::Method::GET)
            .path("/manifest.json.minisig");
        then.status(200).body(manifest_sig);
    });
    let _m3 = server.mock(|when, then| {
        when.method(httpmock::Method::GET)
            .path(format!("/{launcher_filename}"));
        then.status(200).body(launcher_body);
    });
    let _m4 = server.mock(|when, then| {
        when.method(httpmock::Method::GET)
            .path(format!("/{launcher_filename}.minisig"));
        then.status(200).body(launcher_sig);
    });

    // Point the launcher at the mock + the fake current exe + temp cache.
    let url = format!("{}/manifest.json", server.base_url());
    std::env::set_var("DEPUTYOS_DESKTOP_MANIFEST_URL", &url);
    std::env::set_var("DEPUTYOS_DESKTOP_SELF_EXE", &current_exe);
    std::env::set_var("DEPUTYOS_DESKTOP_CACHE_DIR", &cache_dir);
    std::env::set_var("DEPUTYOS_DESKTOP_PUBKEY", &pubkey);

    // Fetch + verify the manifest, then drive the self-update lib API the
    // way cmd_selfupdate does.
    let src = launcher_manifest::fetch_and_verify(&url, &pubkey).expect("fetch manifest");
    let launcher = selfupdate::check(&src, triple)
        .expect("check")
        .expect("update available");

    selfupdate::apply(launcher, &src, &pubkey).expect("apply self-update");

    // The on-disk current-exe now holds the signed blob's bytes (swapped).
    let after = fs::read(&current_exe).expect("read swapped exe");
    assert_eq!(
        after, blob,
        "current exe was replaced with the signed launcher"
    );

    // Cleanup the env we set.
    std::env::remove_var("DEPUTYOS_DESKTOP_MANIFEST_URL");
    std::env::remove_var("DEPUTYOS_DESKTOP_SELF_EXE");
    std::env::remove_var("DEPUTYOS_DESKTOP_CACHE_DIR");
    std::env::remove_var("DEPUTYOS_DESKTOP_PUBKEY");
}
