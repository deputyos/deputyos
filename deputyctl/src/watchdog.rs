//! Boot-count watchdog and post-update health confirmation.
//!
//! Called by `deputyos-watchdog.service` at early boot via the hidden
//! `--internal-watchdog-check` flag. Reads `/var/lib/deputyos/slots.json`,
//! increments `boot_count`, and auto-rolls back if the count exceeds the
//! threshold (default 3).

use std::path::Path;

use anyhow::{Context, Result};

use crate::rollback::{write_slots, Slots};

const ROLLBACK_THRESHOLD: u32 = 3;

pub fn run_check() -> Result<()> {
    let path = Path::new("/var/lib/deputyos/slots.json");
    if !path.is_file() {
        // No A/B slots provisioned — nothing to check. Cloud targets and
        // first-boot appliances don't have slots.json yet.
        return Ok(());
    }

    let raw =
        std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    let mut slots: Slots =
        serde_json::from_str(&raw).with_context(|| format!("parsing {}", path.display()))?;

    // If there was a user-requested rollback, this boot succeeded on the
    // rollback target — clear the pending flag and reset boot count.
    if slots.pending_rollback.unwrap_or(false) {
        slots.boot_count = Some(0);
        slots.pending_rollback = Some(false);
        slots.update_pending = Some(false);
        slots.pending_version = None;
        write_slots(&slots)?;
        return Ok(());
    }

    // Ordinary boots are never counted. Only a newly selected update slot is
    // subject to the rollback window.
    if !slots.update_pending.unwrap_or(false) {
        return Ok(());
    }

    // Increment the boot counter. This is the normal path: we booted,
    // this is the Nth attempt since the last successful boot.
    let count = slots.boot_count.unwrap_or(0) + 1;
    slots.boot_count = Some(count);

    if count >= ROLLBACK_THRESHOLD {
        eprintln!(
            "watchdog: boot_count={count} >= threshold={ROLLBACK_THRESHOLD} — auto-rolling back"
        );
        // Swap to the inactive slot.
        let inactive_slot = slots.inactive.clone();
        let inactive_version = match inactive_slot.as_str() {
            "A" => slots.version_a.clone(),
            "B" => slots.version_b.clone(),
            _ => "unknown".into(),
        };

        // Flip slots.
        let (new_active, new_inactive) = match slots.active.as_str() {
            "A" => ("B".to_string(), "A".to_string()),
            "B" => ("A".to_string(), "B".to_string()),
            other => anyhow::bail!("unknown active slot '{other}'"),
        };

        slots.active = new_active;
        slots.inactive = new_inactive;
        slots.boot_count = Some(0);
        slots.auto_rollback = Some(true);
        slots.update_pending = Some(false);
        slots.pending_version = None;
        write_slots(&slots)?;

        // Reboot into the previous slot.
        eprintln!(
            "watchdog: auto-rollback complete — slot={inactive_slot} version={inactive_version}. rebooting."
        );
        let _ = std::process::Command::new("reboot").status();
    } else {
        write_slots(&slots)?;
        eprintln!("watchdog: boot_count incremented to {count}");
    }

    Ok(())
}

/// Confirm the pending slot only after the active profile answers its typed
/// health endpoint. A failed confirmation leaves `update_pending` set so the
/// systemd retry policy and next boot remain inside the rollback window.
pub fn run_confirm() -> Result<()> {
    let path = Path::new("/var/lib/deputyos/slots.json");
    if !path.is_file() {
        return Ok(());
    }
    let raw =
        std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    let mut slots: Slots =
        serde_json::from_str(&raw).with_context(|| format!("parsing {}", path.display()))?;
    if !slots.update_pending.unwrap_or(false) {
        return Ok(());
    }

    let (_, manifest) = crate::profile::load_active()?;
    let url = &manifest.health.http_check;
    let response = ureq::AgentBuilder::new()
        .timeout_connect(std::time::Duration::from_secs(3))
        .timeout(std::time::Duration::from_secs(10))
        .build()
        .get(url)
        .call()
        .with_context(|| format!("post-update health check GET {url}"))?;
    if !(200..400).contains(&response.status()) {
        anyhow::bail!(
            "post-update health check returned HTTP {}",
            response.status()
        );
    }

    if let Some(version) = slots.pending_version.take() {
        std::fs::write("/etc/deputyos/version", format!("{version}\n"))
            .context("recording confirmed installed version")?;
    }
    slots.boot_count = Some(0);
    slots.update_pending = Some(false);
    slots.pending_rollback = Some(false);
    slots.auto_rollback = Some(false);
    write_slots(&slots)?;
    eprintln!("watchdog: pending update confirmed healthy");
    Ok(())
}
