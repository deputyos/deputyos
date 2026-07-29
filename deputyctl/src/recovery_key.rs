//! Stable, user-exportable recovery secret for backup encryption.
//!
//! API/device tokens are revocable credentials and are deliberately not used
//! as encryption keys. This secret survives credential rotation and can be
//! imported on another deputy to restore any bundle encrypted with it.

use std::io::Write;
use std::path::Path;

use anyhow::{bail, Context, Result};
use rand::RngCore;
use serde::Serialize;
use sha2::{Digest, Sha256};

use crate::paths;

const SECRET_BYTES: usize = 32;

#[derive(Debug, Clone, Serialize)]
pub struct RecoveryKeyInfo {
    pub key_id: String,
    pub path: String,
    pub created: bool,
}

pub fn load() -> Result<String> {
    let path = paths::backup_recovery_key_file();
    let secret = std::fs::read_to_string(&path)
        .with_context(|| format!("reading recovery key {}", path.display()))?
        .trim()
        .to_string();
    validate(&secret)?;
    Ok(secret)
}

pub fn initialize() -> Result<(RecoveryKeyInfo, Option<String>)> {
    let path = paths::backup_recovery_key_file();
    initialize_at(&path)
}

/// Initialize at an explicit root-relative path (used by the image wizard).
pub fn initialize_at(path: &Path) -> Result<(RecoveryKeyInfo, Option<String>)> {
    if path.is_file() {
        let secret = load_at(path)?;
        return Ok((info(path, &secret, false), None));
    }

    let mut bytes = [0u8; SECRET_BYTES];
    rand::rngs::OsRng.fill_bytes(&mut bytes);
    let secret = hex::encode(bytes);
    write_secret(path, &secret)?;
    Ok((info(path, &secret, true), Some(secret)))
}

fn load_at(path: &Path) -> Result<String> {
    let secret = std::fs::read_to_string(path)
        .with_context(|| format!("reading recovery key {}", path.display()))?
        .trim()
        .to_string();
    validate(&secret)?;
    Ok(secret)
}

pub fn import(secret: &str, replace: bool) -> Result<RecoveryKeyInfo> {
    let secret = secret.trim();
    validate(secret)?;
    let path = paths::backup_recovery_key_file();
    if path.exists() && !replace {
        bail!(
            "recovery key already exists at {}; use --replace only after exporting it",
            path.display()
        );
    }
    write_secret(&path, secret)?;
    Ok(info(&path, secret, true))
}

pub fn key_id(secret: &str) -> String {
    let digest = hex::encode(Sha256::digest(secret.as_bytes()));
    format!("rk-{}", &digest[..16])
}

fn validate(secret: &str) -> Result<()> {
    if secret.len() != SECRET_BYTES * 2 || !secret.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        bail!("recovery key must be exactly 64 hexadecimal characters");
    }
    Ok(())
}

fn info(path: &Path, secret: &str, created: bool) -> RecoveryKeyInfo {
    RecoveryKeyInfo {
        key_id: key_id(secret),
        path: path.display().to_string(),
        created,
    }
}

fn write_secret(path: &Path, secret: &str) -> Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| anyhow::anyhow!("recovery key path has no parent"))?;
    std::fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
    let temp = path.with_extension(format!("tmp-{}", std::process::id()));
    {
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temp)
            .with_context(|| format!("creating {}", temp.display()))?;
        file.write_all(secret.as_bytes())?;
        file.write_all(b"\n")?;
        file.sync_all()?;
    }
    set_private_mode(&temp)?;
    std::fs::rename(&temp, path)
        .with_context(|| format!("installing recovery key {}", path.display()))?;
    Ok(())
}

#[cfg(unix)]
fn set_private_mode(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
        .with_context(|| format!("setting mode 0600 on {}", path.display()))
}

#[cfg(not(unix))]
fn set_private_mode(_path: &Path) -> Result<()> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn initializes_once_and_import_requires_explicit_replace() {
        let _guard = crate::env_mutex().lock().unwrap_or_else(|e| e.into_inner());
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("recovery-key");
        std::env::set_var("DEPUTYOS_BACKUP_RECOVERY_KEY_FILE", &path);

        let (first, exported) = initialize().expect("initialize");
        assert!(first.created);
        assert_eq!(exported.as_deref().expect("new secret").len(), 64);
        let (second, exported_again) = initialize().expect("initialize again");
        assert!(!second.created);
        assert!(exported_again.is_none());
        assert_eq!(first.key_id, second.key_id);
        assert!(import(&"11".repeat(32), false).is_err());
        let replaced = import(&"11".repeat(32), true).expect("replace");
        assert_ne!(replaced.key_id, first.key_id);

        std::env::remove_var("DEPUTYOS_BACKUP_RECOVERY_KEY_FILE");
    }
}
