//! `deputyctl restore --list` and `deputyctl restore --snapshot <id>`.
//!
//! `--list` calls `rclone lsd` against the configured remote and parses the
//! per-snapshot directory names. `--snapshot` performs an atomic restore:
//! stop the active unit → move current data dir aside → `rclone copy` →
//! start the unit. On any failure mid-flight the moved-aside copy is
//! restored.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{bail, Context, Result};
use serde::Serialize;

use crate::backup;
use crate::paths;
use crate::profile;
use crate::systemd;

#[derive(Debug, Clone, Default)]
pub struct ListOpts {
    pub json: bool,
}

#[derive(Debug, Clone, Default)]
pub struct SnapshotOpts {
    pub id: String,
    pub yes: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct Snapshot {
    pub id: String,
    pub size: Option<String>,
    pub iso_timestamp: Option<String>,
}

fn rclone_available() -> bool {
    Command::new("rclone")
        .arg("version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

fn read_remote_root() -> Result<String> {
    let cfg = backup::load_config().context("loading backup config")?;
    let host = hostname_string();
    let base = cfg.remote.trim_end_matches('/').to_string();
    Ok(format!("{base}/{host}"))
}

fn hostname_string() -> String {
    std::fs::read_to_string("/etc/hostname")
        .map(|s| s.trim().to_string())
        .ok()
        .filter(|s| !s.is_empty())
        .or_else(|| std::env::var("HOSTNAME").ok())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "deputyos".into())
}

pub fn run_list(opts: ListOpts) -> Result<u8> {
    if !rclone_available() {
        eprintln!("restore --list: rclone not installed — run `make doctor`");
        return Ok(1);
    }
    let root = match read_remote_root() {
        Ok(r) => r,
        Err(e) => {
            eprintln!("restore --list: {e:#}");
            return Ok(1);
        }
    };
    let out = Command::new("rclone")
        .arg("lsd")
        .arg(&root)
        .arg("--config")
        .arg(paths::rclone_config_file())
        .output()
        .with_context(|| format!("spawning rclone lsd {root}"))?;
    if !out.status.success() {
        eprintln!("restore --list: rclone lsd failed:");
        eprint!("{}", String::from_utf8_lossy(&out.stderr));
        return Ok(1);
    }
    let snapshots = parse_lsd_output(&String::from_utf8_lossy(&out.stdout));
    if opts.json {
        println!("{}", serde_json::to_string_pretty(&snapshots)?);
    } else if snapshots.is_empty() {
        println!("(no snapshots)");
    } else {
        for s in &snapshots {
            println!(
                "{:<40} {:<10} {}",
                s.id,
                s.size.clone().unwrap_or_else(|| "?".into()),
                s.iso_timestamp.clone().unwrap_or_default()
            );
        }
    }
    Ok(0)
}

/// Parse rclone's `lsd` output:
///     -1 2026-04-26 12:00:00         3 snapshot-id
fn parse_lsd_output(raw: &str) -> Vec<Snapshot> {
    let mut out = Vec::new();
    for line in raw.lines() {
        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.len() < 5 {
            continue;
        }
        // parts[0]=size, parts[1]=date, parts[2]=time, parts[3]=count, parts[4..]=name
        let date = parts[1];
        let time = parts[2];
        let id = parts[4..].join(" ");
        out.push(Snapshot {
            id,
            size: Some(parts[0].to_string()),
            iso_timestamp: Some(format!("{date}T{time}Z")),
        });
    }
    out
}

pub fn run_snapshot(opts: SnapshotOpts) -> Result<u8> {
    if opts.id.trim().is_empty() {
        eprintln!("restore --snapshot: empty snapshot id");
        return Ok(64);
    }
    if !rclone_available() {
        eprintln!("restore --snapshot: rclone not installed — run `make doctor`");
        return Ok(1);
    }
    let root = match read_remote_root() {
        Ok(r) => r,
        Err(e) => {
            eprintln!("restore --snapshot: {e:#}");
            return Ok(1);
        }
    };
    let (_id, manifest) = profile::load_active().context("loading active profile")?;
    let data_dir = PathBuf::from(&manifest.paths.data_dir);

    if !opts.yes && std::io::IsTerminal::is_terminal(&std::io::stdin()) {
        use std::io::Write;
        eprint!(
            "this will replace the current data dir at {} with snapshot {}; continue? [y/N] ",
            data_dir.display(),
            opts.id
        );
        let _ = std::io::stderr().flush();
        let mut buf = String::new();
        std::io::stdin().read_line(&mut buf)?;
        if !matches!(buf.trim(), "y" | "Y" | "yes" | "Yes") {
            eprintln!("restore --snapshot: aborted");
            return Ok(1);
        }
    }

    // 1. Stop unit (best effort).
    let unit = manifest.service.unit.clone();
    if systemd::available() {
        let _ = systemd::run("stop", &unit);
    }

    // 2. Move current data dir aside.
    let backup_path = data_dir.with_extension(format!("pre-restore-{}", utc_iso_filename_safe()));
    let restored_data_existed = data_dir.exists();
    if restored_data_existed {
        std::fs::rename(&data_dir, &backup_path).with_context(|| {
            format!(
                "moving {} aside to {}",
                data_dir.display(),
                backup_path.display()
            )
        })?;
    }

    // 3. rclone copy <root>/<id>/ <data_dir>/
    std::fs::create_dir_all(&data_dir)
        .with_context(|| format!("creating fresh {}", data_dir.display()))?;
    let src = format!("{root}/{}/", opts.id);
    let copy = Command::new("rclone")
        .arg("copy")
        .arg(&src)
        .arg(&data_dir)
        .arg("--config")
        .arg(paths::rclone_config_file())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .status();
    let copy_ok = matches!(copy, Ok(s) if s.success());

    if !copy_ok {
        eprintln!("restore --snapshot: rclone copy failed; rolling back");
        // Roll back: remove the new (partial) data_dir; restore backup.
        let _ = std::fs::remove_dir_all(&data_dir);
        if restored_data_existed {
            if let Err(e) = std::fs::rename(&backup_path, &data_dir) {
                eprintln!(
                    "restore --snapshot: ROLLBACK FAILED renaming {} -> {}: {e}",
                    backup_path.display(),
                    data_dir.display()
                );
                return Ok(1);
            }
        }
        if systemd::available() {
            let _ = systemd::run("start", &unit);
        }
        return Ok(1);
    }

    // 4. Start unit.
    if systemd::available() {
        let _ = systemd::run("start", &unit);
    }
    println!(
        "restored: snapshot {} into {} (rollback copy at {})",
        opts.id,
        data_dir.display(),
        backup_path.display()
    );
    Ok(0)
}

#[derive(Debug, Clone, Default)]
pub struct FromCloudOpts {
    pub id: String,
    pub yes: bool,
}

/// One restorable component: a path inside the extracted bundle and the
/// canonical on-disk destination it should be placed at.
struct Component {
    name: &'static str,
    src: PathBuf,
    dest: PathBuf,
    is_file: bool,
}

/// A component that has been moved aside and replaced, tracked for rollback.
struct Placed {
    dest: PathBuf,
    aside: Option<PathBuf>,
}

impl Placed {
    /// Remove what we placed at `dest` and restore the moved-aside original.
    fn rollback(&self) -> Result<()> {
        let _ = std::fs::remove_dir_all(&self.dest).or_else(|_| std::fs::remove_file(&self.dest));
        if let Some(aside) = &self.aside {
            std::fs::rename(aside, &self.dest).with_context(|| {
                format!(
                    "rolling back {} -> {}",
                    aside.display(),
                    self.dest.display()
                )
            })?;
        }
        Ok(())
    }
}

/// Build the list of components to restore from a modern (BUNDLE.json) bundle.
fn build_component_plan(
    staging: &Path,
    manifest: &crate::manifest::Manifest,
    data_dir: &Path,
) -> Vec<Component> {
    let mut plan = vec![
        Component {
            name: "data",
            src: staging.join("data"),
            dest: data_dir.to_path_buf(),
            is_file: false,
        },
        Component {
            name: "hooks",
            src: staging.join("hooks"),
            dest: paths::hooks_dir(),
            is_file: false,
        },
        Component {
            name: "secrets",
            src: staging.join("secrets.env"),
            dest: paths::secrets_file(),
            is_file: true,
        },
    ];
    if let Some(session_db) = manifest
        .memory
        .as_ref()
        .and_then(|m| m.session_db.as_deref())
    {
        plan.push(Component {
            name: "session.db",
            src: staging.join("session.db"),
            dest: PathBuf::from(session_db),
            is_file: true,
        });
    }
    plan
}

/// `<dest>.pre-restore-<ts>` — a sibling path to move the current dest aside to.
fn aside_path(dest: &Path, ts: &str) -> PathBuf {
    PathBuf::from(format!("{}.pre-restore-{ts}", dest.display()))
}

/// Move `dest` aside (if it exists), then place `src` at `dest` via copy (so it
/// works across filesystems). On placement failure, roll this component back.
/// Returns a `Placed` handle for end-to-end rollback on a later failure.
fn move_aside_and_place(src: &Path, dest: &Path, is_file: bool, ts: &str) -> Result<Placed> {
    let aside = if dest.exists() {
        let a = aside_path(dest, ts);
        std::fs::rename(dest, &a)
            .with_context(|| format!("moving {} aside to {}", dest.display(), a.display()))?;
        Some(a)
    } else {
        None
    };

    let place = if is_file {
        std::fs::copy(src, dest)
            .map(|_| ())
            .map_err(anyhow::Error::from)
    } else {
        std::fs::create_dir_all(dest).ok();
        backup::copy_dir_recursive(src, dest)
    };

    if let Err(e) = place {
        if let Some(aside) = &aside {
            let _ = std::fs::remove_dir_all(dest).or_else(|_| std::fs::remove_file(dest));
            let _ = std::fs::rename(aside, dest);
        }
        return Err(e);
    }

    if is_file {
        // secrets.env / session.db are sensitive — restore them mode 0600.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(dest, std::fs::Permissions::from_mode(0o600));
        }
    }

    Ok(Placed {
        dest: dest.to_path_buf(),
        aside,
    })
}

/// Decrypt an age-encrypted file using an identity derived from the supplied
/// recovery secret. MANDATORY — there is no unencrypted fallback.
///
/// `agekey::derive_identity` turns it into the age identity that decrypts the
/// bundle device A encrypted to its
/// recipient. The identity is piped to `age -d -i -` (age reads it from stdin,
/// the ciphertext from the `enc` file arg) — no tty, so this runs unattended.
pub(crate) fn decrypt_with_age(enc: &Path, out: &Path, secret: &str) -> Result<()> {
    let identity = crate::agekey::derive_identity(secret)?;
    let mut child = Command::new("age")
        .args(["-d", "-i", "-", "-o"])
        .arg(out)
        .arg(enc)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .context("spawning age — install it (e.g. `sudo apt install age`) for cloud restore")?;
    if let Some(mut stdin) = child.stdin.take() {
        use std::io::Write;
        stdin.write_all(identity.as_bytes())?;
        stdin.write_all(b"\n")?;
    }
    let output = child.wait_with_output().context("waiting for age")?;
    if !output.status.success() {
        bail!(
            "age decryption failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(())
}

/// Snapshot ids are alphanumeric + `-_.`, max 64 chars (matches the API check).
fn valid_snapshot_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 64
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.')
}

/// `deputyctl restore --from cloud` — download an age-encrypted bundle from the
/// deputyOS cloud API, decrypt it with the stable recovery key, and atomically
/// restore each component (data_dir, hooks, secrets.env, session.db) into
/// place. The backup token is only the download bearer. A token-derived
/// decrypt fallback remains for schema-v2 bundles created before recovery keys.
/// `DEPUTYOS_API_BASE` overrides the hostname for local E2E.
pub fn run_from_cloud(opts: FromCloudOpts) -> Result<u8> {
    if opts.id.trim().is_empty() {
        eprintln!("restore --from cloud: empty snapshot id");
        return Ok(64);
    }
    if !valid_snapshot_id(&opts.id) {
        eprintln!(
            "restore --from cloud: invalid snapshot id {:?} (alphanumeric, '-', '_', '.', max 64 chars)",
            opts.id
        );
        return Ok(64);
    }

    let token_path = paths::cloud_backup_token_file();
    if !token_path.is_file() {
        eprintln!(
            "restore --from cloud: no backup token at {}",
            token_path.display()
        );
        eprintln!("  the backup token was written when this device was registered;");
        eprintln!(
            "  place a token at {} (mode 0600) to restore",
            token_path.display()
        );
        return Ok(1);
    }
    let token = std::fs::read_to_string(&token_path)
        .with_context(|| format!("reading {}", token_path.display()))?
        .trim()
        .to_string();
    if token.is_empty() {
        eprintln!("restore --from cloud: backup token is empty");
        return Ok(1);
    }

    let (_profile_id, manifest) = profile::load_active().context("loading active profile")?;
    let data_dir = PathBuf::from(&manifest.paths.data_dir);

    if !opts.yes && std::io::IsTerminal::is_terminal(&std::io::stdin()) {
        use std::io::Write;
        eprint!(
            "this will replace the current data dir at {} with cloud snapshot {}; continue? [y/N] ",
            data_dir.display(),
            opts.id
        );
        let _ = std::io::stderr().flush();
        let mut buf = String::new();
        std::io::stdin().read_line(&mut buf)?;
        if !matches!(buf.trim(), "y" | "Y" | "yes" | "Yes") {
            eprintln!("restore --from cloud: aborted");
            return Ok(1);
        }
    }

    let api_base = std::env::var("DEPUTYOS_API_BASE")
        .unwrap_or_else(|_| "https://api.deputyos.com".to_string());
    let url = format!(
        "{}/api/v1/backup/{}",
        api_base.trim_end_matches('/'),
        opts.id
    );

    // 1. Download the age-encrypted bundle.
    let enc_path = Path::new("/var/tmp").join(format!("{}.age", opts.id));
    eprintln!(
        "restore --from cloud: downloading {} from {}",
        opts.id, api_base
    );
    let resp = ureq::get(&url)
        .set("Authorization", &format!("Bearer {token}"))
        .call();
    let resp = match resp {
        Ok(r) => r,
        Err(ureq::Error::Status(401, _)) => {
            eprintln!("restore --from cloud: unauthorized (401) — backup token rejected or device revoked");
            return Ok(1);
        }
        Err(ureq::Error::Status(404, _)) => {
            eprintln!("restore --from cloud: no such snapshot {} (404)", opts.id);
            return Ok(1);
        }
        Err(ureq::Error::Status(status, _)) => {
            eprintln!("restore --from cloud: download failed ({status})");
            return Ok(1);
        }
        Err(ureq::Error::Transport(t)) => {
            eprintln!("restore --from cloud: download error: {t}");
            return Ok(1);
        }
    };
    {
        let mut file = std::fs::File::create(&enc_path)
            .with_context(|| format!("creating {}", enc_path.display()))?;
        let mut reader = resp.into_reader();
        std::io::copy(&mut reader, &mut file).context("writing downloaded bundle")?;
    }

    // 2. age-decrypt (mandatory). New bundles use the stable recovery secret.
    //    Fall back to the token-derived legacy identity for schema-v2 bundles.
    let tar_path = Path::new("/var/tmp").join(format!("{}.tar.gz", opts.id));
    let recovery_error = match crate::recovery_key::load() {
        Ok(secret) => decrypt_with_age(&enc_path, &tar_path, &secret).err(),
        Err(error) => Some(error),
    };
    if let Some(recovery_error) = recovery_error {
        std::fs::remove_file(&tar_path).ok();
        if let Err(legacy_error) = decrypt_with_age(&enc_path, &tar_path, &token) {
            eprintln!("restore --from cloud: recovery-key decrypt failed: {recovery_error:#}");
            eprintln!("restore --from cloud: legacy decrypt failed: {legacy_error:#}");
            eprintln!(
                "  import the matching key with `deputyctl backup recovery-key import <file>`"
            );
            std::fs::remove_file(&enc_path).ok();
            return Ok(1);
        }
        eprintln!("restore --from cloud: using legacy token-derived decryption");
    }
    std::fs::remove_file(&enc_path).ok();

    // 3. Extract the bundle.
    let staging = Path::new("/var/tmp").join(format!("{}.extract", opts.id));
    if staging.exists() {
        std::fs::remove_dir_all(&staging).ok();
    }
    std::fs::create_dir_all(&staging)?;
    let extract = Command::new("tar")
        .args([
            "-xzf",
            &tar_path.to_string_lossy(),
            "-C",
            &staging.to_string_lossy(),
        ])
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .status()
        .context("extracting bundle tar")?;
    std::fs::remove_file(&tar_path).ok();
    if !extract.success() {
        eprintln!("restore --from cloud: tar extraction failed");
        std::fs::remove_dir_all(&staging).ok();
        return Ok(1);
    }

    // 4. Read BUNDLE.json (modern component bundle) or fall back to legacy
    //    data_dir-only restore (pre-Lane-F bundles had no BUNDLE.json).
    let components: Vec<Component> = if staging.join(backup::BUNDLE_MANIFEST_NAME).is_file() {
        let bundle_path = staging.join(backup::BUNDLE_MANIFEST_NAME);
        let bundle: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(&bundle_path)
                .with_context(|| format!("reading {}", bundle_path.display()))?,
        )
        .context("parsing BUNDLE.json")?;
        let sv = bundle
            .get("schema_version")
            .and_then(|v| v.as_u64())
            .unwrap_or(0) as u32;
        if sv != backup::BUNDLE_SCHEMA_VERSION {
            eprintln!(
                "restore --from cloud: WARN bundle schema_version={sv}, expected {}; attempting restore anyway",
                backup::BUNDLE_SCHEMA_VERSION
            );
        }
        if let Some(included) = bundle.get("included").and_then(|v| v.as_array()) {
            let names: Vec<&str> = included.iter().filter_map(|x| x.as_str()).collect();
            eprintln!(
                "restore --from cloud: bundle includes: {}",
                names.join(", ")
            );
        }
        build_component_plan(&staging, &manifest, &data_dir)
    } else {
        eprintln!("restore --from cloud: no BUNDLE.json — legacy data_dir-only bundle");
        vec![Component {
            name: "data",
            src: staging.clone(),
            dest: data_dir.clone(),
            is_file: false,
        }]
    };

    // 5. Atomic restore: stop unit, move-aside + place each component, start unit.
    let unit = manifest.service.unit.clone();
    if systemd::available() {
        let _ = systemd::run("stop", &unit);
    }
    let ts = utc_iso_filename_safe();

    let mut placed: Vec<Placed> = Vec::new();
    for comp in &components {
        if !comp.src.exists() {
            continue; // component not in this bundle
        }
        match move_aside_and_place(&comp.src, &comp.dest, comp.is_file, &ts) {
            Ok(p) => {
                eprintln!(
                    "restore --from cloud: restored {} -> {}",
                    comp.name,
                    comp.dest.display()
                );
                placed.push(p);
            }
            Err(e) => {
                eprintln!(
                    "restore --from cloud: placement of {} failed: {e:#}",
                    comp.name
                );
                eprintln!(
                    "restore --from cloud: rolling back {} component(s)",
                    placed.len()
                );
                for p in placed.iter().rev() {
                    if let Err(e) = p.rollback() {
                        eprintln!(
                            "restore --from cloud: ROLLBACK FAILED for {}: {e}",
                            p.dest.display()
                        );
                    }
                }
                if systemd::available() {
                    let _ = systemd::run("start", &unit);
                }
                std::fs::remove_dir_all(&staging).ok();
                return Ok(1);
            }
        }
    }

    if systemd::available() {
        let _ = systemd::run("start", &unit);
    }
    eprintln!(
        "restored: cloud snapshot {} (pre-restore backups retained as *.pre-restore-{} beside each path)",
        opts.id, ts
    );
    std::fs::remove_dir_all(&staging).ok();
    Ok(0)
}

fn utc_iso_filename_safe() -> String {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    format!("{secs}")
}

/// Helper: enable callers to point rclone at any path.
#[allow(dead_code)]
fn rclone_config_path() -> &'static Path {
    Path::new("/etc/deputyos/rclone.conf")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_lsd_output_basic() {
        let raw = "          -1 2026-04-25 03:00:01         5 host-20260425T030001Z\n          -1 2026-04-26 03:00:00         5 host-20260426T030000Z\n";
        let snaps = parse_lsd_output(raw);
        assert_eq!(snaps.len(), 2);
        assert_eq!(snaps[0].id, "host-20260425T030001Z");
        assert!(snaps[1]
            .iso_timestamp
            .as_ref()
            .expect("ts present")
            .contains("2026-04-26"));
    }

    /// `true` if the `age` binary is on PATH (the cloud backup/restore gate).
    fn age_present() -> bool {
        std::process::Command::new("age")
            .arg("-h")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .is_ok()
    }

    /// Build a minimal `Manifest` whose `data_dir` + `memory.session_db` point
    /// at the given paths (mirrors `backup::tests::min_manifest`, which is
    /// test-only and unreachable from this module).
    fn min_manifest(data_dir: String, session_db: Option<String>) -> crate::manifest::Manifest {
        use crate::manifest::{
            HealthSection, Manifest, MemorySection, PathsSection, ProfileSection, RuntimeSection,
            ServiceSection,
        };
        Manifest {
            profile: ProfileSection {
                id: "openclaw".into(),
                display_name: "OpenClaw".into(),
                upstream_repo: "openclaw/openclaw".into(),
                release_channel: "stable".into(),
                min_ram_mb: 512,
                pinned_version: "1.0.0".into(),
            },
            paths: PathsSection {
                install_root: "/opt/deputyos/profiles/openclaw".into(),
                data_dir,
                binary: "/opt/deputyos/profiles/openclaw/bin/openclaw".into(),
            },
            runtime: RuntimeSection {
                language: "node".into(),
                node_version: None,
                python_version: None,
                package_manager: "npm".into(),
                extra_apt: vec![],
            },
            service: ServiceSection {
                unit: "openclaw-gateway.service".into(),
                entrypoint: "/bin/true".into(),
                ports: vec![8088],
                restart_policy: "always".into(),
            },
            health: HealthSection {
                http_check: "http://localhost:8088/health".into(),
                journal_unit: "openclaw-gateway.service".into(),
                startup_grace_s: 10,
            },
            apparmor: None,
            kernel: None,
            wizard: None,
            channels: None,
            memory: Some(MemorySection {
                session_db,
                backup_strategy: None,
            }),
            upgrade: None,
            mounts: None,
            airgap: None,
            default_egress: None,
        }
    }

    /// `age` round-trip gate for M8 (unblocked once `age` is installed). This
    /// exercises the *in-sandbox* half of the M8 exit criterion: build a
    /// profile-state bundle on "device A", age-encrypt it (the mandatory cloud
    /// gate), age-decrypt it, extract, and place each component onto "device
    /// B" — then assert device B reproduces device A byte-for-byte. The full
    /// two-device cloud round-trip (real bucket + `deputyctl restore --from
    /// cloud` over HTTPS) stays out-of-sandbox; this proves the crypto + bundle
    /// + placement seam the cloud path rides on.
    #[test]
    fn age_round_trip_backup_to_restore_is_byte_identical() {
        if !age_present() {
            eprintln!("test: age not installed; skipping");
            return;
        }
        let _g = crate::env_mutex().lock().unwrap_or_else(|e| e.into_inner());
        let root = tempfile::tempdir().expect("tempdir");

        // ---- Device A: the profile state being backed up. ----
        let a_data = root.path().join("a-data");
        std::fs::create_dir_all(a_data.join("sub")).expect("mkdir a-data/sub");
        std::fs::write(a_data.join("file.txt"), b"data-content-A").expect("a data file");
        std::fs::write(a_data.join("sub").join("nested.txt"), b"nested-A").expect("a nested");
        let a_hooks = root.path().join("a-hooks");
        std::fs::create_dir_all(&a_hooks).expect("mkdir a-hooks");
        std::fs::write(a_hooks.join("hook.sh"), b"#!/bin/sh\necho A\n").expect("a hook");
        let a_secrets = root.path().join("a-secrets.env");
        std::fs::write(&a_secrets, b"OPENROUTER_KEY=sk-device-A\n").expect("a secrets");
        let a_session = root.path().join("a-sessions.db");
        std::fs::write(&a_session, b"session-blob-A").expect("a session db");

        // build_profile_bundle reads hooks via paths::hooks_dir() and secrets
        // via paths::secrets_file() — point both at device A's copies.
        std::env::set_var("DEPUTYOS_HOOKS_DIR", &a_hooks);
        std::env::set_var("DEPUTYOS_SECRETS_FILE", &a_secrets);

        let manifest = min_manifest(
            a_data.to_string_lossy().to_string(),
            Some(a_session.to_string_lossy().to_string()),
        );

        // 1. Stage the bundle (what `deputyctl backup --to cloud` does before tar).
        let staging = root.path().join("stage");
        let included =
            backup::build_profile_bundle(&staging, "openclaw", &manifest, "host-a", "123")
                .expect("build bundle");
        assert!(included.contains(&"data".to_string()));
        assert!(included.contains(&"hooks".to_string()));
        assert!(included.contains(&"secrets".to_string()));
        assert!(included.contains(&"session.db".to_string()));

        // 2. Tar the staged bundle (the cloud path tars before age-encrypting).
        let tar_a = root.path().join("bundle.tar.gz");
        let tar = std::process::Command::new("tar")
            .args([
                "-czf",
                &tar_a.to_string_lossy(),
                "-C",
                &staging.to_string_lossy(),
                ".",
            ])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .expect("tar create");
        assert!(tar.success(), "tar create succeeded");

        // 3. age-encrypt with the backup token as the passphrase (mandatory).
        let enc = root.path().join("bundle.age");
        let passphrase = "the-backup-token-is-the-passphrase";
        backup::encrypt_with_age(&tar_a, &enc, passphrase).expect("age encrypt");
        // The cloud path removes the unencrypted tar intermediate.
        std::fs::remove_file(&tar_a).ok();

        // 4. age-decrypt (what `deputyctl restore --from cloud` does after download).
        let tar_b = root.path().join("bundle.decrypted.tar.gz");
        decrypt_with_age(&enc, &tar_b, passphrase).expect("age decrypt round-trip");
        assert!(tar_b.is_file(), "decrypted tar exists");

        // 5. A wrong passphrase must fail the gate (no silent accept).
        let bad = root.path().join("bundle.bad.tar.gz");
        assert!(decrypt_with_age(&enc, &bad, "wrong-passphrase").is_err());

        // 6. Extract the decrypted bundle.
        let extract = root.path().join("extract");
        std::fs::create_dir_all(&extract).expect("mkdir extract");
        let x = std::process::Command::new("tar")
            .args([
                "-xzf",
                &tar_b.to_string_lossy(),
                "-C",
                &extract.to_string_lossy(),
            ])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .expect("tar extract");
        assert!(x.success(), "tar extract succeeded");

        // The extracted bundle must be byte-identical to what was staged.
        assert_eq!(
            std::fs::read(extract.join("data").join("file.txt")).expect("read"),
            b"data-content-A".to_vec()
        );
        assert_eq!(
            std::fs::read(extract.join("data").join("sub").join("nested.txt")).expect("read"),
            b"nested-A".to_vec()
        );
        assert_eq!(
            std::fs::read(extract.join("hooks").join("hook.sh")).expect("read"),
            b"#!/bin/sh\necho A\n".to_vec()
        );
        assert_eq!(
            std::fs::read(extract.join("secrets.env")).expect("read"),
            b"OPENROUTER_KEY=sk-device-A\n".to_vec()
        );
        assert_eq!(
            std::fs::read(extract.join("session.db")).expect("read"),
            b"session-blob-A".to_vec()
        );
        let bundle_json: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(extract.join(backup::BUNDLE_MANIFEST_NAME))
                .expect("read BUNDLE.json"),
        )
        .expect("parse BUNDLE.json");
        assert_eq!(bundle_json["schema_version"], backup::BUNDLE_SCHEMA_VERSION);
        assert_eq!(bundle_json["profile_id"], "openclaw");
        assert_eq!(bundle_json["host"], "host-a");

        // ---- Device B: a fresh image with empty canonical paths. ----
        // Re-point the env-overridden canonical paths at device B so the
        // placement step writes there (simulating restore onto a fresh image).
        let b_hooks = root.path().join("b-hooks");
        let b_secrets = root.path().join("b-secrets.env");
        let b_data = root.path().join("b-data");
        let b_session = root.path().join("b-sessions.db");
        std::env::set_var("DEPUTYOS_HOOKS_DIR", &b_hooks);
        std::env::set_var("DEPUTYOS_SECRETS_FILE", &b_secrets);
        // The data-dir + session.db dests come from the manifest / the arg we
        // pass to build_component_plan, so point a *device-B manifest* at them.
        let b_manifest = min_manifest(
            b_data.to_string_lossy().to_string(),
            Some(b_session.to_string_lossy().to_string()),
        );

        // 7. Build the restore plan + place each component (the placement seam
        //    `run_from_cloud` uses after extract — minus the systemd stop/start,
        //    which is a no-op without systemd in the sandbox).
        let plan = build_component_plan(&extract, &b_manifest, &b_data);
        let ts = "123";
        let mut placed: Vec<Placed> = Vec::new();
        for comp in &plan {
            if !comp.src.exists() {
                continue;
            }
            placed.push(
                move_aside_and_place(&comp.src, &comp.dest, comp.is_file, ts)
                    .expect("place component"),
            );
        }

        // 8. Device B now reproduces device A byte-for-byte.
        assert_eq!(
            std::fs::read(b_data.join("file.txt")).expect("read"),
            b"data-content-A".to_vec()
        );
        assert_eq!(
            std::fs::read(b_data.join("sub").join("nested.txt")).expect("read"),
            b"nested-A".to_vec()
        );
        assert_eq!(
            std::fs::read(b_hooks.join("hook.sh")).expect("read"),
            b"#!/bin/sh\necho A\n".to_vec()
        );
        assert_eq!(
            std::fs::read(&b_secrets).expect("read"),
            b"OPENROUTER_KEY=sk-device-A\n".to_vec()
        );
        assert_eq!(
            std::fs::read(&b_session).expect("read"),
            b"session-blob-A".to_vec()
        );

        // 9. Rollback reverses placement: remove what we placed, restore the
        //    aside (none here, device B was empty) — dests must be gone again.
        for p in placed.iter().rev() {
            p.rollback().expect("rollback");
        }
        assert!(!b_data.join("file.txt").exists(), "rollback cleared data");
        assert!(!b_secrets.exists(), "rollback cleared secrets");

        std::env::remove_var("DEPUTYOS_HOOKS_DIR");
        std::env::remove_var("DEPUTYOS_SECRETS_FILE");
    }

    #[test]
    fn run_list_clean_error_when_no_config() {
        let _g = crate::env_mutex().lock().unwrap_or_else(|e| e.into_inner());
        let dir = tempfile::tempdir().expect("tempdir");
        std::env::set_var("DEPUTYOS_BACKUP_CONFIG", dir.path().join("nope.toml"));
        let code = run_list(ListOpts { json: false }).expect("run");
        // Either rclone is not installed (1) or the config error pre-empts it
        // (also 1). Either way, no panic.
        assert_eq!(code, 1);
        std::env::remove_var("DEPUTYOS_BACKUP_CONFIG");
    }

    #[test]
    fn snapshot_empty_id_is_ex_usage() {
        let code = run_snapshot(SnapshotOpts {
            id: "".into(),
            yes: true,
        })
        .expect("run");
        assert_eq!(code, 64);
    }

    #[test]
    fn valid_snapshot_id_accepts_and_rejects() {
        // accepted: alphanumeric + - _ ., up to 64 chars
        assert!(valid_snapshot_id("host-20260426T030000Z"));
        assert!(valid_snapshot_id("snap_001.data"));
        assert!(valid_snapshot_id("a"));
        let max = "a".repeat(64);
        assert!(valid_snapshot_id(&max));

        // rejected: empty, too long, path traversal, spaces, slashes
        assert!(!valid_snapshot_id(""));
        assert!(!valid_snapshot_id(&"a".repeat(65)));
        assert!(!valid_snapshot_id("../etc"));
        assert!(!valid_snapshot_id("has space"));
        assert!(!valid_snapshot_id("a/b"));
        assert!(!valid_snapshot_id("a;b"));
    }
}
