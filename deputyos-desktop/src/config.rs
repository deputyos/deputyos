//! Platform-canonical paths for the desktop launcher.
//!
//! Mirrors `deputyctl::paths`'s env-overridable pattern so integration tests
//! can stage fixtures into a tempdir without touching the user's real
//! cache. Resolution order for every getter:
//!
//! 1. `DEPUTYOS_DESKTOP_*` env var, if set.
//! 2. Platform default (XDG on Linux, `%LOCALAPPDATA%` on Windows,
//!    `~/Library/Caches/deputyos-desktop` on macOS).
//! 3. Best-effort fallback to `./deputyos-desktop-data`.
//!
//! No code in this module touches the filesystem; callers are responsible
//! for `mkdir -p` before writing.

use std::path::PathBuf;

/// Unit tests mutate process-wide `DEPUTYOS_DESKTOP_*` variables from several
/// modules. They must all use one lock because module-local locks do not
/// serialize against each other under the parallel Rust test harness.
#[cfg(test)]
pub(crate) fn test_env_lock() -> &'static std::sync::Mutex<()> {
    use std::sync::{Mutex, OnceLock};
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

/// Pubkey path used to verify manifest signatures. Defaults to the dev key
/// shipped under `~/.config/deputyos/dev-keys/` (same convention deputyctl uses).
///
/// In production the launcher is built with the public key embedded —
/// tracked as M2.5-rest. For now we read it from the same dev path so the
/// signed-manifest test loop works locally.
/// Returns the embedded pubkey string if the binary was built with one.
/// CI builds set `DEPUTYOS_DESKTOP_EMBEDDED_PUBKEY` at compile time.
pub fn embedded_pubkey() -> Option<&'static str> {
    const KEY: &str = include_str!(env!("DEPUTYOS_EMBEDDED_PUBKEY_PATH"));
    if KEY.trim().is_empty() {
        None
    } else {
        Some(KEY)
    }
}

pub fn pubkey_path() -> PathBuf {
    if let Ok(p) = std::env::var("DEPUTYOS_DESKTOP_PUBKEY") {
        return PathBuf::from(p);
    }
    if let Some(home) = dirs::home_dir() {
        let dev = home.join(".config/deputyos/dev-keys/deputyos-dev.pub");
        if dev.is_file() {
            return dev;
        }
    }
    PathBuf::from("deputyos-dev.pub")
}

/// Path to the last-installed manifest version tracker.
pub fn last_manifest_path() -> PathBuf {
    data_dir().join("last-manifest.json")
}

/// Cache dir for downloaded images.
///
/// - Linux: `$XDG_CACHE_HOME/deputyos-desktop` (default `~/.cache/deputyos-desktop`)
/// - macOS: `~/Library/Caches/deputyos-desktop`
/// - Windows: `%LOCALAPPDATA%\deputyos-desktop\cache`
pub fn cache_dir() -> PathBuf {
    if let Ok(p) = std::env::var("DEPUTYOS_DESKTOP_CACHE_DIR") {
        return PathBuf::from(p);
    }
    if let Some(base) = dirs::cache_dir() {
        return base.join("deputyos-desktop");
    }
    PathBuf::from("./deputyos-desktop-data/cache")
}

/// Data dir for persistent state (PID file, last-installed manifest, etc.).
///
/// - Linux: `$XDG_DATA_HOME/deputyos-desktop` (default `~/.local/share/deputyos-desktop`)
/// - macOS: `~/Library/Application Support/deputyos-desktop`
/// - Windows: `%LOCALAPPDATA%\deputyos-desktop\data`
pub fn data_dir() -> PathBuf {
    if let Ok(p) = std::env::var("DEPUTYOS_DESKTOP_DATA_DIR") {
        return PathBuf::from(p);
    }
    if let Some(base) = dirs::data_local_dir() {
        return base.join("deputyos-desktop");
    }
    PathBuf::from("./deputyos-desktop-data/data")
}

/// Runtime dir (PID file lives here). Linux `/run/user/<uid>` if available,
/// otherwise falls back to `data_dir()`.
pub fn runtime_dir() -> PathBuf {
    if let Ok(p) = std::env::var("DEPUTYOS_DESKTOP_RUNTIME_DIR") {
        return PathBuf::from(p);
    }
    if let Some(base) = dirs::runtime_dir() {
        return base.join("deputyos-desktop");
    }
    data_dir()
}

/// URL of the channel's `manifest.json`. Defaults to the production CDN's
/// `dev` channel. Override via env for testing or private mirrors.
pub fn manifest_url() -> String {
    if let Ok(u) = std::env::var("DEPUTYOS_DESKTOP_MANIFEST_URL") {
        return u;
    }
    "https://cdn.deputyos.com/dev/manifest.json".to_string()
}

/// Host-side port the launcher forwards to the in-VM wizard (the guest
/// wizard listens on :8088). Override via `DEPUTYOS_DESKTOP_WIZARD_PORT` to
/// dodge host port collisions — the local dev loop sets this to the 7000
/// series (7088) because dev hosts often have stale processes on 8088.
pub fn wizard_host_port() -> u16 {
    std::env::var("DEPUTYOS_DESKTOP_WIZARD_PORT")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(8088)
}

/// Host-side port the launcher forwards to the in-VM chat/relay (the guest
/// service listens on :8080). Override via `DEPUTYOS_DESKTOP_GATEWAY_PORT`.
pub fn gateway_host_port() -> u16 {
    std::env::var("DEPUTYOS_DESKTOP_GATEWAY_PORT")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(8080)
}

/// Wizard URL the launcher opens after `start`. The Linux driver forwards
/// `wizard_host_port()` on the host to :8088 in the guest, so by default this
/// is `http://localhost:8088`. Override the whole URL via
/// `DEPUTYOS_DESKTOP_WIZARD_URL`, or just the port via
/// `DEPUTYOS_DESKTOP_WIZARD_PORT`.
pub fn wizard_url() -> String {
    if let Ok(u) = std::env::var("DEPUTYOS_DESKTOP_WIZARD_URL") {
        return u;
    }
    format!("http://localhost:{}", wizard_host_port())
}

/// Map a (OS, ARCH) pair to the Rust target triple the **launcher binary**
/// is built for. Distinct from the driver's image `target_for_host()`
/// (which returns `qemu-x86_64` / `wsl2` / `macos-qemu`): this is the triple
/// that indexes `manifest.desktop_launchers[<triple>]` so the launcher can
/// self-update. Takes args so it is unit-testable independent of the host.
pub fn triple_for(os: &str, arch: &str) -> Option<&'static str> {
    match (os, arch) {
        ("linux", "x86_64") => Some("x86_64-unknown-linux-gnu"),
        ("linux", "aarch64") => Some("aarch64-unknown-linux-gnu"),
        ("macos", "aarch64") => Some("aarch64-apple-darwin"),
        ("macos", "x86_64") => Some("x86_64-apple-darwin"),
        ("windows", "x86_64") => Some("x86_64-pc-windows-msvc"),
        _ => None,
    }
}

/// The Rust target triple of the launcher binary running on this host.
/// `None` on an unsupported host (the launcher is only built for the five
/// triples above, so in practice this always resolves on a real launcher).
pub fn host_triple() -> Option<&'static str> {
    triple_for(std::env::consts::OS, std::env::consts::ARCH)
}

/// Path to the running launcher binary — the file `self-update` replaces.
///
/// Honors the `DEPUTYOS_DESKTOP_SELF_EXE` env override so integration tests
/// can point "the current exe" at a temp fixture without touching the real
/// binary. Production callers leave it unset and we ask the kernel via
/// [`std::env::current_exe`].
pub fn current_exe_path() -> Result<std::path::PathBuf, std::io::Error> {
    if let Ok(p) = std::env::var("DEPUTYOS_DESKTOP_SELF_EXE") {
        return Ok(std::path::PathBuf::from(p));
    }
    std::env::current_exe()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embedded_pubkey_is_complete_when_configured() {
        if let Some(key) = embedded_pubkey() {
            assert!(
                key.lines().count() >= 2,
                "a minisign public key includes its comment and key-data lines"
            );
        }
    }

    #[test]
    fn cache_dir_overridable_by_env() {
        let _g = test_env_lock().lock().expect("test_env_lock poisoned");
        // Acquire test-only env mutex to avoid races with other tests.
        // We don't have deputyctl::env_mutex visible here, so just use a
        // unique sentinel value and clean up.
        let prev = std::env::var("DEPUTYOS_DESKTOP_CACHE_DIR").ok();
        std::env::set_var(
            "DEPUTYOS_DESKTOP_CACHE_DIR",
            "/tmp/deputyos-desktop-cache-test",
        );
        assert_eq!(
            cache_dir(),
            PathBuf::from("/tmp/deputyos-desktop-cache-test")
        );
        match prev {
            Some(v) => std::env::set_var("DEPUTYOS_DESKTOP_CACHE_DIR", v),
            None => std::env::remove_var("DEPUTYOS_DESKTOP_CACHE_DIR"),
        }
    }

    #[test]
    fn manifest_url_defaults_to_production_cdn() {
        let _g = test_env_lock().lock().expect("test_env_lock poisoned");
        let prev = std::env::var("DEPUTYOS_DESKTOP_MANIFEST_URL").ok();
        std::env::remove_var("DEPUTYOS_DESKTOP_MANIFEST_URL");
        let u = manifest_url();
        assert_eq!(u, "https://cdn.deputyos.com/dev/manifest.json");
        if let Some(v) = prev {
            std::env::set_var("DEPUTYOS_DESKTOP_MANIFEST_URL", v);
        }
    }

    #[test]
    fn manifest_url_env_override() {
        let _g = test_env_lock().lock().expect("test_env_lock poisoned");
        let prev = std::env::var("DEPUTYOS_DESKTOP_MANIFEST_URL").ok();
        std::env::set_var(
            "DEPUTYOS_DESKTOP_MANIFEST_URL",
            "http://example.test/m.json",
        );
        assert_eq!(manifest_url(), "http://example.test/m.json");
        match prev {
            Some(v) => std::env::set_var("DEPUTYOS_DESKTOP_MANIFEST_URL", v),
            None => std::env::remove_var("DEPUTYOS_DESKTOP_MANIFEST_URL"),
        }
    }

    #[test]
    fn triple_for_maps_all_five_launcher_targets() {
        assert_eq!(
            triple_for("linux", "x86_64"),
            Some("x86_64-unknown-linux-gnu")
        );
        assert_eq!(
            triple_for("linux", "aarch64"),
            Some("aarch64-unknown-linux-gnu")
        );
        assert_eq!(triple_for("macos", "aarch64"), Some("aarch64-apple-darwin"));
        assert_eq!(triple_for("macos", "x86_64"), Some("x86_64-apple-darwin"));
        assert_eq!(
            triple_for("windows", "x86_64"),
            Some("x86_64-pc-windows-msvc")
        );
    }

    #[test]
    fn triple_for_unknown_returns_none() {
        assert_eq!(triple_for("freebsd", "x86_64"), None);
        assert_eq!(triple_for("linux", "riscv64"), None);
    }

    #[test]
    fn host_triple_resolves_on_a_built_target() {
        // The launcher only builds for the five triples above, so whatever
        // host runs this test must resolve to one of them.
        let t = host_triple().expect("test host must be a launcher target");
        assert!(matches!(
            t,
            "x86_64-unknown-linux-gnu"
                | "aarch64-unknown-linux-gnu"
                | "aarch64-apple-darwin"
                | "x86_64-apple-darwin"
                | "x86_64-pc-windows-msvc"
        ));
    }

    #[test]
    fn current_exe_path_honors_env_override() {
        let _g = test_env_lock().lock().expect("test_env_lock poisoned");
        let prev = std::env::var("DEPUTYOS_DESKTOP_SELF_EXE").ok();
        std::env::set_var(
            "DEPUTYOS_DESKTOP_SELF_EXE",
            "/tmp/deputyos-desktop-fake-exe",
        );
        let p = current_exe_path().expect("override path");
        assert_eq!(
            p,
            std::path::PathBuf::from("/tmp/deputyos-desktop-fake-exe")
        );
        match prev {
            Some(v) => std::env::set_var("DEPUTYOS_DESKTOP_SELF_EXE", v),
            None => std::env::remove_var("DEPUTYOS_DESKTOP_SELF_EXE"),
        }
    }
}
