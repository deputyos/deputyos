//! Launcher-binary self-update (M2.5 Lane D).
//!
//! The image-update path in `main::cmd_update` refreshes the *VM image* the
//! launcher boots. This module is the other half: replacing the **launcher
//! binary itself** from the manifest's `desktop_launchers[<host-triple>]`
//! entry, so a published release can ship a new launcher without making the
//! user re-download it by hand.
//!
//! Trust surface is identical to the image path — the new binary is
//! sha256-checked **and** minisign-verified against the same embedded pubkey
//! (`download::download_and_verify`) before it ever touches the on-disk
//! executable. The running process is never replaced in memory; we only
//! swap the file on disk, so the new launcher takes effect on the next
//! launch. See `documentation/docs/distribution/desktop-launcher-internals.md`
//! § Launcher-binary self-update for the per-OS swap semantics + the
//! security note.

use std::path::Path;

use anyhow::{anyhow, Context, Result};
use deputyctl::release::{resolve_url, sha256_hex, DesktopLauncher, ManifestSource};

use crate::{config, download};

/// Decide whether a self-update is available for `triple`.
///
/// Looks up `src.manifest.desktop_launchers[triple]` and compares its
/// `sha256` to the sha256 of the running launcher binary
/// ([`config::current_exe_path`]). Returns:
/// - `Ok(None)` — the running binary already matches the manifest (up to
///   date; a same-version rebuild with a different sha still counts as
///   "update available" — see the internals doc).
/// - `Ok(Some(entry))` — a different launcher binary is published.
/// - `Err` — the triple is absent from the manifest (clear message listing
///   the available triples), or the current exe couldn't be read/hashed.
pub fn check<'a>(src: &'a ManifestSource, triple: &str) -> Result<Option<&'a DesktopLauncher>> {
    let launcher = src.manifest.desktop_launchers.get(triple).ok_or_else(|| {
        let available: Vec<&str> = src
            .manifest
            .desktop_launchers
            .keys()
            .map(|s| s.as_str())
            .collect();
        anyhow!(
            "no desktop_launchers entry for triple='{triple}'; available: [{}]",
            available.join(", ")
        )
    })?;

    let exe =
        config::current_exe_path().context("locating the running launcher binary (current_exe)")?;
    let actual = sha256_hex(&exe)
        .with_context(|| format!("hashing current launcher at {}", exe.display()))?;
    if actual == launcher.sha256 {
        eprintln!("==> launcher up to date (sha256 {actual}) for triple {triple}");
        Ok(None)
    } else {
        eprintln!(
            "==> newer launcher available for triple {triple} (manifest sha {} != running {})",
            &launcher.sha256[..8],
            &actual[..8]
        );
        Ok(Some(launcher))
    }
}

/// Download + verify + atomically install `launcher` as the running binary.
///
/// Stages to `cache_dir()/launcher-staging/<filename>`, sha+minisig verifies
/// via [`download::download_and_verify`], then swaps it over the current
/// executable via [`replace_binary`]. On success the new binary is on disk
/// and takes effect on the next launch.
pub fn apply(launcher: &DesktopLauncher, src: &ManifestSource, pubkey: &Path) -> Result<()> {
    let img_url = resolve_url(&src.origin, &launcher.url);
    let sig_url = resolve_url(&src.origin, &launcher.minisig_url);

    let staging_dir = config::cache_dir().join("launcher-staging");
    std::fs::create_dir_all(&staging_dir)
        .with_context(|| format!("creating launcher staging dir {}", staging_dir.display()))?;
    let staged = staging_dir.join(&launcher.filename);
    let staged_sig = staged.with_extension(format!(
        "{}.minisig",
        staged
            .extension()
            .and_then(|s| s.to_str())
            .unwrap_or("launcher")
    ));

    download::download_and_verify(
        &img_url,
        &sig_url,
        &staged,
        &staged_sig,
        &launcher.sha256,
        pubkey,
    )?;

    let current =
        config::current_exe_path().context("locating the running launcher binary (current_exe)")?;
    replace_binary(&current, &staged)
        .with_context(|| format!("swapping new launcher into place at {}", current.display()))?;

    eprintln!(
        "==> self-update applied — restart deputyos-desktop to run the new launcher ({})",
        launcher.filename
    );
    Ok(())
}

/// Atomically replace `current` with `staged`. Per-OS:
///
/// - **POSIX**: `chmod 0o755` the staged file then `rename` it over
///   `current`. `rename` is atomic on the same filesystem; the running
///   process keeps the old inode and the new bytes take effect next launch.
/// - **Windows**: a running `.exe` cannot be renamed over in place, so we
///   move the current exe aside to `<name>.exe.old`, move the new one into
///   place, then best-effort delete the `.old` (it may still be locked by
///   the running process; it's cleaned up on a future launch). If moving
///   the running exe aside fails, we bail with a clear "close all
///   deputyos-desktop windows and re-run `self-update`" message.
fn replace_binary(current: &Path, staged: &Path) -> Result<()> {
    replace_binary_impl(current, staged)
}

#[cfg(unix)]
fn replace_binary_impl(current: &Path, staged: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    // 0o755: rwxr-xr-x — executable, world-readable (the launcher is a
    // user-facing binary, not a secret).
    std::fs::set_permissions(staged, std::fs::Permissions::from_mode(0o755))
        .with_context(|| format!("chmod +x {}", staged.display()))?;
    std::fs::rename(staged, current)
        .with_context(|| format!("renaming {} -> {}", staged.display(), current.display()))?;
    Ok(())
}

#[cfg(windows)]
fn replace_binary_impl(current: &Path, staged: &Path) -> Result<()> {
    let old = current.with_extension("exe.old");
    // Move the running exe aside. This can fail with a sharing violation if
    // the binary is still executing — surface a clear actionable error.
    match std::fs::rename(current, &old) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => {
            return Err(anyhow!(
                "could not move the running launcher aside ({}): close all \
                 deputyos-desktop windows and re-run `deputyos-desktop self-update`",
                current.display()
            ));
        }
        Err(e) => {
            return Err(anyhow!("moving {} aside: {e}", current.display()));
        }
    }
    std::fs::rename(staged, current)
        .with_context(|| format!("renaming {} -> {}", staged.display(), current.display()))?;
    // Best-effort cleanup of the superseded binary; ignore failure (it may
    // still be locked by the running process — a future launch reaps it).
    let _ = std::fs::remove_file(&old);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use deputyctl::release::{DesktopLauncher, Manifest, ManifestSource};
    use std::collections::BTreeMap;

    /// Build a `ManifestSource` carrying a single `desktop_launchers` entry
    /// by writing a manifest JSON to a leaked tempdir and loading it — the
    /// same trick `manifest::tests::fake_source` uses (the tempdir must
    /// outlive the source because `ManifestSource` holds the on-disk path).
    fn source_with_launcher(launcher: DesktopLauncher) -> (ManifestSource, tempfile::TempDir) {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("manifest.json");
        let m = Manifest {
            schema_version: 1,
            release_version: "2026.6.22".into(),
            channel: "dev".into(),
            released_at: "2026-06-22T00:00:00Z".into(),
            tracker: BTreeMap::new(),
            artefacts: vec![deputyctl::release::Artefact {
                target: "qemu-x86_64".into(),
                profile: "openclaw".into(),
                filename: "deputyos-openclaw-qemu-x86_64-2026.6.22-dev.qcow2".into(),
                format: "qcow2".into(),
                size_bytes: 1024,
                sha256: "0".repeat(64),
                minisig_url: "2026.6.22/img.qcow2.minisig".into(),
                url: Some("2026.6.22/img.qcow2".into()),
            }],
            wizard_version: None,
            chat_ui_version: None,
            desktop_launchers: {
                let mut map = BTreeMap::new();
                map.insert(launcher.triple.clone(), launcher);
                map
            },
            mounts_policy_schema_version: 1,
        };
        std::fs::write(&path, serde_json::to_string(&m).expect("ser")).expect("write manifest");
        let real_path = path.to_str().expect("utf8").to_string();
        let src = deputyctl::release::load_manifest(&real_path).expect("load manifest");
        (src, dir)
    }

    fn env_guard(var: &str) -> Option<String> {
        std::env::var(var).ok()
    }

    fn set_self_exe(p: &std::path::Path) -> Option<String> {
        let prev = env_guard("DEPUTYOS_DESKTOP_SELF_EXE");
        std::env::set_var("DEPUTYOS_DESKTOP_SELF_EXE", p);
        prev
    }

    fn restore_self_exe(prev: Option<String>) {
        match prev {
            Some(v) => std::env::set_var("DEPUTYOS_DESKTOP_SELF_EXE", v),
            None => std::env::remove_var("DEPUTYOS_DESKTOP_SELF_EXE"),
        }
    }

    fn fake_launcher(triple: &str, sha: &str) -> DesktopLauncher {
        DesktopLauncher {
            triple: triple.into(),
            filename: format!("deputyos-desktop-{triple}"),
            url: "2026.6.22/launcher.bin".into(),
            sha256: sha.into(),
            minisig_url: "2026.6.22/launcher.bin.minisig".into(),
        }
    }

    #[test]
    fn check_up_to_date_when_running_sha_matches_manifest() {
        let _g = crate::config::test_env_lock()
            .lock()
            .expect("test_env_lock poisoned");
        // Fixture "current exe" whose sha we compute and pin into the
        // manifest entry → check must report up-to-date (None).
        let dir = tempfile::tempdir().expect("tempdir");
        let exe = dir.path().join("deputyos-desktop");
        std::fs::write(&exe, b"i am the current launcher").expect("write exe");
        let sha = sha256_hex(&exe).expect("hash exe");

        let triple = std::env::consts::ARCH; // any triple string works for the lookup
        let (src, _md) = source_with_launcher(fake_launcher("x86_64-unknown-linux-gnu", &sha));

        let prev = set_self_exe(&exe);
        let out = check(&src, "x86_64-unknown-linux-gnu").expect("check");
        restore_self_exe(prev);
        assert!(out.is_none(), "matching sha → up to date");
        let _ = triple;
    }

    #[test]
    fn check_reports_update_when_sha_differs() {
        let _g = crate::config::test_env_lock()
            .lock()
            .expect("test_env_lock poisoned");
        let dir = tempfile::tempdir().expect("tempdir");
        let exe = dir.path().join("deputyos-desktop");
        std::fs::write(&exe, b"old launcher bytes").expect("write exe");

        // Manifest pins a sha that is NOT the fixture's sha → Some.
        let (src, _md) =
            source_with_launcher(fake_launcher("x86_64-unknown-linux-gnu", &"f".repeat(64)));

        let prev = set_self_exe(&exe);
        let out = check(&src, "x86_64-unknown-linux-gnu").expect("check");
        restore_self_exe(prev);
        let entry = out.expect("mismatched sha → update available");
        assert_eq!(entry.triple, "x86_64-unknown-linux-gnu");
    }

    #[test]
    fn check_errors_when_triple_absent_lists_available() {
        let (src, _md) =
            source_with_launcher(fake_launcher("x86_64-unknown-linux-gnu", &"0".repeat(64)));
        let err = check(&src, "aarch64-apple-darwin").expect_err("absent triple");
        let msg = format!("{err:#}");
        assert!(msg.contains("aarch64-apple-darwin"), "got: {msg}");
        assert!(msg.contains("x86_64-unknown-linux-gnu"), "got: {msg}");
    }

    #[cfg(unix)]
    #[test]
    fn replace_binary_atomically_swaps_and_chmods_on_posix() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().expect("tempdir");
        let current = dir.path().join("launcher");
        let staged = dir.path().join("launcher.new");
        std::fs::write(&current, b"OLD").expect("write current");
        std::fs::write(&staged, b"NEW").expect("write staged");

        replace_binary(&current, &staged).expect("swap");

        // current now holds the staged bytes; staged path is gone; mode 0o755.
        assert_eq!(std::fs::read(&current).expect("read current"), b"NEW");
        assert!(!staged.exists(), "staged file was renamed away");
        let mode = std::fs::metadata(&current)
            .expect("meta")
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o755, "executable mode, got {:o}", mode);
    }
}
