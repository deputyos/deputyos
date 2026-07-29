//! `deputyctl factory-reset` — wipe the data partition; preserve system.
//!
//! Per `docs/02-profiles.md`, this is M5 territory. We ship a real but
//! conservative implementation: stop the active unit, wipe the user's data
//! dir (preserving `~/.ssh/authorized_keys`), truncate
//! `/etc/deputyos/secrets.env` (mode 0600 retained), and clear the
//! active-profile + wizard-state pointers so first-boot UX repeats.
//!
//! In dev mode (`DEPUTYOS_DEV_OUT=<dir>`) the wipe targets only paths under
//! `<dev-out>/`, so a contributor laptop never loses real data. The
//! confirmation prompt requires the literal phrase "reset deputyos" to
//! continue (unless `--yes` is passed).

use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use crate::paths;
use crate::profile;
use crate::systemd;

#[derive(Debug, Clone, Default)]
pub struct ResetOpts {
    pub yes: bool,
    pub dry_run: bool,
    /// In tests we feed a stdin replacement directly.
    pub confirmation_override: Option<String>,
}

const REQUIRED_PHRASE: &str = "reset deputyos";

pub fn run(opts: ResetOpts) -> Result<u8> {
    let dev = paths::dev_out_dir();

    // Discover paths to wipe.
    let plan = build_plan(dev.as_deref())?;
    print_plan(&plan, dev.is_some());

    if opts.dry_run {
        println!("(dry-run) no changes made");
        return Ok(0);
    }

    if !opts.yes {
        let typed = match opts.confirmation_override {
            Some(s) => s,
            None => {
                eprint!(
                    "This wipes all conversations, model keys, and learned skills. Are you sure?\n\
                     Type 'reset deputyos' to confirm: "
                );
                let _ = std::io::stderr().flush();
                let mut buf = String::new();
                std::io::stdin().read_line(&mut buf)?;
                buf.trim().to_string()
            }
        };
        if typed != REQUIRED_PHRASE {
            eprintln!("factory-reset: confirmation phrase mismatch — aborting");
            return Ok(1);
        }
    }

    // Stop the active unit (best effort, real mode only).
    if dev.is_none() && systemd::available() {
        if let Ok((_id, m)) = profile::load_active() {
            let _ = systemd::run("stop", &m.service.unit);
        }
    }

    // Wipe data dir, preserving authorized_keys when it sits under the
    // home directory parent.
    if plan.data_dir.exists() {
        wipe_data_dir(&plan.data_dir, &plan.preserve)?;
    }

    // Truncate secrets.env (preserve mode 0600).
    if plan.secrets.exists() {
        truncate_preserving_mode(&plan.secrets)?;
    }

    // Remove active-profile and wizard-state.
    for p in [&plan.active_profile_file, &plan.wizard_state_file] {
        if p.exists() {
            std::fs::remove_file(p).with_context(|| format!("removing {}", p.display()))?;
        }
    }

    println!("factory-reset complete; reboot or run `deputyctl init`");
    Ok(0)
}

#[derive(Debug)]
struct Plan {
    data_dir: PathBuf,
    preserve: Vec<PathBuf>,
    secrets: PathBuf,
    active_profile_file: PathBuf,
    wizard_state_file: PathBuf,
}

fn build_plan(dev_out: Option<&Path>) -> Result<Plan> {
    let (data_dir, preserve) = if let Some(dev) = dev_out {
        // Dev mode: wipe only what we created in dev-out.
        let dd = dev.join("data");
        let preserve = vec![dev.join("data").join(".ssh").join("authorized_keys")];
        (dd, preserve)
    } else {
        // Real mode: read the active profile's data_dir, preserve authorized_keys
        // under its parent ($HOME).
        let (_id, m) = match profile::load_active() {
            Ok(x) => x,
            Err(_) => {
                // No active profile — fall back to the documented user home.
                return Ok(Plan {
                    data_dir: PathBuf::from("/home/agent"),
                    preserve: vec![PathBuf::from("/home/agent/.ssh/authorized_keys")],
                    secrets: paths::secrets_file(),
                    active_profile_file: paths::active_profile_file(),
                    wizard_state_file: paths::wizard_state_file(),
                });
            }
        };
        let dd = PathBuf::from(&m.paths.data_dir);
        // Preserve authorized_keys at $HOME/.ssh/authorized_keys where $HOME
        // is the parent of data_dir's parent if present, else /home/agent.
        let home = dd
            .parent()
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("/home/agent"));
        let preserve = vec![home.join(".ssh").join("authorized_keys")];
        (dd, preserve)
    };
    let secrets = if let Some(dev) = dev_out {
        dev.join("secrets.env")
    } else {
        paths::secrets_file()
    };
    let active_profile_file = if let Some(dev) = dev_out {
        dev.join("active-profile")
    } else {
        paths::active_profile_file()
    };
    let wizard_state_file = if let Some(dev) = dev_out {
        dev.join("wizard-state.json")
    } else {
        paths::wizard_state_file()
    };

    Ok(Plan {
        data_dir,
        preserve,
        secrets,
        active_profile_file,
        wizard_state_file,
    })
}

fn print_plan(plan: &Plan, dev: bool) {
    println!("plan{}:", if dev { " (dev mode)" } else { "" });
    println!("  - stop active profile unit");
    println!("  - wipe {}", plan.data_dir.display());
    for p in &plan.preserve {
        println!("    (preserving {})", p.display());
    }
    println!("  - truncate {}", plan.secrets.display());
    println!("  - remove {}", plan.active_profile_file.display());
    println!("  - remove {}", plan.wizard_state_file.display());
}

fn wipe_data_dir(data_dir: &Path, preserve: &[PathBuf]) -> Result<()> {
    // Stash preserved files in tempfiles, wipe the dir, restore.
    let mut stashed: Vec<(PathBuf, Vec<u8>, Option<u32>)> = Vec::new();
    for p in preserve {
        if p.is_file() {
            let body = std::fs::read(p)
                .with_context(|| format!("reading preserved file {}", p.display()))?;
            #[cfg(unix)]
            let mode = {
                use std::os::unix::fs::PermissionsExt;
                std::fs::metadata(p).ok().map(|md| md.permissions().mode())
            };
            #[cfg(not(unix))]
            let mode: Option<u32> = None;
            stashed.push((p.clone(), body, mode));
        }
    }
    // Walk and remove every entry under data_dir.
    for entry in
        std::fs::read_dir(data_dir).with_context(|| format!("reading {}", data_dir.display()))?
    {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            std::fs::remove_dir_all(&path).with_context(|| format!("rm -rf {}", path.display()))?;
        } else {
            std::fs::remove_file(&path).with_context(|| format!("rm {}", path.display()))?;
        }
    }
    // Restore preserved files.
    for (path, body, mode) in stashed {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("mkdir -p {}", parent.display()))?;
        }
        std::fs::write(&path, &body).with_context(|| format!("restore {}", path.display()))?;
        #[cfg(unix)]
        if let Some(m) = mode {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(m));
        }
        #[cfg(not(unix))]
        let _ = mode;
    }
    Ok(())
}

fn truncate_preserving_mode(path: &Path) -> Result<()> {
    #[cfg(unix)]
    let mode = {
        use std::os::unix::fs::PermissionsExt;
        std::fs::metadata(path)
            .ok()
            .map(|md| md.permissions().mode() & 0o777)
    };
    std::fs::write(path, b"").with_context(|| format!("truncating {}", path.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let m = mode.unwrap_or(0o600);
        let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(m));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stage_dev_out(dir: &tempfile::TempDir) -> PathBuf {
        let dev = dir.path().join("dev-out");
        let data = dev.join("data");
        std::fs::create_dir_all(data.join(".ssh")).expect("mkdir ssh");
        std::fs::create_dir_all(data.join("conversations")).expect("mkdir convo");
        std::fs::write(
            data.join(".ssh").join("authorized_keys"),
            "ssh-ed25519 AAAA",
        )
        .expect("auth keys");
        std::fs::write(data.join("conversations").join("c1.json"), "[]").expect("convo");
        std::fs::write(dev.join("secrets.env"), "OPENROUTER_API_KEY=sk-test\n").expect("sec");
        std::fs::write(dev.join("active-profile"), "openclaw\n").expect("active");
        std::fs::write(dev.join("wizard-state.json"), "{}").expect("wizard");
        std::env::set_var("DEPUTYOS_DEV_OUT", &dev);
        dev
    }

    fn cleanup() {
        std::env::remove_var("DEPUTYOS_DEV_OUT");
    }

    #[test]
    fn dry_run_prints_plan_no_changes() {
        let _g = crate::env_mutex().lock().unwrap_or_else(|e| e.into_inner());
        let dir = tempfile::tempdir().expect("tempdir");
        let dev = stage_dev_out(&dir);
        let code = run(ResetOpts {
            yes: true,
            dry_run: true,
            confirmation_override: None,
        })
        .expect("run");
        assert_eq!(code, 0);
        // Files still present.
        assert!(dev
            .join("data")
            .join("conversations")
            .join("c1.json")
            .is_file());
        assert!(dev.join("secrets.env").is_file());
        cleanup();
    }

    #[test]
    fn wrong_confirmation_aborts() {
        let _g = crate::env_mutex().lock().unwrap_or_else(|e| e.into_inner());
        let dir = tempfile::tempdir().expect("tempdir");
        let dev = stage_dev_out(&dir);
        let code = run(ResetOpts {
            yes: false,
            dry_run: false,
            confirmation_override: Some("definitely not".into()),
        })
        .expect("run");
        assert_eq!(code, 1);
        // Files still present.
        assert!(dev
            .join("data")
            .join("conversations")
            .join("c1.json")
            .is_file());
        cleanup();
    }

    #[test]
    fn yes_wipes_dev_out_preserving_authorized_keys() {
        let _g = crate::env_mutex().lock().unwrap_or_else(|e| e.into_inner());
        let dir = tempfile::tempdir().expect("tempdir");
        let dev = stage_dev_out(&dir);
        let code = run(ResetOpts {
            yes: true,
            dry_run: false,
            confirmation_override: None,
        })
        .expect("run");
        assert_eq!(code, 0);
        assert!(
            dev.join("data")
                .join(".ssh")
                .join("authorized_keys")
                .is_file(),
            "authorized_keys must be preserved"
        );
        assert!(
            !dev.join("data")
                .join("conversations")
                .join("c1.json")
                .exists(),
            "conversations must be wiped"
        );
        // secrets truncated, not removed.
        let body = std::fs::read_to_string(dev.join("secrets.env")).expect("read");
        assert!(body.is_empty(), "secrets.env should be truncated");
        // pointers gone.
        assert!(!dev.join("active-profile").exists());
        assert!(!dev.join("wizard-state.json").exists());
        cleanup();
    }
}
