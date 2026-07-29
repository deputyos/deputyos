//! `deputyctl profile switch <id>` — atomic active-profile swap.
//!
//! Per `docs/02-profiles.md` §"What `deputyctl` does with a manifest", the
//! switch reads `[paths]`, `[service]`, `[apparmor]` from the *new* manifest,
//! stops the old systemd unit, atomically updates `/etc/deputyos/active-profile`,
//! reloads the new AppArmor profile, starts the new unit, then runs the
//! `[upgrade].post_upgrade_hooks`.
//!
//! The switch is **committed** the moment the active-profile pointer is
//! swapped — every subsequent step is best-effort. Hook failures and AppArmor
//! reload failures log warnings but do not unwind the swap. This matches the
//! M0 frozen contract; recovery from a bad post-hook is the user's job
//! (`deputyctl doctor` to identify, `deputyctl profile switch <old>` to revert).

use std::io::{IsTerminal, Write};
use std::path::PathBuf;
use std::process::Command;

use anyhow::{Context, Result};

use crate::manifest::Manifest;
use crate::paths;
use crate::profile;
use crate::systemd;

/// Caller-supplied flags for `profile switch`.
#[derive(Debug, Clone, Copy, Default)]
pub struct SwitchOpts {
    pub yes: bool,
    pub dry_run: bool,
}

/// Execute the switch. Returns the exit code deputyctl should use.
pub fn run(target_id: &str, opts: SwitchOpts) -> Result<u8> {
    // 1. Validate target exists.
    let target_manifest = match profile::load_by_id(target_id) {
        Ok(m) => m,
        Err(_) => {
            eprintln!("unknown profile: {target_id} (run `deputyctl profile list`)");
            return Ok(1);
        }
    };

    // 2. Read current active. None is allowed (first activation).
    let current_id = paths::read_active_profile_id();
    if current_id.as_deref() == Some(target_id) {
        println!("already active: {target_id}");
        return Ok(0);
    }

    // Load current manifest if there is a current active so we can stop it.
    let current_unit = current_id.as_deref().and_then(|id| {
        profile::load_by_id(id)
            .ok()
            .map(|m| (id.to_string(), m.service.unit))
    });

    // 3. Confirm.
    if !opts.yes && !opts.dry_run && !confirm_switch(current_id.as_deref(), target_id)? {
        println!("aborted");
        return Ok(1);
    }

    print_plan(current_id.as_deref(), target_id, &target_manifest);

    if opts.dry_run {
        println!("(dry-run) no changes made");
        return Ok(0);
    }

    // 4. Stop current unit (best effort).
    if let Some((_id, unit)) = &current_unit {
        if systemd::available() {
            tracing::info!(unit, "stopping current profile unit");
            if let Err(e) = systemd::run("stop", unit) {
                eprintln!("warn: could not stop {unit}: {e}");
            }
        } else {
            eprintln!("warn: systemd unavailable; skipping stop of {unit}");
        }
    }

    // 5. Atomic swap of active-profile pointer.
    swap_active_profile(target_id).context("swapping active profile pointer")?;
    println!(
        "switched: {} -> {}",
        current_id.as_deref().unwrap_or("(none)"),
        target_id,
    );

    // 6. Reload AppArmor profile (best effort).
    if let Some(aa) = &target_manifest.apparmor {
        if systemd::apparmor_parser_available() {
            tracing::info!(profile = %aa.profile, "reloading apparmor profile");
            match systemd::apparmor_reload(&aa.profile) {
                Ok(s) if s.success() => {}
                Ok(s) => eprintln!("warn: apparmor_parser exited {s}"),
                Err(e) => eprintln!("warn: apparmor_parser failed: {e}"),
            }
        } else {
            eprintln!(
                "warn: apparmor_parser not available; skipping reload of {}",
                aa.profile
            );
        }
    }

    // 7. Start the new unit.
    let new_unit = &target_manifest.service.unit;
    if systemd::available() {
        match systemd::run("start", new_unit) {
            Ok(s) if s.success() => {}
            Ok(s) => eprintln!("warn: systemctl start {new_unit} exited {s}"),
            Err(e) => eprintln!("warn: could not start {new_unit}: {e}"),
        }
    } else {
        eprintln!("warn: systemd unavailable; skipping start of {new_unit}");
    }

    // 8. Run post-upgrade hooks (warn but don't fail on nonzero).
    if let Some(up) = &target_manifest.upgrade {
        for hook in &up.post_upgrade_hooks {
            run_hook(hook);
        }
    }

    // 9. Print final active state one-liner.
    if systemd::available() {
        let state = systemd::is_active(new_unit)
            .map(|s| s.as_str().to_string())
            .unwrap_or_else(|_| "unknown".into());
        println!("unit:    {new_unit}");
        println!("state:   {state}");
    }
    Ok(0)
}

fn print_plan(current: Option<&str>, target: &str, m: &Manifest) {
    println!("plan:");
    if let Some(c) = current {
        println!("  - stop unit for current profile \"{c}\"");
    } else {
        println!("  - (no current active profile)");
    }
    println!(
        "  - swap {} -> \"{target}\"",
        paths::active_profile_file().display()
    );
    if let Some(aa) = &m.apparmor {
        println!("  - reload AppArmor profile {}", aa.profile);
    }
    println!("  - start unit {}", m.service.unit);
    if let Some(up) = &m.upgrade {
        for h in &up.post_upgrade_hooks {
            println!("  - run post-upgrade hook: {h}");
        }
    }
}

fn confirm_switch(current: Option<&str>, target: &str) -> Result<bool> {
    let mut stdin = std::io::stdin();
    if !stdin.is_terminal() {
        // Non-interactive without --yes: refuse.
        eprintln!(
            "refusing to switch profile without confirmation (re-run with --yes for non-interactive use)",
        );
        return Ok(false);
    }
    let prompt = match current {
        Some(c) => format!("switch profile \"{c}\" -> \"{target}\"? [y/N] "),
        None => format!("activate profile \"{target}\"? [y/N] "),
    };
    let mut stdout = std::io::stdout();
    write!(stdout, "{prompt}")?;
    stdout.flush()?;
    let mut buf = String::new();
    let n = std::io::Read::read_to_string(&mut stdin, &mut buf)?;
    if n == 0 {
        return Ok(false);
    }
    let answer = buf.trim().to_ascii_lowercase();
    Ok(matches!(answer.as_str(), "y" | "yes"))
}

/// Atomic write of `<active-profile-file>` via tmp + rename in same dir.
fn swap_active_profile(new_id: &str) -> Result<()> {
    let dst = paths::active_profile_file();
    let parent = dst
        .parent()
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
    std::fs::create_dir_all(&parent)
        .with_context(|| format!("creating {} for active-profile pointer", parent.display()))?;
    let tmp = dst.with_extension("tmp");
    std::fs::write(&tmp, format!("{new_id}\n"))
        .with_context(|| format!("writing {}", tmp.display()))?;
    std::fs::rename(&tmp, &dst)
        .with_context(|| format!("renaming {} -> {}", tmp.display(), dst.display()))?;
    Ok(())
}

fn run_hook(cmd: &str) {
    println!("hook: {cmd}");
    let result = Command::new("sh").arg("-c").arg(cmd).status();
    match result {
        Ok(s) if s.success() => {}
        Ok(s) => eprintln!("warn: hook exited {s}: {cmd}"),
        Err(e) => eprintln!("warn: failed to spawn hook \"{cmd}\": {e}"),
    }
}
