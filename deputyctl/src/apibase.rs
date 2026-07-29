//! Shared resolution of the deputyOS API base URL.
//!
//! The appliance talks to one API hostname for auth, device registration,
//! the integrated tunnel, and the remote command poller. Which hostname is
//! resolved identically everywhere so a self-hosted / custom backend chosen
//! once flows to every component.
//!
//! # Precedence (highest first)
//! 1. an explicit `--api-base` flag the caller passes to [`resolve`]
//!    (operator override on the CLI),
//! 2. the `DEPUTYOS_API_BASE` env var (local E2E / one-shot operator override),
//! 3. the persisted file at [`file_path`] (default `/etc/deputyos/api-base`),
//!    written by the deputywizard Account step when the user picks a custom
//!    backend at first boot — the mechanism that lets a first-boot user
//!    choose a self-hosted backend without setting env vars on the appliance,
//! 4. [`DEFAULT_API_BASE`] (`https://api.deputyos.com`).
//!
//! The file is `0644` and non-secret — it holds only a public hostname. The
//! wizard owns writing it (see `deputywizard/src/routes.rs::poll_and_register`);
//! `deputyctl` (tunnel, command poller) only reads it back here.

use std::path::PathBuf;

/// Production API hostname. Used when nothing else is configured.
pub const DEFAULT_API_BASE: &str = "https://api.deputyos.com";

/// Env var a caller/operator can set to override the API base for one process.
const ENV_OVERRIDE: &str = "DEPUTYOS_API_BASE";

/// Env var redirecting the persisted-file location (test isolation). When
/// unset, [`file_path`] is `/etc/deputyos/api-base`.
const ENV_FILE_PATH: &str = "DEPUTYOS_API_BASE_FILE";

/// Default location of the persisted api-base file.
const DEFAULT_FILE_PATH: &str = "/etc/deputyos/api-base";

/// Path to the persisted api-base file. Honors `DEPUTYOS_API_BASE_FILE` (used
/// by tests to point at a tempdir); otherwise `/etc/deputyos/api-base`.
pub fn file_path() -> PathBuf {
    std::env::var(ENV_FILE_PATH)
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from(DEFAULT_FILE_PATH))
}

/// Read + trim the persisted api-base file. `None` if it is missing, unreadable,
/// or empty/whitespace-only. Never panics.
pub fn read_file() -> Option<String> {
    let raw = std::fs::read_to_string(file_path()).ok()?;
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

/// Resolve the API base with the documented precedence. `flag` is the caller's
/// explicit override (a `--api-base` value); pass `None` when there isn't one.
/// The result has any trailing `/` stripped.
pub fn resolve(flag: Option<&str>) -> String {
    let chosen = flag
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
        .or_else(|| {
            std::env::var(ENV_OVERRIDE)
                .ok()
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
        })
        .or_else(read_file)
        .unwrap_or_else(|| DEFAULT_API_BASE.to_string());
    chosen.trim_end_matches('/').to_string()
}

/// Persist a custom api-base to [`file_path`] (`0644`, non-secret — a public
/// hostname only). The parent directory is created if missing. Only call this
/// for a non-default base so production appliances (using the compiled default)
/// stay free of a redundant file.
pub fn persist(base: &str) -> std::io::Result<()> {
    let path = file_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    // 0644 — a public URL, not a secret. Write then set the mode explicitly
    // so umask can't widen it.
    std::fs::write(&path, format!("{base}\n"))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Acquire the process-wide env lock (see `crate::env_mutex`) so these
    /// env-mutating tests don't race the other env-var tests.
    fn locked() -> std::sync::MutexGuard<'static, ()> {
        crate::env_mutex().lock().unwrap_or_else(|e| e.into_inner())
    }

    fn reset_env() {
        std::env::remove_var(ENV_OVERRIDE);
        std::env::remove_var(ENV_FILE_PATH);
    }

    #[test]
    fn resolve_defaults_when_nothing_configured() {
        let _g = locked();
        reset_env();
        // Point the file at a path that doesn't exist so a stray
        // /etc/deputyos/api-base on the host can't perturb this assertion.
        let dir = tempfile::tempdir().expect("tempdir");
        std::env::set_var(ENV_FILE_PATH, dir.path().join("no-such-file"));
        assert_eq!(resolve(None), DEFAULT_API_BASE);
    }

    #[test]
    fn resolve_flag_wins_over_env_and_file() {
        let _g = locked();
        reset_env();
        let dir = tempfile::tempdir().expect("tempdir");
        std::env::set_var(ENV_FILE_PATH, dir.path().join("api-base"));
        std::fs::write(dir.path().join("api-base"), "https://file.test").expect("write file");
        std::env::set_var(ENV_OVERRIDE, "https://env.test");
        assert_eq!(resolve(Some("https://flag.test")), "https://flag.test");
    }

    #[test]
    fn resolve_env_wins_over_file() {
        let _g = locked();
        reset_env();
        let dir = tempfile::tempdir().expect("tempdir");
        std::env::set_var(ENV_FILE_PATH, dir.path().join("api-base"));
        std::fs::write(dir.path().join("api-base"), "https://file.test").expect("write file");
        std::env::set_var(ENV_OVERRIDE, "https://env.test");
        assert_eq!(resolve(None), "https://env.test");
    }

    #[test]
    fn resolve_file_wins_over_default() {
        let _g = locked();
        reset_env();
        let dir = tempfile::tempdir().expect("tempdir");
        let f = dir.path().join("api-base");
        std::env::set_var(ENV_FILE_PATH, &f);
        std::fs::write(&f, "  https://self-hosted.example/api  ").expect("write file");
        assert_eq!(resolve(None), "https://self-hosted.example/api");
    }

    #[test]
    fn resolve_strips_trailing_slash() {
        let _g = locked();
        reset_env();
        std::env::set_var(ENV_OVERRIDE, "https://env.example/");
        assert_eq!(resolve(None), "https://env.example");
    }

    #[test]
    fn resolve_ignores_blank_flag_and_env() {
        let _g = locked();
        reset_env();
        let dir = tempfile::tempdir().expect("tempdir");
        std::env::set_var(ENV_FILE_PATH, dir.path().join("no-such-file"));
        std::env::set_var(ENV_OVERRIDE, "   ");
        assert_eq!(resolve(Some("   ")), DEFAULT_API_BASE);
    }

    #[test]
    fn resolve_ignores_empty_file() {
        let _g = locked();
        reset_env();
        let dir = tempfile::tempdir().expect("tempdir");
        let f = dir.path().join("api-base");
        std::env::set_var(ENV_FILE_PATH, &f);
        std::fs::write(&f, "  \n").expect("write file");
        assert_eq!(resolve(None), DEFAULT_API_BASE);
    }

    #[test]
    fn persist_then_resolve_reads_it_back() {
        let _g = locked();
        reset_env();
        let dir = tempfile::tempdir().expect("tempdir");
        let f = dir.path().join("nested/api-base");
        std::env::set_var(ENV_FILE_PATH, &f);
        persist("https://custom.example").expect("persist");
        assert_eq!(resolve(None), "https://custom.example");
        // File mode is 0644 on unix.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&f)
                .expect("metadata")
                .permissions()
                .mode()
                & 0o777;
            assert_eq!(mode, 0o644);
        }
    }
}
