//! macOS driver — UTM on Apple Silicon.
//!
//! Uses `utmctl` CLI for VM lifecycle management. Requires UTM 4.x+ installed
//! on an Apple Silicon Mac. Intel Macs are not supported (no Vz.framework
//! acceleration for aarch64 guests).
//!
//! ## Multi-instance (M9.5)
//!
//! The desktop console manages several named agents at once. Each
//! console-created instance gets its own UTM VM named `deputyos-<id>` (the
//! instance id, derived from the per-instance `cache_dir` layout — see
//! [`crate::instance::InstanceConfig::instance_slug`]). UTM stores each VM's
//! disk + config under its own per-VM directory, so naming the VM uniquely is
//! enough for two console-managed agents to register + start/stop as
//! independent VMs instead of colliding on one shared `deputyos` VM.
//!
//! The bare single-instance CLI (`deputyos-desktop install/start/stop` with no
//! registry) keeps using the fixed `VM_NAME = "deputyos"` via the trait's
//! default `*_with` delegation to `start`/`stop`/`status`/`install_image`.
//!
//! ### Multi-instance networking
//!
//! UTM shared networking places the host and guests on the same VLAN. Each
//! deputy therefore remains independently reachable at its QEMU Guest Agent
//! reported IP, even though `utmctl` does not expose an arbitrary localhost
//! port remap. The in-image outbound tunnel is independent of this LAN path,
//! so remote wizard and typed resident-agent access continue to work when no
//! inbound connection to the Mac is possible.

use std::path::Path;
use std::process::Command;
use std::time::Duration;

use anyhow::{bail, Context, Result};

use crate::driver::{Driver, DriverCapabilities, VmHandle, VmStatus};
use crate::instance::{InstanceConfig, ResourceSpec};

/// Fixed VM name for the bare single-instance CLI path.
const VM_NAME: &str = "deputyos";

pub struct MacOsDriver;

impl MacOsDriver {
    pub fn new() -> Self {
        Self
    }

    fn is_utm_installed() -> bool {
        Command::new("utmctl")
            .arg("version")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    }

    fn vm_exists(name: &str) -> bool {
        let output = Command::new("utmctl").arg("list").output().ok();
        match output {
            Some(o) => String::from_utf8_lossy(&o.stdout).contains(name),
            None => false,
        }
    }

    /// True if the named VM is in the "started" state.
    fn vm_running(name: &str) -> bool {
        vm_status_text(name).is_some_and(|status| status.contains("started"))
    }

    fn vm_paused(name: &str) -> bool {
        vm_status_text(name)
            .is_some_and(|status| status.contains("paused") || status.contains("suspended"))
    }
}

impl Default for MacOsDriver {
    fn default() -> Self {
        Self::new()
    }
}

/// Per-instance UTM VM name: `deputyos-<id>` for console-created instances, or
/// the bare `deputyos` for the single-instance CLI path (no slug).
fn vm_name(cfg: &InstanceConfig) -> String {
    match cfg.instance_slug() {
        Some(slug) => format!("deputyos-{slug}"),
        None => VM_NAME.to_string(),
    }
}

/// UTM's CLI intentionally has no create command. Creation goes through its
/// documented AppleScript `make new virtual machine` API.
fn create_script(name: &str, image: &Path, resources: &ResourceSpec) -> String {
    format!(
        "tell application \"UTM\"\n\
         set diskImage to POSIX file {}\n\
         make new virtual machine with properties {{backend:qemu, configuration:\
         {{name:{}, architecture:\"aarch64\", memory:{}, cpu cores:{}, \
         hypervisor:true, uefi:true, drives:{{{{interface:VirtIO, source:diskImage}}}}}}}}\n\
         end tell",
        applescript_string(&image.display().to_string()),
        applescript_string(name),
        resources.memory_max_mib,
        resources.vcpus,
    )
}

/// `utmctl start <name>` — pure arg builder.
fn start_argv(name: &str) -> Vec<String> {
    vec!["start".into(), name.to_string()]
}

/// `utmctl stop <name> --request` — ask the guest OS to shut down.
fn stop_argv(name: &str) -> Vec<String> {
    vec!["stop".into(), name.to_string(), "--request".into()]
}

/// `utmctl stop <name> --force` — explicit force fallback.
fn force_stop_argv(name: &str) -> Vec<String> {
    vec!["stop".into(), name.to_string(), "--force".into()]
}

/// `utmctl status <name>` — pure arg builder.
fn status_argv(name: &str) -> Vec<String> {
    vec!["status".into(), name.to_string()]
}

fn suspend_script(name: &str) -> String {
    format!(
        "tell application \"UTM\"\nset vm to virtual machine named {}\nsuspend vm with saving\nend tell",
        applescript_string(name)
    )
}

fn applescript_string(value: &str) -> String {
    format!("\"{}\"", value.replace('\\', "\\\\").replace('"', "\\\""))
}

fn deputyd_args(command: &deputyd::AgentCommand) -> Vec<String> {
    match command {
        deputyd::AgentCommand::Health => vec!["health".into()],
        deputyd::AgentCommand::PreparePause => vec!["prepare-pause".into()],
        deputyd::AgentCommand::Resume => vec!["resume".into()],
        deputyd::AgentCommand::Reclaim { drop_caches: false } => vec!["reclaim".into()],
        deputyd::AgentCommand::Reclaim { drop_caches: true } => {
            vec!["reclaim".into(), "--drop-caches".into()]
        }
        other => vec![
            "execute".into(),
            "--request-json".into(),
            serde_json::to_string(other).expect("serializing typed deputyd command"),
        ],
    }
}

fn guest_agent_argv(name: &str, command: &deputyd::AgentCommand) -> Vec<String> {
    let mut args = vec![
        "exec".into(),
        name.to_string(),
        "/usr/local/bin/deputyd".into(),
    ];
    args.extend(deputyd_args(command));
    args
}

fn vm_status_text(name: &str) -> Option<String> {
    let output = Command::new("utmctl")
        .args(status_argv(name))
        .output()
        .ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).to_ascii_lowercase())
}

fn ip_address_argv(name: &str) -> Vec<String> {
    vec!["ip-address".into(), name.to_string()]
}

fn vm_ip(name: &str) -> Option<String> {
    let output = Command::new("utmctl")
        .args(ip_address_argv(name))
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    String::from_utf8_lossy(&output.stdout)
        .split_whitespace()
        .find(|candidate| candidate.contains('.') && !candidate.starts_with("127."))
        .map(|candidate| candidate.to_string())
}

fn wait_for_vm_ip(name: &str) -> Option<String> {
    for _ in 0..20 {
        if let Some(ip) = vm_ip(name) {
            return Some(ip);
        }
        std::thread::sleep(Duration::from_millis(250));
    }
    None
}

fn vm_urls(name: &str) -> Vec<String> {
    vm_ip(name)
        .map(|ip| vec![format!("http://{ip}:8088"), format!("http://{ip}:8080")])
        .unwrap_or_default()
}

impl Driver for MacOsDriver {
    fn capabilities(&self) -> DriverCapabilities {
        DriverCapabilities {
            pause_resume: true,
            memory_balloon: false,
            guest_agent: true,
            // UTM's scripting bridge requests a saved suspend state.
            checkpoint: true,
            per_instance_resources: true,
        }
    }

    fn check_prereq(&self) -> Result<()> {
        if std::env::consts::ARCH != "aarch64" {
            bail!(
                "deputyOS macOS launcher requires an Apple Silicon Mac (aarch64). \
                 Intel Macs lack Vz.framework acceleration needed for aarch64 guests."
            );
        }
        if !Self::is_utm_installed() {
            bail!(
                "UTM is not installed. Install from the App Store: \
                 https://mac.getutm.app/ or `brew install --cask utm`."
            );
        }
        // Check UTM version >= 4.x.
        let output = Command::new("utmctl").arg("version").output().ok();
        if let Some(o) = output {
            let ver = String::from_utf8_lossy(&o.stdout);
            let major = ver
                .trim()
                .split('.')
                .next()
                .and_then(|value| value.parse::<u64>().ok());
            if major.is_none_or(|major| major < 4) {
                eprintln!(
                    "warn: utmctl version '{}' may not support the CLI used by this launcher. \
                     UTM 4.x+ is recommended.",
                    ver.trim()
                );
            }
        }
        Ok(())
    }

    fn target_for_host(&self) -> &'static str {
        "macos-qemu"
    }

    fn install_image(&self, image_path: &Path) -> Result<()> {
        if !image_path.is_file() {
            bail!("image not found: {}", image_path.display());
        }

        if Self::vm_exists(VM_NAME) {
            println!("UTM VM '{VM_NAME}' already registered — skipping create.");
            return Ok(());
        }

        println!(
            "registering UTM VM '{VM_NAME}' from {} ...",
            image_path.display()
        );
        let status = Command::new("osascript")
            .args([
                "-e",
                &create_script(VM_NAME, image_path, &ResourceSpec::default()),
            ])
            .status();

        match status {
            Ok(s) if s.success() => {
                println!("UTM VM '{VM_NAME}' registered.");
                Ok(())
            }
            Ok(s) => bail!("UTM create exited with status {}", s.code().unwrap_or(-1)),
            Err(e) => bail!("failed to ask UTM to create the VM: {e}"),
        }
    }

    /// Per-instance create: register `deputyos-<id>`. Idempotent.
    fn install_image_with(&self, image_path: &Path, cfg: &InstanceConfig) -> Result<()> {
        if !image_path.is_file() {
            bail!("image not found: {}", image_path.display());
        }
        let name = vm_name(cfg);
        if Self::vm_exists(&name) {
            println!("UTM VM '{name}' already registered — skipping create.");
            return Ok(());
        }
        println!(
            "registering UTM VM '{name}' from {} ...",
            image_path.display()
        );
        let status = Command::new("osascript")
            .args(["-e", &create_script(&name, image_path, &cfg.resources)])
            .status();
        match status {
            Ok(s) if s.success() => {
                println!("UTM VM '{name}' registered.");
                Ok(())
            }
            Ok(s) => bail!("UTM create exited with status {}", s.code().unwrap_or(-1)),
            Err(e) => bail!("failed to ask UTM to create the VM: {e}"),
        }
    }

    fn start(&self) -> Result<VmHandle> {
        self.start_with(&InstanceConfig::from_env())
    }

    fn stop(&self) -> Result<()> {
        self.stop_with(&InstanceConfig::from_env())
    }

    fn status(&self) -> Result<VmStatus> {
        self.status_with(&InstanceConfig::from_env())
    }

    /// Per-instance start: boot `deputyos-<id>`. Idempotent.
    fn start_with(&self, cfg: &InstanceConfig) -> Result<VmHandle> {
        let name = vm_name(cfg);
        if !Self::vm_exists(&name) {
            bail!("UTM VM '{name}' not found — run install for this instance first.");
        }
        if Self::vm_paused(&name) {
            return self.resume_with(cfg);
        }
        if Self::vm_running(&name) {
            println!("UTM VM '{name}' is already running.");
            return Ok(VmHandle { id: name.clone() });
        }
        println!("starting UTM VM '{name}'...");
        let status = Command::new("utmctl").args(start_argv(&name)).status();
        match status {
            Ok(s) if s.success() => {
                let endpoint = wait_for_vm_ip(&name)
                    .map(|ip| format!("http://{ip}:8088"))
                    .unwrap_or_else(|| "the authenticated deputyOS outbound tunnel".to_string());
                println!("UTM VM '{name}' started. Wizard at {endpoint}");
                Ok(VmHandle { id: name })
            }
            Ok(s) => bail!("utmctl start exited with status {}", s.code().unwrap_or(-1)),
            Err(e) => bail!("failed to run utmctl start: {e}"),
        }
    }

    /// Per-instance stop: soft-stop `deputyos-<id>`, force on failure. Idempotent.
    fn stop_with(&self, cfg: &InstanceConfig) -> Result<()> {
        let name = vm_name(cfg);
        if !Self::vm_exists(&name) || (!Self::vm_running(&name) && !Self::vm_paused(&name)) {
            return Ok(());
        }
        println!("stopping UTM VM '{name}'...");
        let status = Command::new("utmctl").args(stop_argv(&name)).status();
        match status {
            Ok(s) if s.success() => Ok(()),
            Ok(s) => {
                eprintln!(
                    "soft stop failed ({}), trying force stop...",
                    s.code().unwrap_or(-1)
                );
                let force = Command::new("utmctl").args(force_stop_argv(&name)).status();
                if force.map(|s| s.success()).unwrap_or(false) {
                    Ok(())
                } else {
                    bail!("failed to stop UTM VM '{name}'");
                }
            }
            Err(e) => bail!("failed to run utmctl stop: {e}"),
        }
    }

    /// Per-instance status: liveness of `deputyos-<id>`. The surfaced URL
    /// reflects the instance's allocated wizard port (see the module-level
    /// caveat about UTM's same-number localhost forwarding).
    fn status_with(&self, cfg: &InstanceConfig) -> Result<VmStatus> {
        let name = vm_name(cfg);
        let output = Command::new("utmctl").args(status_argv(&name)).output();
        match output {
            Ok(o) => {
                let stdout = String::from_utf8_lossy(&o.stdout);
                if stdout.contains("started") {
                    Ok(VmStatus::Running {
                        handle: VmHandle { id: name.clone() },
                        urls: vm_urls(&name),
                    })
                } else if stdout.contains("paused") || stdout.contains("suspended") {
                    Ok(VmStatus::Paused {
                        handle: VmHandle { id: name },
                    })
                } else {
                    Ok(VmStatus::Stopped)
                }
            }
            Err(_) => Ok(VmStatus::Stopped),
        }
    }

    fn pause_with(&self, cfg: &InstanceConfig) -> Result<()> {
        let name = vm_name(cfg);
        if Self::vm_paused(&name) {
            return Ok(());
        }
        if !Self::vm_running(&name) {
            bail!("UTM VM '{name}' is not running");
        }

        if self
            .guest_agent_with(cfg, deputyd::AgentCommand::PreparePause)
            .is_ok()
        {
            let _ =
                self.guest_agent_with(cfg, deputyd::AgentCommand::Reclaim { drop_caches: true });
        }

        let status = Command::new("osascript")
            .args(["-e", &suspend_script(&name)])
            .status()
            .with_context(|| format!("requesting saved suspend for UTM VM '{name}'"))?;
        if status.success() {
            Ok(())
        } else {
            let _ = self.guest_agent_with(cfg, deputyd::AgentCommand::Resume);
            bail!(
                "UTM saved suspend exited with status {}",
                status.code().unwrap_or(-1)
            )
        }
    }

    fn resume_with(&self, cfg: &InstanceConfig) -> Result<VmHandle> {
        let name = vm_name(cfg);
        if !Self::vm_paused(&name) && !Self::vm_running(&name) {
            bail!("UTM VM '{name}' is stopped, not paused");
        }
        if !Self::vm_running(&name) {
            let status = Command::new("utmctl").args(start_argv(&name)).status();
            match status {
                Ok(status) if status.success() => {}
                Ok(status) => {
                    bail!(
                        "utmctl resume exited with status {}",
                        status.code().unwrap_or(-1)
                    )
                }
                Err(error) => bail!("failed to resume UTM VM: {error}"),
            }
        }
        let _ = self.guest_agent_with(cfg, deputyd::AgentCommand::Resume);
        let _ = wait_for_vm_ip(&name);
        Ok(VmHandle { id: name })
    }

    fn set_memory_with(&self, _cfg: &InstanceConfig, _target_mib: u64) -> Result<()> {
        bail!(
            "UTM does not expose a supported live memory-balloon target; \
             change the stopped VM resource envelope or pause it with saved state"
        )
    }

    fn guest_agent_with(
        &self,
        cfg: &InstanceConfig,
        command: deputyd::AgentCommand,
    ) -> Result<deputyd::AgentResult> {
        let name = vm_name(cfg);
        if !Self::vm_running(&name) {
            bail!("UTM VM '{name}' is not running");
        }
        let output = Command::new("utmctl")
            .args(guest_agent_argv(&name, &command))
            .output()
            .with_context(|| format!("executing resident agent in UTM VM '{name}'"))?;
        if !output.status.success() {
            bail!(
                "resident agent execution failed: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            );
        }
        serde_json::from_slice(&output.stdout).with_context(|| {
            format!(
                "decoding resident-agent output: {}",
                String::from_utf8_lossy(&output.stdout).trim()
            )
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn cfg_for_slug(slug: Option<&str>) -> InstanceConfig {
        let cache_dir = match slug {
            Some(s) => PathBuf::from(format!("/data/instances/{s}/cache")),
            None => PathBuf::from("/data/cache"),
        };
        InstanceConfig {
            wizard_port: 17088,
            gateway_port: 17080,
            cache_dir,
            runtime_dir: PathBuf::from("/data/run"),
            seed_iso: None,
            resources: crate::instance::ResourceSpec::default(),
        }
    }

    #[test]
    fn vm_name_uses_instance_slug_when_present() {
        let cfg = cfg_for_slug(Some("abc-123"));
        assert_eq!(vm_name(&cfg), "deputyos-abc-123");
    }

    #[test]
    fn vm_name_falls_back_to_bare_for_single_instance() {
        let cfg = cfg_for_slug(None);
        assert_eq!(vm_name(&cfg), VM_NAME);
    }

    #[test]
    fn create_script_uses_documented_utm_record_shape() {
        let resources = ResourceSpec {
            vcpus: 4,
            memory_min_mib: 1024,
            memory_max_mib: 6144,
            auto_balloon: true,
        };
        let script = create_script(
            "deputyos-abc-123",
            &PathBuf::from("/cache/deputyos-macos-qemu.img"),
            &resources,
        );
        assert!(script.contains("make new virtual machine with properties {backend:qemu"));
        assert!(script.contains("name:\"deputyos-abc-123\""));
        assert!(script.contains("architecture:\"aarch64\""));
        assert!(script.contains("memory:6144"));
        assert!(script.contains("cpu cores:4"));
        assert!(script.contains("interface:VirtIO, source:diskImage"));
    }

    #[test]
    fn start_argv_targets_named_vm() {
        assert_eq!(
            start_argv("deputyos-abc-123"),
            vec!["start", "deputyos-abc-123"]
        );
    }

    #[test]
    fn stop_argv_requests_guest_shutdown() {
        assert_eq!(
            stop_argv("deputyos-abc-123"),
            vec!["stop", "deputyos-abc-123", "--request"]
        );
    }

    #[test]
    fn force_stop_argv_is_explicit() {
        assert_eq!(
            force_stop_argv("deputyos-abc-123"),
            vec!["stop", "deputyos-abc-123", "--force"]
        );
    }

    #[test]
    fn status_argv_targets_named_vm() {
        assert_eq!(
            status_argv("deputyos-abc-123"),
            vec!["status", "deputyos-abc-123"]
        );
    }

    #[test]
    fn ip_address_argv_targets_named_vm() {
        assert_eq!(
            ip_address_argv("deputyos-abc-123"),
            vec!["ip-address", "deputyos-abc-123"]
        );
    }

    #[test]
    fn suspend_requests_saved_state() {
        let script = suspend_script("deputyos-abc-123");
        assert!(script.contains("suspend vm with saving"));
        assert!(script.contains("virtual machine named \"deputyos-abc-123\""));
    }

    #[test]
    fn guest_agent_argv_executes_only_deputyd() {
        let args = guest_agent_argv(
            "deputyos-abc-123",
            &deputyd::AgentCommand::Reclaim { drop_caches: true },
        );
        assert_eq!(
            args,
            vec![
                "exec",
                "deputyos-abc-123",
                "/usr/local/bin/deputyd",
                "reclaim",
                "--drop-caches",
            ]
        );
    }

    #[test]
    fn applescript_string_escapes_vm_names() {
        assert_eq!(applescript_string("a\"b\\c"), "\"a\\\"b\\\\c\"");
    }
}
