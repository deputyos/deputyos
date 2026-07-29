//! Path helpers: locate `deputyctl` (PATH first, then workspace target dirs)
//! and resolve the data dir where push subscriptions are persisted.

use std::path::{Path, PathBuf};

/// Where the PWA stores per-host state (push subscriptions, vapid keys,
/// last-known cache). Defaults to `/var/lib/deputyos`; overridable via
/// `DEPUTYPWA_DATA_DIR` for `make pwa` and tests.
pub fn data_dir() -> PathBuf {
    if let Ok(d) = std::env::var("DEPUTYPWA_DATA_DIR") {
        return PathBuf::from(d);
    }
    PathBuf::from("/var/lib/deputyos")
}

/// JSONL file the PWA appends to when a browser registers a Web Push
/// subscription. Each line is an independent subscription record so we
/// never need to read+rewrite the file under contention.
pub fn push_subscriptions_path() -> PathBuf {
    data_dir().join("push-subscriptions.jsonl")
}

/// Try to find an `deputyctl` binary the PWA can shell out to. Order:
///
/// 1. `DEPUTYPWA_DEPUTYCTL` env var (tests pin to a built binary).
/// 2. First `deputyctl` on `PATH`.
/// 3. `target/release/deputyctl` relative to the current dir (release dev
///    workflow).
/// 4. `target/debug/deputyctl` relative to the current dir.
///
/// Returns `None` if none exist; the route handler then either renders the
/// dev-stub (if `DEPUTYPWA_DEV_STUB=1`) or surfaces a friendly error.
pub fn which_deputyctl() -> Option<PathBuf> {
    if let Ok(p) = std::env::var("DEPUTYPWA_DEPUTYCTL") {
        let p = PathBuf::from(p);
        if p.is_file() {
            return Some(p);
        }
    }
    if let Some(p) = which_on_path("deputyctl") {
        return Some(p);
    }
    for rel in ["target/release/deputyctl", "target/debug/deputyctl"] {
        let p = PathBuf::from(rel);
        if p.is_file() {
            return Some(p);
        }
    }
    None
}

fn which_on_path(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path) {
        let candidate = dir.join(name);
        if candidate.is_file() && is_executable(&candidate) {
            return Some(candidate);
        }
    }
    None
}

#[cfg(unix)]
fn is_executable(p: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(p)
        .map(|m| m.permissions().mode() & 0o111 != 0)
        .unwrap_or(false)
}

#[cfg(not(unix))]
fn is_executable(_p: &Path) -> bool {
    true
}

/// Whether the dev-stub data path should be used. Set by `make pwa` so
/// contributors can iterate on UI without first-booting an image.
pub fn dev_stub_enabled() -> bool {
    matches!(std::env::var("DEPUTYPWA_DEV_STUB").as_deref(), Ok("1"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    static ENV_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn data_dir_honours_env() {
        let _guard = ENV_LOCK.lock().expect("env lock poisoned");
        let prev = std::env::var("DEPUTYPWA_DATA_DIR").ok();
        std::env::set_var("DEPUTYPWA_DATA_DIR", "/tmp/deputypwa-test");
        assert_eq!(data_dir(), PathBuf::from("/tmp/deputypwa-test"));
        match prev {
            Some(v) => std::env::set_var("DEPUTYPWA_DATA_DIR", v),
            None => std::env::remove_var("DEPUTYPWA_DATA_DIR"),
        }
    }

    #[test]
    fn push_subscriptions_path_under_data_dir() {
        let _guard = ENV_LOCK.lock().expect("env lock poisoned");
        let prev = std::env::var("DEPUTYPWA_DATA_DIR").ok();
        std::env::set_var("DEPUTYPWA_DATA_DIR", "/tmp/deputypwa-test2");
        let p = push_subscriptions_path();
        assert!(p.ends_with("push-subscriptions.jsonl"));
        assert!(p.starts_with("/tmp/deputypwa-test2"));
        match prev {
            Some(v) => std::env::set_var("DEPUTYPWA_DATA_DIR", v),
            None => std::env::remove_var("DEPUTYPWA_DATA_DIR"),
        }
    }

    #[test]
    fn dev_stub_flag_round_trip() {
        let _guard = ENV_LOCK.lock().expect("env lock poisoned");
        let prev = std::env::var("DEPUTYPWA_DEV_STUB").ok();
        std::env::remove_var("DEPUTYPWA_DEV_STUB");
        assert!(!dev_stub_enabled());
        std::env::set_var("DEPUTYPWA_DEV_STUB", "1");
        assert!(dev_stub_enabled());
        match prev {
            Some(v) => std::env::set_var("DEPUTYPWA_DEV_STUB", v),
            None => std::env::remove_var("DEPUTYPWA_DEV_STUB"),
        }
    }
}
