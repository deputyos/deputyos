//! Bounded, typed health reconciliation for resident deputyOS services.
//!
//! This is intentionally not a generic command runner. The component list and
//! every permitted systemd action are compiled in. A two-strike threshold
//! avoids restarting an agent for a transient slow response; `--force` is an
//! explicit operator override.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

const FAILURE_THRESHOLD: u32 = 2;
const STATE_PATH: &str = "/var/lib/deputyos/reconcile-state.json";
const REPORT_PATH: &str = "/run/deputyos/reconcile.json";

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct ReconcileState {
    #[serde(default)]
    failures: BTreeMap<String, u32>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ComponentReport {
    pub name: String,
    pub unit: String,
    pub healthy: bool,
    pub consecutive_failures: u32,
    pub action: Option<String>,
    pub detail: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct ReconcileReport {
    pub checked_unix: u64,
    pub healthy: bool,
    pub components: Vec<ComponentReport>,
}

#[derive(Debug, Clone)]
struct Component {
    name: &'static str,
    unit: String,
    health_url: Option<String>,
    required: bool,
}

pub fn run(json: bool, force: bool) -> Result<u8> {
    let mut state = load_state();
    let components = components()?;
    let mut reports = Vec::with_capacity(components.len());

    for component in components {
        if !component.required {
            reports.push(ComponentReport {
                name: component.name.to_string(),
                unit: component.unit,
                healthy: true,
                consecutive_failures: 0,
                action: None,
                detail: "not provisioned; skipped".to_string(),
            });
            continue;
        }
        let initial = probe(&component);
        if initial.is_ok() {
            state.failures.remove(component.name);
            reports.push(ComponentReport {
                name: component.name.to_string(),
                unit: component.unit,
                healthy: true,
                consecutive_failures: 0,
                action: None,
                detail: "healthy".to_string(),
            });
            continue;
        }

        let failures = state
            .failures
            .entry(component.name.to_string())
            .or_default();
        *failures = failures.saturating_add(1);
        let mut action = None;
        let mut final_probe = initial;
        if force || *failures >= FAILURE_THRESHOLD {
            let verb = if unit_active(&component.unit) {
                "restart"
            } else {
                "start"
            };
            action = Some(format!("{verb} {}", component.unit));
            final_probe = systemctl(verb, &component.unit).and_then(|_| {
                std::thread::sleep(Duration::from_secs(2));
                probe(&component)
            });
            if final_probe.is_ok() {
                state.failures.remove(component.name);
            }
        }
        let consecutive_failures = state.failures.get(component.name).copied().unwrap_or(0);
        reports.push(ComponentReport {
            name: component.name.to_string(),
            unit: component.unit,
            healthy: final_probe.is_ok(),
            consecutive_failures,
            action,
            detail: match final_probe {
                Ok(()) => "recovered".to_string(),
                Err(error) => format!("{error:#}"),
            },
        });
    }

    save_state(&state)?;
    let report = ReconcileReport {
        checked_unix: now_unix(),
        healthy: reports.iter().all(|report| report.healthy),
        components: reports,
    };
    write_report(&report)?;
    if json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        for component in &report.components {
            let status = if component.healthy {
                "healthy"
            } else {
                "unhealthy"
            };
            match &component.action {
                Some(action) => println!(
                    "{:<18} {:<9} {} ({})",
                    component.name, status, component.detail, action
                ),
                None => println!("{:<18} {:<9} {}", component.name, status, component.detail),
            }
        }
    }
    Ok(if report.healthy { 0 } else { 1 })
}

fn components() -> Result<Vec<Component>> {
    let (_, manifest) = crate::profile::load_active()?;
    Ok(vec![
        Component {
            name: "resident-agent",
            unit: "deputyd.service".to_string(),
            health_url: None,
            required: true,
        },
        Component {
            name: "profile-agent",
            unit: manifest.service.unit,
            health_url: Some(manifest.health.http_check),
            required: true,
        },
        Component {
            name: "remote-terminal",
            unit: "deputy-terminal.service".to_string(),
            health_url: Some("http://127.0.0.1:8090/healthz".to_string()),
            required: true,
        },
        Component {
            name: "external-tunnel",
            unit: "deputyos-tunnel.service".to_string(),
            health_url: None,
            required: crate::paths::tunnel_token_file().is_file(),
        },
        Component {
            name: "backup-scheduler",
            unit: "deputyos-backup.timer".to_string(),
            health_url: None,
            required: crate::paths::systemd_unit_dir()
                .join("deputyos-backup.timer")
                .is_file(),
        },
    ])
}

fn probe(component: &Component) -> Result<()> {
    if !unit_active(&component.unit) {
        anyhow::bail!("{} is not active", component.unit);
    }
    if let Some(url) = &component.health_url {
        let response = ureq::AgentBuilder::new()
            .timeout_connect(Duration::from_secs(2))
            .timeout(Duration::from_secs(5))
            .build()
            .get(url)
            .call()
            .with_context(|| format!("GET {url}"))?;
        if !(200..400).contains(&response.status()) {
            anyhow::bail!("GET {url} returned HTTP {}", response.status());
        }
    }
    Ok(())
}

fn unit_active(unit: &str) -> bool {
    Command::new("systemctl")
        .args(["is-active", "--quiet", unit])
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

fn systemctl(verb: &str, unit: &str) -> Result<()> {
    debug_assert!(matches!(verb, "start" | "restart"));
    let status = Command::new("systemctl")
        .args([verb, unit])
        .status()
        .with_context(|| format!("systemctl {verb} {unit}"))?;
    if !status.success() {
        anyhow::bail!("systemctl {verb} {unit} exited {status}");
    }
    Ok(())
}

fn state_path() -> PathBuf {
    std::env::var_os("DEPUTYOS_RECONCILE_STATE")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(STATE_PATH))
}

fn report_path() -> PathBuf {
    std::env::var_os("DEPUTYOS_RECONCILE_REPORT")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(REPORT_PATH))
}

fn load_state() -> ReconcileState {
    std::fs::read_to_string(state_path())
        .ok()
        .and_then(|raw| serde_json::from_str(&raw).ok())
        .unwrap_or_default()
}

fn save_state(state: &ReconcileState) -> Result<()> {
    atomic_json(&state_path(), state)
}

fn write_report(report: &ReconcileReport) -> Result<()> {
    atomic_json(&report_path(), report)
}

fn atomic_json(path: &Path, value: &impl Serialize) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating {}", parent.display()))?;
    }
    let temporary = path.with_extension("tmp");
    std::fs::write(&temporary, serde_json::to_vec_pretty(value)?)
        .with_context(|| format!("writing {}", temporary.display()))?;
    std::fs::rename(&temporary, path).with_context(|| format!("renaming to {}", path.display()))
}

fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn state_round_trips() {
        let mut state = ReconcileState::default();
        state.failures.insert("profile-agent".to_string(), 2);
        let raw = serde_json::to_string(&state).expect("serialize");
        let decoded: ReconcileState = serde_json::from_str(&raw).expect("deserialize");
        assert_eq!(decoded.failures["profile-agent"], 2);
    }
}
