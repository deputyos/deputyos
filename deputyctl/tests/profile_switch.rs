//! Integration tests for `deputyctl profile switch`.
//!
//! Exercises the dev-host path: `--dry-run` must succeed without touching
//! disk, and an unknown id must yield an error message + nonzero exit.

use std::path::PathBuf;

use deputyctl::profile_switch::{run, SwitchOpts};

fn workspace_profiles() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("deputyctl/ has a parent")
        .join("profiles")
}

/// Set up envs so the binary's "active profile" pointer + profiles dir live
/// in a temp dir (no clobbering /etc/deputyos/ on a real Linux dev host).
fn isolate_env(scratch: &tempfile::TempDir) {
    std::env::set_var("DEPUTYOS_PROFILES_DIR", workspace_profiles());
    std::env::set_var(
        "DEPUTYOS_ACTIVE_PROFILE_FILE",
        scratch.path().join("active-profile"),
    );
}

#[test]
fn dry_run_succeeds_for_known_profile() {
    let dir = tempfile::tempdir().expect("tempdir");
    isolate_env(&dir);
    let code = run(
        "openclaw",
        SwitchOpts {
            yes: true,
            dry_run: true,
        },
    )
    .expect("run");
    assert_eq!(code, 0, "dry-run on known profile must exit 0");
    assert!(
        !dir.path().join("active-profile").exists(),
        "dry-run must not write the active-profile pointer",
    );
}

#[test]
fn unknown_profile_returns_nonzero() {
    let dir = tempfile::tempdir().expect("tempdir");
    isolate_env(&dir);
    let code = run(
        "this-profile-doesnt-exist",
        SwitchOpts {
            yes: true,
            dry_run: true,
        },
    )
    .expect("run");
    assert_eq!(code, 1, "unknown profile must exit nonzero");
}
