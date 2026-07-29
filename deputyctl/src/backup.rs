//! `deputyctl backup now|schedule` — rclone-driven backups.
//!
//! Per `docs/06-storage-and-backup.md`, deputyOS uses rclone as the data plane
//! and a small TOML config (`/etc/deputyos/backup.toml`) as the control plane.
//! `backup now` runs `rclone sync <data_dir> <remote>:<bucket>/<host>/<ts>/`;
//! `backup schedule` writes a systemd timer + service that re-invoke
//! `deputyctl backup now`.
//!
//! In dev mode (`DEPUTYOS_DEV_OUT=<dir>`) the timer/service files are written
//! under `<dev-out>/systemd/` and `systemctl daemon-reload` is skipped, so a
//! contributor can exercise the wiring without root.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use anyhow::{anyhow, bail, Context, Result};
use serde::{Deserialize, Serialize};

use crate::paths;
use crate::profile;

#[derive(Debug, Clone, Default)]
pub struct NowOpts {
    pub dry_run: bool,
    /// If set, upload an age-encrypted bundle to api.deputyos.com instead of
    /// the rclone-configured destination.
    pub to_cloud: bool,
}

#[derive(Debug, Clone, Default)]
pub struct ScheduleOpts {
    pub every: Option<String>,
    pub at: Option<String>,
    pub to_cloud: bool,
    pub list: bool,
    pub disable: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BackupConfig {
    /// `<remote>:<bucket-or-path>` — what rclone calls the destination.
    pub remote: String,
    #[serde(default)]
    pub retention_days: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct BackupConfigFile {
    backup: BackupConfig,
}

#[derive(Debug, Clone, Serialize)]
struct BackupStatus {
    schema_version: u32,
    mode: String,
    success: bool,
    completed_at: String,
    error: Option<String>,
}

/// Schema version for the profile-state backup bundle (M8 Lane F).
///
/// Bundle layout inside the age-encrypted tar root:
///   `BUNDLE.json` — `{ schema_version, profile_id, host, created_at, included }`
///   `data/`       — contents of the active profile's `data_dir`
///   `hooks/`      — contents of `paths::hooks_dir()` (if present)
///   `secrets.env` — copy of `paths::secrets_file()` (if present) — SENSITIVE
///   `session.db`  — copy of `manifest.memory.session_db` (if outside data_dir)
///
/// `deputyctl restore --from cloud` reverses each component into its canonical
/// path. See `documentation/docs/concepts/threat-model-accounts.md` for the
/// security properties (age mandatory; server ciphertext-only).
pub const BUNDLE_SCHEMA_VERSION: u32 = 3;
pub const BUNDLE_MANIFEST_NAME: &str = "BUNDLE.json";

#[derive(Debug, Clone, Serialize)]
struct BundleManifest {
    schema_version: u32,
    snapshot_id: String,
    profile_id: String,
    device_id: Option<String>,
    host: String,
    created_at: String,
    key_id: String,
    consistency: String,
    included: Vec<String>,
}

/// Load `/etc/deputyos/backup.toml` (or the env-overridden path).
pub fn load_config() -> Result<BackupConfig> {
    load_config_from(&paths::backup_config_file())
}

pub fn load_config_from(path: &Path) -> Result<BackupConfig> {
    let raw = std::fs::read_to_string(path)
        .with_context(|| format!("reading backup config {}", path.display()))?;
    let parsed: BackupConfigFile = toml::from_str(&raw)
        .with_context(|| format!("parsing backup config {}", path.display()))?;
    if parsed.backup.remote.trim().is_empty() {
        bail!(
            "backup config at {} has empty [backup].remote",
            path.display()
        );
    }
    Ok(parsed.backup)
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

/// `<hostname>-<utc-iso>` (filename-safe).
pub fn snapshot_id(now: chrono_like::Iso) -> String {
    let host = hostname_string();
    format!("{host}-{}", now.0)
}

/// Run `deputyctl backup now`.
pub fn run_now(opts: NowOpts) -> Result<u8> {
    if opts.to_cloud {
        let result = run_cloud_backup(opts.dry_run);
        match &result {
            Ok(code) => write_backup_status(
                *code == 0,
                if *code == 0 {
                    None
                } else {
                    Some("managed backup failed".to_string())
                },
            ),
            Err(error) => write_backup_status(false, Some(format!("{error:#}"))),
        }
        return result;
    }
    if !rclone_available() {
        eprintln!("backup now: rclone not installed — run `make doctor`");
        return Ok(1);
    }
    let cfg = match load_config() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("backup now: {e:#}");
            return Ok(1);
        }
    };
    let (_id, manifest) = profile::load_active().context("loading active profile")?;
    let data_dir = manifest.paths.data_dir.clone();
    let host = hostname_string();
    let ts = utc_iso_filename_safe();
    let snapshot = format!("{host}-{ts}");
    let dest = format!("{}/{host}/{ts}/", cfg.remote.trim_end_matches('/'));

    let mut cmd = Command::new("rclone");
    cmd.arg("sync")
        .arg(&data_dir)
        .arg(&dest)
        .arg("--config")
        .arg(paths::rclone_config_file());
    if opts.dry_run {
        cmd.arg("--dry-run");
    }
    cmd.stdout(Stdio::inherit()).stderr(Stdio::inherit());

    eprintln!(
        "backup now: rclone sync {} {} (config {})",
        data_dir,
        dest,
        paths::rclone_config_file().display()
    );

    let status = cmd
        .status()
        .with_context(|| format!("spawning rclone sync {data_dir} {dest}"))?;
    if !status.success() {
        eprintln!("backup now: rclone exited {status}");
        return Ok(1);
    }

    println!("snapshot:   {snapshot}");
    println!("dest:       {dest}");
    if let Ok(md) = std::fs::metadata(&data_dir) {
        if md.is_dir() {
            // Best-effort size — recursive walk isn't a primary guarantee.
            println!("source:     {data_dir}");
        }
    }
    Ok(0)
}

fn write_backup_status(success: bool, error: Option<String>) {
    let path = paths::backup_status_file();
    let status = BackupStatus {
        schema_version: 1,
        mode: "managed".to_string(),
        success,
        completed_at: utc_iso_filename_safe(),
        error,
    };
    let Ok(body) = serde_json::to_vec_pretty(&status) else {
        return;
    };
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let temp = path.with_extension("tmp");
    if std::fs::write(&temp, body).is_ok() {
        let _ = std::fs::rename(temp, path);
    }
}

/// Backup to the deputyOS cloud API — client-side age encryption, server sees
/// opaque ciphertext only. The backup token authenticates the upload; a
/// separate stable recovery secret encrypts it so token rotation/revocation
/// cannot destroy decryptability.
fn run_cloud_backup(dry_run: bool) -> Result<u8> {
    let token_path = paths::cloud_backup_token_file();
    if !token_path.is_file() {
        eprintln!(
            "backup --to cloud: no backup token at {}",
            token_path.display()
        );
        eprintln!("  create an account and register this device via the wizard, or");
        eprintln!("  place a token at {} (mode 0600)", token_path.display());
        return Ok(1);
    }
    let token = std::fs::read_to_string(&token_path)
        .with_context(|| format!("reading {}", token_path.display()))?
        .trim()
        .to_string();
    if token.is_empty() {
        eprintln!("backup --to cloud: backup token is empty");
        return Ok(1);
    }
    let recovery_secret = match crate::recovery_key::load() {
        Ok(secret) => secret,
        Err(error) => {
            eprintln!("backup --to cloud: {error:#}");
            eprintln!("  initialize and export it first:");
            eprintln!("  deputyctl backup recovery-key init");
            return Ok(1);
        }
    };
    let key_id = crate::recovery_key::key_id(&recovery_secret);

    let (profile_id, manifest) = profile::load_active().context("loading active profile")?;
    let host = hostname_string();
    let ts = utc_iso_filename_safe();
    let device_id = device_id();
    let snapshot_id = managed_snapshot_id(&host, &profile_id, device_id.as_deref(), &ts);

    eprintln!("backup --to cloud: building profile-state bundle for profile {profile_id}");
    if dry_run {
        eprintln!(
            "backup --to cloud: [dry-run] would quiesce workloads and upload {snapshot_id} using {key_id}"
        );
        return Ok(0);
    }

    // 1. Quiesce the active workload, sync its filesystem state, then copy the
    //    snapshot inputs. The guard always thaws the workload on failure.
    let staging = Path::new("/var/tmp").join(format!("{snapshot_id}.stage"));
    if staging.exists() {
        std::fs::remove_dir_all(&staging).ok();
    }
    let mut snapshot_guard = SnapshotGuard::prepare(&snapshot_id)?;
    let included = build_profile_bundle_with_context(
        &staging,
        &profile_id,
        &manifest,
        &host,
        &ts,
        &snapshot_id,
        device_id.as_deref(),
        &key_id,
        "quiesced",
    );
    let resume_result = snapshot_guard.complete();
    let included = match included {
        Ok(included) => included,
        Err(error) => {
            std::fs::remove_dir_all(&staging).ok();
            return Err(error);
        }
    };
    if let Err(error) = resume_result {
        std::fs::remove_dir_all(&staging).ok();
        return Err(error);
    }

    // 2. Tar the staged bundle.
    let tar_path = Path::new("/var/tmp").join(format!("{snapshot_id}.tar.gz"));
    let tar = Command::new("tar")
        .args([
            "-czf",
            &tar_path.to_string_lossy(),
            "-C",
            &staging.to_string_lossy(),
            ".",
        ])
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .status();
    std::fs::remove_dir_all(&staging).ok();
    let tar = tar.context("creating tar archive of profile-state bundle")?;
    if !tar.success() {
        eprintln!("backup --to cloud: tar failed");
        std::fs::remove_file(&tar_path).ok();
        return Ok(1);
    }

    // 3. age-encrypt (mandatory — the cloud server is ciphertext-only by design).
    let encrypted = Path::new("/var/tmp").join(format!("{snapshot_id}.age"));
    let encryption = encrypt_with_age(&tar_path, &encrypted, &recovery_secret);
    std::fs::remove_file(&tar_path).ok(); // remove unencrypted intermediate
    encryption?;

    // Locally decrypt and inspect the archive before committing it remotely.
    if let Err(error) = verify_encrypted_bundle(&encrypted, &recovery_secret, &snapshot_id) {
        std::fs::remove_file(&encrypted).ok();
        return Err(error);
    }

    // 4. Upload. The bearer is the backup token (the API authenticates via
    //    backup_token_hash). DEPUTYOS_API_BASE overrides the hostname for E2E.
    let api_base = std::env::var("DEPUTYOS_API_BASE")
        .unwrap_or_else(|_| "https://api.deputyos.com".to_string());
    let url = format!(
        "{}/api/v1/backup/{snapshot_id}",
        api_base.trim_end_matches('/')
    );
    eprintln!(
        "backup --to cloud: uploading {snapshot_id} ({:.1} MB, included: {})",
        std::fs::metadata(&encrypted)
            .map(|m| m.len() as f64 / 1_000_000.0)
            .unwrap_or(0.0),
        included.join(", ")
    );

    let resp = ureq::put(&url)
        .set("Authorization", &format!("Bearer {token}"))
        .set("Content-Type", "application/octet-stream")
        .set("X-DeputyOS-Profile", &profile_id)
        .set("X-DeputyOS-Key-Id", &key_id)
        .set(
            "X-DeputyOS-Bundle-Schema",
            &BUNDLE_SCHEMA_VERSION.to_string(),
        )
        .send(std::fs::File::open(&encrypted).context("reading encrypted bundle")?);

    match resp {
        Ok(r) if r.status() == 200 || r.status() == 201 => {
            println!("snapshot:   {snapshot_id}");
            println!(
                "dest:       {api_base} (age-encrypted with {key_id}; bundle: {})",
                included.join(", ")
            );
            let _ = ureq::post(&url)
                .set("Authorization", &format!("Bearer {token}"))
                .call();
            std::fs::remove_file(&encrypted).ok();
            Ok(0)
        }
        Ok(r) => {
            let status = r.status();
            let body = r.into_string().unwrap_or_default();
            eprintln!("backup --to cloud: upload failed ({status}): {body}");
            std::fs::remove_file(&encrypted).ok();
            Ok(1)
        }
        Err(e) => {
            eprintln!("backup --to cloud: upload error: {e}");
            std::fs::remove_file(&encrypted).ok();
            Ok(1)
        }
    }
}

struct SnapshotGuard {
    request_id: String,
    active: bool,
}

impl SnapshotGuard {
    fn prepare(snapshot_id: &str) -> Result<Self> {
        let request_id = format!("backup-{snapshot_id}");
        if !Path::new(deputyd::DEFAULT_SOCKET).exists() {
            // Community images intentionally have no resident overlay. The
            // backup remains available, but without managed workload quiesce.
            return Ok(Self {
                request_id,
                active: false,
            });
        }
        send_snapshot_command(&request_id, deputyd::SnapshotAction::Prepare)
            .context("quiescing workload for a consistent backup")?;
        Ok(Self {
            request_id,
            active: true,
        })
    }

    fn complete(&mut self) -> Result<()> {
        if !self.active {
            return Ok(());
        }
        send_snapshot_command(&self.request_id, deputyd::SnapshotAction::Complete)
            .context("resuming workload after backup snapshot")?;
        self.active = false;
        Ok(())
    }
}

impl Drop for SnapshotGuard {
    fn drop(&mut self) {
        if self.active {
            let _ = send_snapshot_command(&self.request_id, deputyd::SnapshotAction::Complete);
        }
    }
}

fn send_snapshot_command(request_id: &str, action: deputyd::SnapshotAction) -> Result<()> {
    let request =
        deputyd::AgentRequest::new(request_id, deputyd::AgentCommand::Snapshot { action });
    let response = deputyd::request(Path::new(deputyd::DEFAULT_SOCKET), &request)?;
    deputyd::ensure_success(response)?;
    Ok(())
}

fn verify_encrypted_bundle(encrypted: &Path, secret: &str, snapshot_id: &str) -> Result<()> {
    let verify_tar = Path::new("/var/tmp").join(format!("{snapshot_id}.verify.tar.gz"));
    if let Err(error) = crate::restore::decrypt_with_age(encrypted, &verify_tar, secret) {
        std::fs::remove_file(&verify_tar).ok();
        return Err(error).context("verifying encrypted backup can be decrypted");
    }
    let output = Command::new("tar")
        .args(["-tzf", &verify_tar.to_string_lossy()])
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .output()
        .context("verifying backup tar archive")?;
    std::fs::remove_file(&verify_tar).ok();
    if !output.status.success() {
        bail!(
            "backup verification failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(())
}

fn managed_snapshot_id(
    host: &str,
    profile_id: &str,
    device_id: Option<&str>,
    timestamp: &str,
) -> String {
    fn label(value: &str, max: usize) -> String {
        value
            .bytes()
            .filter(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
            .take(max)
            .map(char::from)
            .collect()
    }
    format!(
        "{}-{}-{}-{}",
        label(host, 14),
        label(profile_id, 12),
        label(device_id.unwrap_or("unregistered"), 8),
        label(timestamp, 24)
    )
}

fn device_id() -> Option<String> {
    let value: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(paths::account_file()).ok()?).ok()?;
    value
        .get("device_id")
        .and_then(serde_json::Value::as_str)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

/// Encrypt `path` to `out_path` with age using a stable recovery secret.
///
/// MANDATORY for cloud backups — there is no unencrypted fallback, because the
/// cloud server is ciphertext-only by design (see
/// `documentation/docs/concepts/threat-model-accounts.md`). `agekey::derive`
/// turns the recovery secret into an age identity + recipient. We encrypt to
/// the recipient (public key) — no tty, no stdin
/// piping, so this runs unattended under the systemd timer. (age's `-p`
/// passphrase mode prompts on `/dev/tty` and cannot run unattended; see
/// `agekey.rs`.) Returns an error if `age` is missing or exits non-zero; the
/// caller surfaces a clean install hint.
pub(crate) fn encrypt_with_age(path: &Path, out_path: &Path, secret: &str) -> Result<()> {
    let (_identity, recipient) = crate::agekey::derive(secret)?;
    let output = Command::new("age")
        .arg("-r")
        .arg(&recipient)
        .arg("-o")
        .arg(out_path)
        .arg(path)
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .output()
        .context("spawning age — install it (e.g. `sudo apt install age`) for cloud backup")?;
    if !output.status.success() {
        bail!(
            "age encryption failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(())
}

/// Build the staged profile-state bundle directory. Returns the list of
/// component names actually included (for `BUNDLE.json` + logging).
///
/// Bundle layout inside the age-encrypted tar root:
///   `BUNDLE.json` — `{ schema_version, profile_id, host, created_at, included }`
///   `data/`       — contents of the active profile's `data_dir`
///   `hooks/`      — contents of `paths::hooks_dir()` (if present)
///   `secrets.env` — copy of `paths::secrets_file()` (if present) — SENSITIVE
///   `session.db`  — copy of `manifest.memory.session_db` (if outside data_dir)
#[cfg(test)]
pub(crate) fn build_profile_bundle(
    staging: &Path,
    profile_id: &str,
    manifest: &crate::manifest::Manifest,
    host: &str,
    created_at: &str,
) -> Result<Vec<String>> {
    build_profile_bundle_with_context(
        staging,
        profile_id,
        manifest,
        host,
        created_at,
        &format!("{host}-{created_at}"),
        None,
        "legacy-derived-key",
        "uncoordinated",
    )
}

#[allow(clippy::too_many_arguments)]
fn build_profile_bundle_with_context(
    staging: &Path,
    profile_id: &str,
    manifest: &crate::manifest::Manifest,
    host: &str,
    created_at: &str,
    snapshot_id: &str,
    device_id: Option<&str>,
    key_id: &str,
    consistency: &str,
) -> Result<Vec<String>> {
    std::fs::create_dir_all(staging)
        .with_context(|| format!("creating staging dir {}", staging.display()))?;

    let mut included: Vec<String> = Vec::new();
    let data_dir = Path::new(&manifest.paths.data_dir);

    // data/ — the profile's data partition.
    if data_dir.is_dir() {
        let dst = staging.join("data");
        std::fs::create_dir_all(&dst)?;
        copy_dir_recursive(data_dir, &dst)?;
        included.push("data".into());
    }

    // hooks/ — user-customized hook scripts (live under /etc/deputyos, not data_dir).
    let hooks_dir = paths::hooks_dir();
    if hooks_dir.is_dir() {
        let dst = staging.join("hooks");
        std::fs::create_dir_all(&dst)?;
        copy_dir_recursive(&hooks_dir, &dst)?;
        included.push("hooks".into());
    }

    // secrets.env — provider credentials (SENSITIVE; makes age mandatory).
    let secrets_file = paths::secrets_file();
    if secrets_file.is_file() {
        std::fs::copy(&secrets_file, staging.join("secrets.env"))?;
        included.push("secrets".into());
    }

    // session.db — the channel/memory DB, only if it lives outside data_dir
    // (otherwise it is already captured under data/, and duplicating it would
    // risk a partial overwrite on restore).
    if let Some(session_db) = manifest
        .memory
        .as_ref()
        .and_then(|m| m.session_db.as_deref())
    {
        let session_path = Path::new(session_db);
        if session_path.is_file() && !is_within(data_dir, session_path) {
            std::fs::copy(session_path, staging.join("session.db"))?;
            included.push("session.db".into());
        }
    }

    // BUNDLE.json — manifest of the bundle itself.
    let bundle_manifest = BundleManifest {
        schema_version: BUNDLE_SCHEMA_VERSION,
        snapshot_id: snapshot_id.to_string(),
        profile_id: profile_id.to_string(),
        device_id: device_id.map(str::to_string),
        host: host.to_string(),
        created_at: created_at.to_string(),
        key_id: key_id.to_string(),
        consistency: consistency.to_string(),
        included: included.clone(),
    };
    std::fs::write(
        staging.join(BUNDLE_MANIFEST_NAME),
        serde_json::to_string_pretty(&bundle_manifest)?,
    )?;
    included.push("BUNDLE.json".into());

    Ok(included)
}

/// Recursively copy a directory tree. Symlinks and special files are skipped
/// (the bundle is restored by path, not by inode).
pub fn copy_dir_recursive(src: &Path, dst: &Path) -> Result<()> {
    for entry in std::fs::read_dir(src).with_context(|| format!("reading {}", src.display()))? {
        let entry = entry?;
        let target = dst.join(entry.file_name());
        match entry.file_type()? {
            ft if ft.is_dir() => {
                std::fs::create_dir_all(&target)?;
                copy_dir_recursive(&entry.path(), &target)?;
            }
            ft if ft.is_file() => {
                std::fs::copy(entry.path(), &target)?;
            }
            _ => { /* symlinks / special files: skip */ }
        }
    }
    Ok(())
}

/// True if `child` is `parent` or lives beneath it. Canonicalizes when
/// possible, falls back to a lexical comparison otherwise.
fn is_within(parent: &Path, child: &Path) -> bool {
    if let (Ok(p), Ok(c)) = (parent.canonicalize(), child.canonicalize()) {
        return c.starts_with(&p);
    }
    child.starts_with(parent)
}

/// Run `deputyctl backup schedule`.
pub fn run_schedule(opts: ScheduleOpts) -> Result<u8> {
    if opts.list {
        return list_schedule();
    }
    if opts.disable {
        return disable_schedule();
    }

    // Default: --every 6h.
    let on_calendar = match (&opts.every, &opts.at) {
        (Some(_), Some(_)) => {
            bail!("--every and --at are mutually exclusive");
        }
        (Some(every), None) => parse_every(every)?,
        (None, Some(at)) => parse_at(at)?,
        (None, None) => "*-*-* 00/6:00:00".to_string(),
    };

    let unit_dir = effective_unit_dir();
    std::fs::create_dir_all(&unit_dir)
        .with_context(|| format!("creating {}", unit_dir.display()))?;

    let timer_path = unit_dir.join("deputyos-backup.timer");
    let service_path = unit_dir.join("deputyos-backup.service");

    let deputyctl = current_deputyctl_path();
    let destination = if opts.to_cloud { " --to-cloud" } else { "" };
    let service_body = format!(
        "[Unit]\nDescription=deputyOS backup snapshot\nAfter=deputyd.service network-online.target\nWants=network-online.target\n\n[Service]\nType=oneshot\nExecStart={deputyctl} backup now{destination}\n"
    );
    let timer_body = format!(
        "[Unit]\nDescription=deputyOS backup timer\n\n[Timer]\nOnCalendar={on_calendar}\nPersistent=true\nUnit=deputyos-backup.service\n\n[Install]\nWantedBy=timers.target\n"
    );
    std::fs::write(&service_path, service_body)
        .with_context(|| format!("writing {}", service_path.display()))?;
    std::fs::write(&timer_path, timer_body)
        .with_context(|| format!("writing {}", timer_path.display()))?;

    eprintln!("backup schedule: wrote {}", timer_path.display());
    eprintln!("backup schedule: wrote {}", service_path.display());

    if paths::dev_out_dir().is_some() {
        eprintln!("backup schedule: dev mode — skipping systemctl daemon-reload + enable");
    } else {
        run_systemctl(&["daemon-reload"]).ok();
        run_systemctl(&["enable", "--now", "deputyos-backup.timer"]).ok();
    }

    println!("scheduled: OnCalendar={on_calendar}");
    Ok(0)
}

fn list_schedule() -> Result<u8> {
    let unit_dir = effective_unit_dir();
    let timer_path = unit_dir.join("deputyos-backup.timer");
    if !timer_path.is_file() {
        println!("backup schedule: not configured");
        return Ok(0);
    }
    let raw = std::fs::read_to_string(&timer_path)
        .with_context(|| format!("reading {}", timer_path.display()))?;
    for line in raw.lines() {
        if let Some(rest) = line.trim().strip_prefix("OnCalendar=") {
            println!("OnCalendar: {rest}");
        }
    }
    Ok(0)
}

fn disable_schedule() -> Result<u8> {
    let unit_dir = effective_unit_dir();
    let timer_path = unit_dir.join("deputyos-backup.timer");
    let service_path = unit_dir.join("deputyos-backup.service");

    if paths::dev_out_dir().is_none() {
        run_systemctl(&["disable", "--now", "deputyos-backup.timer"]).ok();
    }
    for p in [&timer_path, &service_path] {
        if p.is_file() {
            std::fs::remove_file(p).with_context(|| format!("removing {}", p.display()))?;
            eprintln!("backup schedule: removed {}", p.display());
        }
    }
    println!("disabled");
    Ok(0)
}

fn run_systemctl(args: &[&str]) -> Result<()> {
    let status = Command::new("systemctl").args(args).status();
    match status {
        Ok(s) if s.success() => Ok(()),
        Ok(s) => Err(anyhow!("systemctl {:?} exited {s}", args)),
        Err(e) => Err(anyhow!("spawning systemctl: {e}")),
    }
}

fn effective_unit_dir() -> PathBuf {
    if let Some(dev) = paths::dev_out_dir() {
        return dev.join("systemd");
    }
    paths::systemd_unit_dir()
}

fn current_deputyctl_path() -> String {
    std::env::current_exe()
        .ok()
        .and_then(|p| p.canonicalize().ok())
        .map(|p| p.display().to_string())
        .unwrap_or_else(|| "/usr/local/bin/deputyctl".to_string())
}

fn parse_every(every: &str) -> Result<String> {
    // Accept simple shapes: `6h`, `30m`, `1d`. Map to OnCalendar.
    let trimmed = every.trim();
    let (num_str, unit) = trimmed.split_at(
        trimmed
            .find(|c: char| c.is_alphabetic())
            .unwrap_or(trimmed.len()),
    );
    let n: u32 = num_str
        .parse()
        .map_err(|_| anyhow!("--every: expected '<n><unit>', got {trimmed}"))?;
    let cal = match unit {
        "h" => format!("*-*-* 00/{n}:00:00"),
        "m" | "min" => format!("*:0/{n}:00"),
        "d" => "*-*-* 03:00:00".to_string(), // daily at 03:00 regardless of n
        other => bail!("--every: unsupported unit '{other}'"),
    };
    Ok(cal)
}

fn parse_at(at: &str) -> Result<String> {
    // "02:00" → "*-*-* 02:00:00"
    let parts: Vec<&str> = at.split(':').collect();
    if parts.len() != 2 {
        bail!("--at: expected HH:MM, got {at}");
    }
    let h: u32 = parts[0].parse().map_err(|_| anyhow!("--at: bad hour"))?;
    let m: u32 = parts[1].parse().map_err(|_| anyhow!("--at: bad minute"))?;
    Ok(format!("*-*-* {h:02}:{m:02}:00"))
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

fn utc_iso_filename_safe() -> String {
    // Filename-safe: ISO8601 basic, expanded to days/seconds-since-epoch.
    // We avoid chrono — snapshot ids only need monotonic + filename-safe.
    use std::time::{SystemTime, UNIX_EPOCH};
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    // Format: epoch-seconds with a `T` marker so the prefix sorts naturally.
    // E.g. `1714137600T-utc`.
    format!("{secs}T-utc")
}

/// Tiny RFC3339-ish iso-now wrapper used by the snapshot helper. Local-only;
/// callers that want a real chrono path should pass their own timestamp.
pub mod chrono_like {
    pub struct Iso(pub String);
    pub fn now_filename_safe() -> Iso {
        Iso(super::utc_iso_filename_safe())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_config_clean_error() {
        let _g = crate::env_mutex().lock().unwrap_or_else(|e| e.into_inner());
        let dir = tempfile::tempdir().expect("tempdir");
        std::env::set_var(
            "DEPUTYOS_BACKUP_CONFIG",
            dir.path().join("does-not-exist.toml"),
        );
        let res = load_config();
        assert!(res.is_err());
        std::env::remove_var("DEPUTYOS_BACKUP_CONFIG");
    }

    #[test]
    fn schedule_writes_units_in_dev_mode() {
        let _g = crate::env_mutex().lock().unwrap_or_else(|e| e.into_inner());
        let dir = tempfile::tempdir().expect("tempdir");
        std::env::set_var("DEPUTYOS_DEV_OUT", dir.path());
        let code = run_schedule(ScheduleOpts {
            every: Some("6h".into()),
            at: None,
            to_cloud: false,
            list: false,
            disable: false,
        })
        .expect("run");
        assert_eq!(code, 0);
        let timer = dir.path().join("systemd").join("deputyos-backup.timer");
        let service = dir.path().join("systemd").join("deputyos-backup.service");
        assert!(timer.is_file(), "timer not written");
        assert!(service.is_file(), "service not written");
        let body = std::fs::read_to_string(&timer).expect("read timer");
        assert!(body.contains("OnCalendar="));
        std::env::remove_var("DEPUTYOS_DEV_OUT");
    }

    #[test]
    fn managed_schedule_writes_cloud_service() {
        let _g = crate::env_mutex().lock().unwrap_or_else(|e| e.into_inner());
        let dir = tempfile::tempdir().expect("tempdir");
        std::env::set_var("DEPUTYOS_DEV_OUT", dir.path());
        run_schedule(ScheduleOpts {
            every: None,
            at: Some("03:00".into()),
            to_cloud: true,
            list: false,
            disable: false,
        })
        .expect("schedule");
        let service =
            std::fs::read_to_string(dir.path().join("systemd").join("deputyos-backup.service"))
                .expect("service");
        assert!(service.contains("backup now --to-cloud"));
        assert!(service.contains("After=deputyd.service network-online.target"));
        std::env::remove_var("DEPUTYOS_DEV_OUT");
    }

    #[test]
    fn schedule_disable_removes_units_in_dev_mode() {
        let _g = crate::env_mutex().lock().unwrap_or_else(|e| e.into_inner());
        let dir = tempfile::tempdir().expect("tempdir");
        std::env::set_var("DEPUTYOS_DEV_OUT", dir.path());
        run_schedule(ScheduleOpts {
            every: Some("6h".into()),
            at: None,
            to_cloud: false,
            list: false,
            disable: false,
        })
        .expect("schedule");
        let code = run_schedule(ScheduleOpts {
            every: None,
            at: None,
            to_cloud: false,
            list: false,
            disable: true,
        })
        .expect("disable");
        assert_eq!(code, 0);
        assert!(!dir
            .path()
            .join("systemd")
            .join("deputyos-backup.timer")
            .is_file());
        std::env::remove_var("DEPUTYOS_DEV_OUT");
    }

    #[test]
    fn parse_every_h() {
        let cal = parse_every("6h").expect("parse");
        assert!(cal.contains("00/6"));
    }

    #[test]
    fn parse_at_hhmm() {
        assert_eq!(parse_at("02:00").expect("parse"), "*-*-* 02:00:00");
    }

    #[test]
    fn snapshot_id_format() {
        let id = snapshot_id(chrono_like::Iso("test-stamp".into()));
        assert!(id.ends_with("-test-stamp"), "got {id}");
    }

    #[test]
    fn managed_snapshot_ids_are_bounded_and_device_scoped() {
        let first = managed_snapshot_id(
            "a-hostname-that-is-much-too-long",
            "openclaw",
            Some("11111111-aaaa-bbbb-cccc-dddddddddddd"),
            "1714137600T-utc",
        );
        let second = managed_snapshot_id(
            "a-hostname-that-is-much-too-long",
            "openclaw",
            Some("22222222-aaaa-bbbb-cccc-dddddddddddd"),
            "1714137600T-utc",
        );
        assert!(first.len() <= 64);
        assert_ne!(first, second);
    }

    use crate::manifest::{
        HealthSection, Manifest, MemorySection, PathsSection, ProfileSection, RuntimeSection,
        ServiceSection,
    };

    fn min_manifest(data_dir: String, memory: Option<MemorySection>) -> Manifest {
        Manifest {
            profile: ProfileSection {
                id: "testprof".into(),
                display_name: "Test".into(),
                upstream_repo: "test/repo".into(),
                release_channel: "stable".into(),
                min_ram_mb: 512,
                pinned_version: "1.0.0".into(),
            },
            paths: PathsSection {
                install_root: "/opt/deputyos/profiles/testprof".into(),
                data_dir,
                binary: "/opt/deputyos/profiles/testprof/bin/test".into(),
            },
            runtime: RuntimeSection {
                language: "python".into(),
                node_version: None,
                python_version: None,
                package_manager: "pip".into(),
                extra_apt: vec![],
            },
            service: ServiceSection {
                unit: "deputyos-test.service".into(),
                entrypoint: "/bin/true".into(),
                ports: vec![8088],
                restart_policy: "always".into(),
            },
            health: HealthSection {
                http_check: "http://localhost:8088/health".into(),
                journal_unit: "deputyos-test.service".into(),
                startup_grace_s: 10,
            },
            apparmor: None,
            kernel: None,
            wizard: None,
            channels: None,
            memory,
            upgrade: None,
            mounts: None,
            airgap: None,
            default_egress: None,
        }
    }

    #[test]
    fn build_profile_bundle_layout() {
        let _g = crate::env_mutex().lock().unwrap_or_else(|e| e.into_inner());
        let root = tempfile::tempdir().expect("tempdir");

        std::fs::create_dir_all(root.path().join("data")).expect("mkdir data");
        std::fs::write(root.path().join("data").join("file.txt"), "data-content")
            .expect("write data file");
        std::fs::create_dir_all(root.path().join("hooks")).expect("mkdir hooks");
        std::fs::write(root.path().join("hooks").join("hook.sh"), "#!/bin/sh\n")
            .expect("write hook");
        std::fs::write(root.path().join("secrets.env"), "OPENROUTER_KEY=sk-test\n")
            .expect("write secrets");

        std::env::set_var("DEPUTYOS_HOOKS_DIR", root.path().join("hooks"));
        std::env::set_var("DEPUTYOS_SECRETS_FILE", root.path().join("secrets.env"));

        let manifest = min_manifest(root.path().join("data").to_string_lossy().to_string(), None);
        let staging = root.path().join("stage");
        let included =
            build_profile_bundle(&staging, "testprof", &manifest, "host", "123").expect("build");

        assert_eq!(
            included,
            vec![
                "data".to_string(),
                "hooks".to_string(),
                "secrets".to_string(),
                "BUNDLE.json".to_string()
            ]
        );
        assert_eq!(
            std::fs::read_to_string(staging.join("data").join("file.txt")).expect("read data"),
            "data-content"
        );
        assert!(staging.join("hooks").join("hook.sh").is_file());
        assert_eq!(
            std::fs::read_to_string(staging.join("secrets.env")).expect("read secrets"),
            "OPENROUTER_KEY=sk-test\n"
        );
        let bundle: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(staging.join(BUNDLE_MANIFEST_NAME)).expect("read BUNDLE.json"),
        )
        .expect("parse BUNDLE.json");
        assert_eq!(bundle["schema_version"], BUNDLE_SCHEMA_VERSION);
        assert_eq!(bundle["profile_id"], "testprof");
        assert_eq!(bundle["host"], "host");

        std::env::remove_var("DEPUTYOS_HOOKS_DIR");
        std::env::remove_var("DEPUTYOS_SECRETS_FILE");
    }

    #[test]
    fn build_profile_bundle_includes_session_db_outside_data_dir() {
        let _g = crate::env_mutex().lock().unwrap_or_else(|e| e.into_inner());
        let root = tempfile::tempdir().expect("tempdir");

        std::fs::create_dir_all(root.path().join("data")).expect("mkdir data");
        std::fs::write(root.path().join("data").join("file.txt"), "x").expect("write data file");
        // A session DB living OUTSIDE data_dir must be bundled separately.
        std::fs::write(root.path().join("sessions.db"), "session-blob").expect("write session db");

        let manifest = min_manifest(
            root.path().join("data").to_string_lossy().to_string(),
            Some(MemorySection {
                session_db: Some(
                    root.path()
                        .join("sessions.db")
                        .to_string_lossy()
                        .to_string(),
                ),
                backup_strategy: None,
            }),
        );
        let staging = root.path().join("stage");
        let included =
            build_profile_bundle(&staging, "testprof", &manifest, "host", "123").expect("build");

        assert!(included.contains(&"session.db".to_string()));
        assert_eq!(
            std::fs::read_to_string(staging.join("session.db")).expect("read session.db"),
            "session-blob"
        );
    }

    #[test]
    fn is_within_canonical_and_lexical() {
        let dir = tempfile::tempdir().expect("tempdir");
        let parent = dir.path();
        let inside = parent.join("x");
        std::fs::create_dir_all(&inside).expect("mkdir inside");
        let outside = std::path::Path::new("/nope-not-real/x");

        assert!(is_within(parent, &inside));
        assert!(!is_within(parent, outside));
        // A path equal to the parent is "within" it.
        assert!(is_within(parent, parent));
    }
}
