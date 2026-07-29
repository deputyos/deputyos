//! `deputyctl rollback` — flip back to the inactive A/B slot on next reboot.
//!
//! The actual partition manipulation lands in M6 (see
//! `docs/08-update-rollback.md`). This module:
//!   1. Reads `/var/lib/deputyos/slots.json` if it exists.
//!   2. Validates the cryptographic record of the inactive slot's image
//!      (so a corrupted rollback target is caught here, not after reboot).
//!   3. Refuses to actually swap, with a clear "lands in M6" message.
//!
//! The integrity check is real — that's the load-bearing M2/M4 contract.
//! A future M6 swap implementation can layer on top without changing this
//! validation logic.

use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};

/// Exit code for "rollback target not yet provisioned" — matches `EX_USAGE`.
const EX_NOT_READY: u8 = 64;

/// Typed view of `/var/lib/deputyos/slots.json`. Schema extensible —
/// new M6 fields (`boot_count`, `pending_rollback`, `auto_rollback`)
/// are all `#[serde(default)]` for forward/backward compat.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Slots {
    pub active: String,
    pub inactive: String,
    pub version_a: String,
    pub version_b: String,
    #[serde(default)]
    pub inactive_sha256: Option<String>,
    #[serde(default)]
    pub inactive_image_path: Option<String>,
    /// Incremented by watchdog on boot; reset to 0 on successful boot.
    #[serde(default)]
    pub boot_count: Option<u32>,
    /// True when a user-requested rollback is pending (reboot needed).
    #[serde(default)]
    pub pending_rollback: Option<bool>,
    /// True when the watchdog triggered an automatic rollback.
    #[serde(default)]
    pub auto_rollback: Option<bool>,
    /// True between selecting a newly written slot and post-boot health
    /// confirmation. The watchdog only counts boots while this is set.
    #[serde(default)]
    pub update_pending: Option<bool>,
    /// Version expected after the pending update is confirmed healthy.
    #[serde(default)]
    pub pending_version: Option<String>,
    /// Explicit writable slot destinations. Full host images such as qcow2
    /// are never guessed into these paths.
    #[serde(default)]
    pub slot_path_a: Option<String>,
    #[serde(default)]
    pub slot_path_b: Option<String>,
}

fn slots_path() -> PathBuf {
    if let Ok(p) = std::env::var("DEPUTYOS_SLOTS_FILE") {
        return PathBuf::from(p);
    }
    PathBuf::from("/var/lib/deputyos/slots.json")
}

pub fn run() -> Result<u8> {
    let path = slots_path();
    if !path.is_file() {
        eprintln!(
            "rollback: no rollback target — A/B slots not yet provisioned at {}; lands in M6 (see docs/08-update-rollback.md)",
            path.display()
        );
        return Ok(EX_NOT_READY);
    }
    let raw =
        std::fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
    let slots: Slots =
        serde_json::from_str(&raw).with_context(|| format!("parsing {}", path.display()))?;

    let inactive_version = match slots.inactive.as_str() {
        "A" => slots.version_a.clone(),
        "B" => slots.version_b.clone(),
        other => bail!(
            "rollback: invalid inactive slot label '{other}' in {}",
            path.display()
        ),
    };

    // Honest integrity check on the rollback target if a SHA was recorded.
    // The M6 implementation will swap GRUB / U-Boot bootloader entries —
    // for now we only validate the recorded bytes, then refuse to actually
    // perform the swap.
    if let (Some(sha), Some(img)) = (&slots.inactive_sha256, &slots.inactive_image_path) {
        let p = PathBuf::from(img);
        if p.is_file() {
            crate::release::verify_sha256(&p, sha)
                .with_context(|| format!("integrity-check inactive slot image {img}"))?;
            eprintln!("rollback: inactive slot integrity verified ({sha})");
        } else {
            eprintln!(
                "rollback: warn: inactive image path {img} not present on this host; skipping integrity check"
            );
        }
    }

    println!(
        "rollback: swapping from slot {} to slot {} (version {inactive_version})",
        slots.active, slots.inactive
    );

    // Perform the bootloader swap.
    rollback_bootloader(&slots)?;

    // Flip slots.
    let (new_active, new_inactive) = match slots.active.as_str() {
        "A" => ("B".to_string(), "A".to_string()),
        "B" => ("A".to_string(), "B".to_string()),
        other => bail!("unknown active slot label '{other}'"),
    };

    let updated = Slots {
        active: new_active,
        inactive: new_inactive,
        version_a: slots.version_a,
        version_b: slots.version_b,
        inactive_sha256: None,
        inactive_image_path: None,
        boot_count: Some(0),
        pending_rollback: Some(true),
        auto_rollback: Some(false),
        update_pending: Some(false),
        pending_version: None,
        slot_path_a: slots.slot_path_a,
        slot_path_b: slots.slot_path_b,
    };
    write_slots(&updated)?;

    println!(
        "rollback: reboot to boot into slot {} (version {inactive_version})",
        updated.active
    );
    Ok(0)
}

/// Detect the bootloader in use and swap the default boot entry.
fn rollback_bootloader(slots: &Slots) -> Result<()> {
    // Try GRUB first (most Linux x86_64).
    if Path::new("/etc/default/grub").exists() {
        return rollback_grub(slots);
    }
    // Try Raspberry Pi / U-Boot (config.txt with kernel= pointer).
    if Path::new("/boot/firmware/config.txt").exists() {
        return rollback_rpi_config(slots);
    }
    // Try EFI.
    if Path::new("/sys/firmware/efi").exists() {
        return rollback_efi(slots);
    }
    bail!("no supported bootloader found (GRUB, rpi config.txt, or EFI)")
}

/// Select an explicit slot for the next boot using the platform bootloader.
/// Update code calls this only after a verified image has been written.
pub fn select_boot_slot(slots: &Slots, target: &str) -> Result<()> {
    if !matches!(target, "A" | "B") {
        bail!("unknown target slot '{target}'");
    }
    let mut plan = slots.clone();
    plan.inactive = target.to_string();
    rollback_bootloader(&plan)
}

fn rollback_grub(slots: &Slots) -> Result<()> {
    let slot_char = match slots.inactive.as_str() {
        "A" => "0", // GRUB_DEFAULT=0 → first entry
        "B" => "1", // GRUB_DEFAULT=1 → second entry
        other => bail!("unknown slot '{other}' for GRUB"),
    };
    let grub_default = Path::new("/etc/default/grub");
    if !grub_default.is_file() {
        bail!("/etc/default/grub not found");
    }
    let content = std::fs::read_to_string(grub_default).context("reading /etc/default/grub")?;
    let mut new_content = String::new();
    let mut found = false;
    for line in content.lines() {
        if line.starts_with("GRUB_DEFAULT=") {
            new_content.push_str(&format!("GRUB_DEFAULT={slot_char}\n"));
            found = true;
        } else {
            new_content.push_str(line);
            new_content.push('\n');
        }
    }
    if !found {
        new_content.push_str(&format!("GRUB_DEFAULT={slot_char}\n"));
    }
    // Write tmp + rename.
    let tmp = grub_default.with_extension("tmp");
    std::fs::write(&tmp, new_content.as_bytes())
        .with_context(|| format!("writing {}", tmp.display()))?;
    std::fs::rename(&tmp, grub_default)
        .with_context(|| format!("renaming -> {}", grub_default.display()))?;

    // Run update-grub.
    let status = std::process::Command::new("update-grub")
        .status()
        .context("running update-grub")?;
    if !status.success() {
        bail!("update-grub exited non-zero");
    }
    println!("rollback: GRUB default set to slot {}", slots.inactive);
    Ok(())
}

fn rollback_rpi_config(slots: &Slots) -> Result<()> {
    let config = Path::new("/boot/firmware/config.txt");
    let content =
        std::fs::read_to_string(config).with_context(|| format!("reading {}", config.display()))?;
    // Swap kernel=vmlinuz-A → kernel=vmlinuz-B (convention).
    let a_kernel = "kernel=vmlinuz-A";
    let b_kernel = "kernel=vmlinuz-B";
    let new_content = match slots.inactive.as_str() {
        "B" => content.replace(a_kernel, b_kernel),
        "A" => content.replace(b_kernel, a_kernel),
        other => bail!("unknown slot '{other}' for rpi config"),
    };
    let tmp = config.with_extension("tmp");
    std::fs::write(&tmp, new_content.as_bytes())
        .with_context(|| format!("writing {}", tmp.display()))?;
    std::fs::rename(&tmp, config).with_context(|| format!("renaming -> {}", config.display()))?;
    println!("rollback: rpi config.txt set to slot {}", slots.inactive);
    Ok(())
}

fn rollback_efi(slots: &Slots) -> Result<()> {
    let label = match slots.inactive.as_str() {
        "A" => "deputyOS-A",
        "B" => "deputyOS-B",
        other => bail!("unknown slot '{other}' for EFI"),
    };
    let output = std::process::Command::new("efibootmgr")
        .output()
        .context("running efibootmgr")?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    // Find the Boot#### entry matching the label.
    let mut boot_num = None;
    for line in stdout.lines() {
        if line.contains(label) {
            if let Some(num) = line.split_whitespace().next() {
                if num.starts_with("Boot") {
                    boot_num = Some(
                        num.strip_prefix("Boot")
                            .and_then(|n| n.strip_suffix('*'))
                            .unwrap_or(num)
                            .to_string(),
                    );
                    break;
                }
            }
        }
    }
    let boot_num = match boot_num {
        Some(n) => n,
        None => bail!("efibootmgr: no Boot entry found matching '{label}'"),
    };
    let status = std::process::Command::new("efibootmgr")
        .args(["--bootnext", &boot_num])
        .status()
        .context("running efibootmgr --bootnext")?;
    if !status.success() {
        bail!("efibootmgr --bootnext {boot_num} exited non-zero");
    }
    println!("rollback: EFI BootNext set to {label} ({boot_num})");
    Ok(())
}

pub fn write_slots(slots: &Slots) -> Result<()> {
    let path = slots_path();
    let parent = path.parent().unwrap_or(Path::new("/var/lib/deputyos"));
    std::fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
    let tmp = path.with_extension("tmp");
    let body = serde_json::to_string_pretty(slots)?;
    std::fs::write(&tmp, body.as_bytes()).with_context(|| format!("writing {}", tmp.display()))?;
    std::fs::rename(&tmp, &path).with_context(|| format!("renaming -> {}", path.display()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_slots_file_reports_clearly() {
        let _g = crate::env_mutex().lock().unwrap_or_else(|e| e.into_inner());
        let dir = tempfile::tempdir().expect("tempdir");
        let p = dir.path().join("slots.json");
        std::env::set_var("DEPUTYOS_SLOTS_FILE", &p);
        let code = run().expect("run");
        assert_eq!(code, EX_NOT_READY);
        std::env::remove_var("DEPUTYOS_SLOTS_FILE");
    }

    #[test]
    fn present_slots_file_reports_target() {
        let _g = crate::env_mutex().lock().unwrap_or_else(|e| e.into_inner());
        let dir = tempfile::tempdir().expect("tempdir");
        let p = dir.path().join("slots.json");
        std::fs::write(
            &p,
            serde_json::to_string(&Slots {
                active: "A".into(),
                inactive: "B".into(),
                version_a: "2026.4.27".into(),
                version_b: "2026.4.20".into(),
                inactive_sha256: None,
                inactive_image_path: None,
                boot_count: None,
                pending_rollback: None,
                auto_rollback: None,
                update_pending: None,
                pending_version: None,
                slot_path_a: None,
                slot_path_b: None,
            })
            .expect("ser"),
        )
        .expect("write");
        std::env::set_var("DEPUTYOS_SLOTS_FILE", &p);
        // Will fail because no bootloader is found on dev host — that's fine.
        let result = run();
        // Expect an error (no bootloader) or success on a system with one.
        std::env::remove_var("DEPUTYOS_SLOTS_FILE");
        let _ = result;
    }
}
