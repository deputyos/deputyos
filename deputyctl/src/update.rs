//! `deputyctl update --check` and `deputyctl update --apply`.
//!
//! Reads the signed release manifest from a configured URL, verifies its
//! signature, and reports (or stages) the artefact appropriate for this
//! device's target + profile. The actual A/B slot swap lands in M6 — see
//! `docs/08-update-rollback.md` — so `--apply` ends at "staged at
//! /var/lib/deputyos/staging/<filename>" and prints a clear pointer.
//!
//! The trust chain is: manifest URL is fixed at build time
//! (`/etc/deputyos/update-url`); pubkey is fixed at build time
//! (`/etc/deputyos/pubkey.minisign`). Both are overridable via env in dev,
//! and both have dev fallbacks pointing at the local `dist/` so the loop
//! works on a contributor laptop without any infra.
//!
//! See `docs/02-profiles.md` §"deputyctl command surface" for the frozen
//! interface.

use std::io::{self, Write};
use std::path::{Path, PathBuf};

use crate::rollback::{write_slots, Slots};

use anyhow::{anyhow, bail, Context, Result};

use crate::release::{
    self, fetch_to, is_newer, load_manifest, verify_manifest_signature, verify_sha256, Artefact,
    Manifest, ManifestSource,
};

/// Location of the one-line update URL written at image bake.
fn update_url_file() -> PathBuf {
    PathBuf::from("/etc/deputyos/update-url")
}

/// Location of the one-line installed-version file written at image bake.
fn installed_version_file() -> PathBuf {
    PathBuf::from("/etc/deputyos/version")
}

/// Location of the public key that signed the manifest.
fn pubkey_file() -> PathBuf {
    PathBuf::from("/etc/deputyos/pubkey.minisign")
}

/// Location of the active-profile id (already used elsewhere; redefined
/// here so we don't add a new public path constant for one call site).
fn active_profile_file() -> PathBuf {
    PathBuf::from("/etc/deputyos/active-profile")
}

/// Where update artefacts are staged before A/B swap. Real swap lands in M6.
fn staging_dir() -> PathBuf {
    if let Ok(p) = std::env::var("DEPUTYOS_STAGING_DIR") {
        PathBuf::from(p)
    } else {
        PathBuf::from("/var/lib/deputyos/staging")
    }
}

/// Resolve the update URL with the documented precedence:
///   1. DEPUTYOS_UPDATE_URL env
///   2. /etc/deputyos/update-url
///   3. file://<repo>/dist/manifest.json (dev fallback for contributor laptops)
fn resolve_update_url() -> Result<String> {
    if let Ok(s) = std::env::var("DEPUTYOS_UPDATE_URL") {
        if !s.trim().is_empty() {
            return Ok(s);
        }
    }
    let f = update_url_file();
    if let Ok(s) = std::fs::read_to_string(&f) {
        let s = s.trim().to_string();
        if !s.is_empty() {
            return Ok(s);
        }
    }
    // Dev fallback — relative to current working directory.
    let cwd = std::env::current_dir().context("getting cwd for dev-fallback update URL")?;
    let candidate = cwd.join("dist").join("manifest.json");
    Ok(format!("file://{}", candidate.display()))
}

fn resolve_pubkey() -> Result<PathBuf> {
    if let Ok(s) = std::env::var("DEPUTYOS_UPDATE_PUBKEY") {
        if !s.trim().is_empty() {
            return Ok(PathBuf::from(s));
        }
    }
    let f = pubkey_file();
    if f.is_file() {
        return Ok(f);
    }
    let cwd = std::env::current_dir().context("getting cwd for dev-fallback pubkey")?;
    let candidate = cwd.join("dist").join("pubkey.minisign");
    if candidate.is_file() {
        return Ok(candidate);
    }
    // Final dev fallback — the contributor's dev keys directory.
    if let Some(home) = dirs_home() {
        let dev = home
            .join(".config")
            .join("deputyos")
            .join("dev-keys")
            .join("deputyos-dev.pub");
        if dev.is_file() {
            return Ok(dev);
        }
    }
    bail!(
        "no update pubkey found; tried DEPUTYOS_UPDATE_PUBKEY, /etc/deputyos/pubkey.minisign, ./dist/pubkey.minisign, ~/.config/deputyos/dev-keys/deputyos-dev.pub"
    )
}

fn dirs_home() -> Option<PathBuf> {
    std::env::var_os("HOME").map(PathBuf::from)
}

fn read_installed_version() -> String {
    if let Ok(s) = std::env::var("DEPUTYOS_INSTALLED_VERSION") {
        let s = s.trim().to_string();
        if !s.is_empty() {
            return s;
        }
    }
    if let Ok(raw) = std::fs::read_to_string(installed_version_file()) {
        let trimmed = raw.trim();
        if !trimmed.is_empty() {
            return trimmed.to_string();
        }
    }
    "0.0.0".to_string()
}

fn read_active_profile() -> String {
    if let Ok(s) = std::env::var("DEPUTYOS_ACTIVE_PROFILE") {
        let s = s.trim().to_string();
        if !s.is_empty() {
            return s;
        }
    }
    if let Ok(raw) = std::fs::read_to_string(active_profile_file()) {
        let trimmed = raw.trim();
        if !trimmed.is_empty() {
            return trimmed.to_string();
        }
    }
    // Dev fallback: dispatch on the DEPUTYOS_ACTIVE_PROFILE_FILE the test
    // harness already understands.
    if let Ok(p) = std::env::var("DEPUTYOS_ACTIVE_PROFILE_FILE") {
        if let Ok(raw) = std::fs::read_to_string(p) {
            let trimmed = raw.trim();
            if !trimmed.is_empty() {
                return trimmed.to_string();
            }
        }
    }
    "openclaw".to_string()
}

fn read_target_from_limits() -> String {
    if let Ok(s) = std::env::var("DEPUTYOS_TARGET") {
        let s = s.trim().to_string();
        if !s.is_empty() {
            return s;
        }
    }
    match crate::limits::load() {
        Ok(l) => l.target,
        Err(_) => "qemu-aarch64".to_string(),
    }
}

/// Fetch + verify the manifest. Used by both --check and --apply paths.
///
/// `from` is the sneakernet path (M4.5 Lane D): when set, the manifest is
/// read from that local file instead of the configured update URL, and its
/// `.minisig` sidecar is read from disk next to it — no network egress. The
/// minisign verification is identical to the online path.
fn fetch_verified_manifest(from: Option<&str>) -> Result<(ManifestSource, PathBuf)> {
    let url = match from {
        Some(p) => p.to_string(),
        None => resolve_update_url()?,
    };
    let pubkey = resolve_pubkey()?;
    let src = load_manifest(&url)?;
    // Signature sidecar lives at <manifest>.minisig. For an HTTP(S) manifest
    // the sidecar is remote and must be fetched into the manifest's tempdir.
    // For a local manifest (bare path or file://, incl. the sneakernet
    // `--from <usb>/manifest.json` case) the sig is already on disk next to
    // the manifest — read it in place. (Calling fetch_to on a bare path would
    // fs::copy the sig onto itself, truncating it to zero.)
    let sig_path = if url.starts_with("http://") || url.starts_with("https://") {
        let sig_url = format!("{url}.minisig");
        let sig_dest = src.local_path.with_file_name(format!(
            "{}.minisig",
            src.local_path
                .file_name()
                .and_then(|s| s.to_str())
                .unwrap_or("manifest.json")
        ));
        fetch_to(&sig_url, &sig_dest).with_context(|| format!("fetching {sig_url}"))?;
        sig_dest
    } else {
        PathBuf::from(format!("{}.minisig", src.local_path.display()))
    };
    verify_manifest_signature(&src.local_path, &sig_path, &pubkey)?;
    Ok((src, pubkey))
}

/// Pick the first artefact matching this device's target + profile.
fn pick_artefact<'a>(manifest: &'a Manifest, target: &str, profile: &str) -> Option<&'a Artefact> {
    manifest
        .artefacts
        .iter()
        .find(|a| a.target == target && a.profile == profile)
}

/// Run `deputyctl update --check`. `from` is the optional sneakernet manifest
/// path (`--from`); `None` means use the configured update URL.
pub fn run_check(json: bool, from: Option<&str>) -> Result<u8> {
    let installed = read_installed_version();
    let target = read_target_from_limits();
    let profile = read_active_profile();

    let (src, _pubkey) = match fetch_verified_manifest(from) {
        Ok(x) => x,
        Err(e) => {
            // Surface the failure plainly; this is a hard error (signature,
            // network, or missing pubkey).
            if json {
                let payload = serde_json::json!({
                    "ok": false,
                    "error": format!("{e:#}"),
                    "installed_version": installed,
                });
                println!("{}", serde_json::to_string_pretty(&payload)?);
            } else {
                eprintln!("update --check: {e:#}");
            }
            return Ok(1);
        }
    };
    let manifest = &src.manifest;
    let latest = manifest.release_version.clone();
    let update_available = is_newer(&latest, &installed);

    let matched = pick_artefact(manifest, &target, &profile);

    if json {
        let payload = serde_json::json!({
            "ok": true,
            "installed_version": installed,
            "latest_version": latest,
            "channel": manifest.channel,
            "update_available": update_available,
            "target": target,
            "profile": profile,
            "matching_artefact": matched,
            "manifest_origin": src.origin,
        });
        println!("{}", serde_json::to_string_pretty(&payload)?);
    } else {
        println!("manifest:    {}", src.origin);
        println!("channel:     {}", manifest.channel);
        println!("installed:   {installed}");
        println!("latest:      {latest}");
        println!(
            "status:      {}",
            if update_available {
                "update available"
            } else {
                "up to date"
            }
        );
        println!("target:      {target}");
        println!("profile:     {profile}");
        match matched {
            Some(a) => {
                println!("artefact:    {} ({})", a.filename, a.format);
                println!("size:        {} bytes", a.size_bytes);
                println!("sha256:      {}", a.sha256);
            }
            None => {
                println!(
                    "artefact:    (no matching artefact for target={target} profile={profile} in this manifest)"
                );
            }
        }
    }
    Ok(0)
}

/// Run `deputyctl update --apply`. `from` is the optional sneakernet manifest
/// path (`--from`); `None` means use the configured update URL. When `from`
/// is set, the artefact + its signature are resolved locally next to the
/// manifest (no network egress) — the same minisign + sha256 gates apply.
pub fn run_apply(yes: bool, json: bool, from: Option<&str>) -> Result<u8> {
    let installed = read_installed_version();
    let target = read_target_from_limits();
    let profile = read_active_profile();

    let (src, _pubkey) = match fetch_verified_manifest(from) {
        Ok(x) => x,
        Err(e) => {
            eprintln!("update --apply: {e:#}");
            return Ok(1);
        }
    };
    let manifest = &src.manifest;
    let latest = manifest.release_version.clone();

    let artefact = match pick_artefact(manifest, &target, &profile) {
        Some(a) => a.clone(),
        None => {
            eprintln!(
                "update --apply: no matching artefact for target={target} profile={profile} in {latest}"
            );
            return Ok(1);
        }
    };

    if !is_newer(&latest, &installed) {
        if json {
            let payload = serde_json::json!({
                "ok": true,
                "applied": false,
                "reason": "already up to date",
                "installed_version": installed,
                "latest_version": latest,
            });
            println!("{}", serde_json::to_string_pretty(&payload)?);
        } else {
            println!("update --apply: already up to date ({installed})");
        }
        return Ok(0);
    }

    if !yes {
        eprint!(
            "update --apply: stage {} ({} bytes) over installed version {installed}? [y/N] ",
            artefact.filename, artefact.size_bytes
        );
        let _ = io::stderr().flush();
        let mut buf = String::new();
        io::stdin()
            .read_line(&mut buf)
            .context("reading stdin for confirmation")?;
        let trimmed = buf.trim();
        if !matches!(trimmed, "y" | "Y" | "yes" | "Yes") {
            eprintln!("update --apply: aborted by user");
            return Ok(1);
        }
    }

    run_pre_update_backup();

    // Resolve URLs against the manifest origin.
    let url_field = artefact
        .url
        .clone()
        .unwrap_or_else(|| artefact.filename.clone());
    let artefact_url = release::resolve_url(&src.origin, &url_field);
    let sig_url = release::resolve_url(&src.origin, &artefact.minisig_url);

    let staging = staging_dir();
    std::fs::create_dir_all(&staging)
        .with_context(|| format!("creating staging dir {}", staging.display()))?;
    let staged = staging.join(&artefact.filename);
    let staged_sig = staging.join(format!("{}.minisig", artefact.filename));

    eprintln!("update --apply: downloading {artefact_url}");
    fetch_to(&artefact_url, &staged)
        .with_context(|| format!("downloading artefact from {artefact_url}"))?;
    eprintln!("update --apply: downloading {sig_url}");
    fetch_to(&sig_url, &staged_sig)
        .with_context(|| format!("downloading signature from {sig_url}"))?;

    eprintln!("update --apply: verifying sha256");
    verify_sha256(&staged, &artefact.sha256)?;

    eprintln!("update --apply: verifying minisign signature");
    let pubkey = resolve_pubkey()?;
    verify_artefact_signature(&staged, &staged_sig, &pubkey)?;

    // Perform A/B swap: write and re-verify the inactive slot, then update the
    // bootloader. Host-distribution artefacts are rejected inside swap_slots.
    let swapped = swap_slots(&artefact, &staged, &latest)?;

    // Fire `update-applied` only after the verified slot and boot selection
    // both succeeded. Hook failures are logged but do not undo the update.
    let hook_payload = serde_json::json!({
        "kind": "update-applied",
        "staged_at": staged.display().to_string(),
        "filename": artefact.filename,
        "sha256": artefact.sha256,
        "release_version": latest,
    });
    let _ = crate::hooks::fire_hook(crate::hooks::HookKind::UpdateApplied, &hook_payload);

    if json {
        let payload = serde_json::json!({
            "ok": true,
            "applied": true,
            "swapped_to_slot": swapped.inactive,
            "new_version": latest,
            "filename": artefact.filename,
            "sha256": artefact.sha256,
            "reboot_required": true,
        });
        println!("{}", serde_json::to_string_pretty(&payload)?);
    } else {
        println!(
            "update --apply: swapped to slot {} (version {latest})",
            swapped.inactive
        );
        println!("update --apply: reboot to boot into the new version");
    }
    Ok(0)
}

fn run_pre_update_backup() {
    if std::env::var("DEPUTYOS_SKIP_PRE_UPDATE_BACKUP").as_deref() == Ok("1") {
        return;
    }
    let managed = crate::paths::cloud_backup_token_file().is_file()
        && crate::paths::backup_recovery_key_file().is_file();
    let self_managed = crate::paths::backup_config_file().is_file();
    if !managed && !self_managed {
        return;
    }
    eprintln!("update --apply: creating best-effort pre-update backup");
    match crate::backup::run_now(crate::backup::NowOpts {
        dry_run: false,
        to_cloud: managed,
    }) {
        Ok(0) => eprintln!("update --apply: pre-update backup complete"),
        Ok(code) => eprintln!(
            "update --apply: warning: pre-update backup exited {code}; continuing verified A/B update"
        ),
        Err(error) => eprintln!(
            "update --apply: warning: pre-update backup failed: {error:#}; continuing verified A/B update"
        ),
    }
}

/// Write the artefact to the inactive slot and update the bootloader.
fn swap_slots(artefact: &super::release::Artefact, staged: &Path, latest: &str) -> Result<Slots> {
    let slots = load_or_init_slots()?;
    if !matches!(artefact.format.as_str(), "raw" | "img" | "rootfs") {
        bail!(
            "artefact format '{}' is a host-distribution image, not an in-guest A/B payload; the desktop updater must replace it",
            artefact.format
        );
    }
    let target_slot = slots.inactive.clone();
    let inactive_image = match target_slot.as_str() {
        "A" => slots.slot_path_a.as_deref(),
        "B" => slots.slot_path_b.as_deref(),
        other => bail!("unknown inactive slot '{other}'"),
    }
    .ok_or_else(|| {
        anyhow!(
            "A/B slot {} has no explicit destination; refusing to guess a boot path",
            target_slot
        )
    })?;
    let inactive_image = Path::new(inactive_image);
    if !(inactive_image.starts_with("/dev")
        || inactive_image.starts_with("/var/lib/deputyos/slots"))
    {
        bail!(
            "inactive slot destination {} is outside the allowed slot roots",
            inactive_image.display()
        );
    }

    if let Some(parent) = inactive_image.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating {}", parent.display()))?;
    };
    std::fs::copy(staged, inactive_image).with_context(|| {
        format!(
            "copying verified update payload to {}",
            inactive_image.display()
        )
    })?;
    crate::release::verify_sha256(inactive_image, &artefact.sha256)
        .context("verifying written inactive slot")?;
    crate::rollback::select_boot_slot(&slots, &target_slot)?;

    // Update slots.json.
    let updated = Slots {
        active: slots.inactive.clone(),
        inactive: slots.active.clone(),
        version_a: if slots.active == "A" {
            slots.version_a
        } else {
            latest.to_string()
        },
        version_b: if slots.active == "B" {
            slots.version_b
        } else {
            latest.to_string()
        },
        inactive_sha256: Some(artefact.sha256.clone()),
        inactive_image_path: Some(inactive_image.display().to_string()),
        boot_count: Some(0),
        pending_rollback: Some(false),
        auto_rollback: None,
        update_pending: Some(true),
        pending_version: Some(latest.to_string()),
        slot_path_a: slots.slot_path_a,
        slot_path_b: slots.slot_path_b,
    };
    write_slots(&updated)?;
    Ok(updated)
}

fn load_or_init_slots() -> Result<Slots> {
    let path = Path::new("/var/lib/deputyos/slots.json");
    if path.is_file() {
        let raw = std::fs::read_to_string(path).context("reading slots.json")?;
        serde_json::from_str(&raw).context("parsing slots.json")
    } else {
        // Initialise on first update.
        Ok(Slots {
            active: "A".into(),
            inactive: "B".into(),
            version_a: "0".into(),
            version_b: "0".into(),
            inactive_sha256: None,
            inactive_image_path: None,
            boot_count: Some(0),
            pending_rollback: None,
            auto_rollback: None,
            update_pending: Some(false),
            pending_version: None,
            slot_path_a: None,
            slot_path_b: None,
        })
    }
}

/// Timer entrypoint. Every image checks signed metadata automatically.
/// Platforms with provisioned A/B slots may opt into `apply` by writing that
/// exact word to `/etc/deputyos/auto-update`; immutable VM formats remain
/// desktop-managed and therefore use the default `check` mode.
pub fn run_automatic() -> Result<u8> {
    let mode = std::fs::read_to_string("/etc/deputyos/auto-update")
        .unwrap_or_else(|_| "check".to_string());
    if mode.trim() == "apply" {
        run_apply(true, true, None)
    } else {
        run_check(true, None)
    }
}

/// Wrapper around minisign for an artefact (vs. the manifest). Identical
/// semantics; named separately so error messages call out which file failed.
fn verify_artefact_signature(art: &Path, sig: &Path, pubkey: &Path) -> Result<()> {
    let status = std::process::Command::new("minisign")
        .arg("-V")
        .arg("-p")
        .arg(pubkey)
        .arg("-m")
        .arg(art)
        .arg("-x")
        .arg(sig)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .output()
        .map_err(|e| anyhow!("running minisign: {e}"))?;
    if !status.status.success() {
        bail!(
            "artefact signature verification failed for {}: {}",
            art.display(),
            String::from_utf8_lossy(&status.stderr).trim()
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::{Command, Stdio};

    fn minisign_present() -> bool {
        Command::new("minisign")
            .arg("-h")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_ok()
    }

    /// End-to-end --check against a synthetic dist tree.
    #[test]
    fn check_against_synthetic_dist() {
        if !minisign_present() {
            eprintln!("test: minisign not installed; skipping");
            return;
        }

        let dir = tempfile::tempdir().expect("tempdir");
        let dist = dir.path().join("dist");
        std::fs::create_dir_all(&dist).expect("mkdir dist");

        // Generate a keypair.
        let pubkey = dist.join("pubkey.minisign");
        let seckey = dir.path().join("dev.key");
        let g = Command::new("minisign")
            .args(["-G", "-W", "-p"])
            .arg(&pubkey)
            .arg("-s")
            .arg(&seckey)
            .status()
            .expect("genkey");
        assert!(g.success());

        // Stage a fake artefact.
        let version = "2026.4.27";
        let versioned = dist.join(version);
        std::fs::create_dir_all(&versioned).expect("mkdir versioned");
        let art_name = "deputyos-openclaw-qemu-aarch64-2026.4.27-dev.qcow2";
        let art_path = versioned.join(art_name);
        std::fs::write(&art_path, b"hello world fake artefact bytes").expect("art");
        let art_sha = sha256_hex(&art_path);

        // Sign the artefact.
        let s = Command::new("minisign")
            .args(["-S", "-W", "-s"])
            .arg(&seckey)
            .arg("-m")
            .arg(&art_path)
            .status()
            .expect("sign art");
        assert!(s.success());

        // Write manifest and sign it.
        let manifest_path = dist.join("manifest.json");
        let manifest = serde_json::json!({
            "schema_version": 1,
            "release_version": version,
            "channel": "dev",
            "released_at": "2026-04-27T00:00:00Z",
            "artefacts": [{
                "target": "qemu-aarch64",
                "profile": "openclaw",
                "filename": art_name,
                "format": "qcow2",
                "size_bytes": std::fs::metadata(&art_path).expect("md").len(),
                "sha256": art_sha,
                "minisig_url": format!("{version}/{art_name}.minisig"),
                "url": format!("{version}/{art_name}"),
            }]
        });
        std::fs::write(
            &manifest_path,
            serde_json::to_string(&manifest).expect("ser"),
        )
        .expect("write manifest");
        let s = Command::new("minisign")
            .args(["-S", "-W", "-s"])
            .arg(&seckey)
            .arg("-m")
            .arg(&manifest_path)
            .status()
            .expect("sign manifest");
        assert!(s.success());

        // Snapshot the manifest into the versioned dir.
        std::fs::copy(&manifest_path, versioned.join("manifest.json")).expect("snap");
        std::fs::copy(
            manifest_path.with_extension("json.minisig"),
            versioned.join("manifest.json.minisig"),
        )
        .expect("snap sig");

        // Point deputyctl at this synthetic dist.
        let url = format!("file://{}", manifest_path.display());
        std::env::set_var("DEPUTYOS_UPDATE_URL", &url);
        std::env::set_var("DEPUTYOS_UPDATE_PUBKEY", &pubkey);
        std::env::set_var("DEPUTYOS_INSTALLED_VERSION", "2026.4.20"); // older
        std::env::set_var("DEPUTYOS_TARGET", "qemu-aarch64");
        std::env::set_var("DEPUTYOS_ACTIVE_PROFILE", "openclaw");

        // Check should report update available.
        let code = run_check(true, None).expect("run check");
        assert_eq!(code, 0);

        // Bad signature: corrupt the manifest after-the-fact.
        std::fs::OpenOptions::new()
            .append(true)
            .open(&manifest_path)
            .expect("open append")
            .write_all(b"X")
            .expect("write");
        let code = run_check(false, None).expect("bad sig run");
        assert_eq!(code, 1, "bad sig must exit 1");

        // Cleanup env so we don't pollute other tests.
        std::env::remove_var("DEPUTYOS_UPDATE_URL");
        std::env::remove_var("DEPUTYOS_UPDATE_PUBKEY");
        std::env::remove_var("DEPUTYOS_INSTALLED_VERSION");
        std::env::remove_var("DEPUTYOS_TARGET");
        std::env::remove_var("DEPUTYOS_ACTIVE_PROFILE");
    }

    fn sha256_hex(path: &Path) -> String {
        let out = Command::new("sha256sum").arg(path).output().expect("sha");
        let s = String::from_utf8_lossy(&out.stdout);
        s.split_whitespace().next().unwrap_or("").to_string()
    }

    /// `--from <path>` reads the signed manifest + its sidecar sig from disk
    /// (sneakernet, M4.5 Lane D) — no DEPUTYOS_UPDATE_URL set, so the only way
    /// this resolves is via the explicit local path. Same minisign gate.
    #[test]
    fn check_from_local_manifest_path() {
        if !minisign_present() {
            eprintln!("test: minisign not installed; skipping");
            return;
        }
        let dir = tempfile::tempdir().expect("tempdir");
        let usb = dir.path().join("usb");
        std::fs::create_dir_all(&usb).expect("mkdir usb");

        // Keypair.
        let pubkey = dir.path().join("pubkey.minisign");
        let seckey = dir.path().join("dev.key");
        let g = Command::new("minisign")
            .args(["-G", "-W", "-p"])
            .arg(&pubkey)
            .arg("-s")
            .arg(&seckey)
            .status()
            .expect("genkey");
        assert!(g.success());

        // Manifest + signature on the "USB stick".
        let manifest_path = usb.join("manifest.json");
        let manifest = serde_json::json!({
            "schema_version": 1,
            "release_version": "2026.4.27",
            "channel": "dev",
            "released_at": "2026-04-27T00:00:00Z",
            "artefacts": [{
                "target": "qemu-aarch64",
                "profile": "openclaw",
                "filename": "deputyos-openclaw-qemu-aarch64-2026.4.27-dev.qcow2",
                "format": "qcow2",
                "size_bytes": 64u64,
                "sha256": "0000000000000000000000000000000000000000000000000000000000000000",
                "minisig_url": "deputyos-openclaw-qemu-aarch64-2026.4.27-dev.qcow2.minisig",
                "url": "deputyos-openclaw-qemu-aarch64-2026.4.27-dev.qcow2",
            }]
        });
        std::fs::write(
            &manifest_path,
            serde_json::to_string(&manifest).expect("ser"),
        )
        .expect("write manifest");
        let s = Command::new("minisign")
            .args(["-S", "-W", "-s"])
            .arg(&seckey)
            .arg("-m")
            .arg(&manifest_path)
            .status()
            .expect("sign manifest");
        assert!(s.success());
        // The sidecar minisign writes is `manifest.json.minisig`.
        let sig_path = manifest_path.with_extension("json.minisig");
        assert!(sig_path.is_file(), "minisign wrote the sidecar sig");

        // NO DEPUTYOS_UPDATE_URL set — the only resolution path is `--from`.
        std::env::remove_var("DEPUTYOS_UPDATE_URL");
        std::env::set_var("DEPUTYOS_UPDATE_PUBKEY", &pubkey);
        std::env::set_var("DEPUTYOS_INSTALLED_VERSION", "2026.4.20");
        std::env::set_var("DEPUTYOS_TARGET", "qemu-aarch64");
        std::env::set_var("DEPUTYOS_ACTIVE_PROFILE", "openclaw");

        let code = run_check(true, manifest_path.to_str()).expect("run check --from");
        assert_eq!(code, 0, "--from resolves + verifies the local manifest");

        // A missing sidecar sig must fail the gate (no silent accept).
        std::fs::remove_file(&sig_path).expect("rm sig");
        let code = run_check(false, manifest_path.to_str()).expect("run check --from no sig");
        assert_eq!(code, 1, "missing sidecar sig must fail");

        std::env::remove_var("DEPUTYOS_UPDATE_PUBKEY");
        std::env::remove_var("DEPUTYOS_INSTALLED_VERSION");
        std::env::remove_var("DEPUTYOS_TARGET");
        std::env::remove_var("DEPUTYOS_ACTIVE_PROFILE");
    }
}
