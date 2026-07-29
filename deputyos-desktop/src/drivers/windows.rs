//! Windows driver — WSL2.
//!
//! Uses `wsl.exe` for distro lifecycle management. Requires WSL2 installed
//! with the `wsl` command on PATH (true for all Win10 21H2+ / Win11 with WSL).
//! The launcher imports the `wsl2` tarball artefact.
//!
//! ## Multi-instance (M9.5)
//!
//! The desktop console manages several named agents at once. Each
//! console-created instance gets its own WSL distro named `deputyos-<id>`
//! (the instance id, derived from the per-instance `cache_dir` layout — see
//! [`crate::instance::InstanceConfig::instance_slug`]) registered into a
//! per-instance rootfs dir under the instance's `runtime_dir`. So two
//! console-managed agents register + start/stop as **independent distros**
//! instead of colliding on one shared `deputyos` distro.
//!
//! The bare single-instance CLI (`deputyos-desktop install/start/stop` with no
//! registry) keeps using the fixed `DISTRO_NAME = "deputyos"` via the trait's
//! default `*_with` delegation to `start`/`stop`/`status`/`install_image`.
//!
//! ### Host port forwarding — per-instance remap
//!
//! WSL2 auto-forwards a distro's bound guest port to Windows `localhost` at
//! the *same* port number; it does not remap to an arbitrary host port. So two
//! simultaneously-running distros both serving their wizard on guest `:8088`
//! would otherwise contend for Windows `localhost:8088`. To give each
//! console instance a distinct host port, `start_with` reads the distro's WSL
//! IP (`wsl -d <name> hostname -I`) and installs a `netsh interface portproxy`
//! mapping `127.0.0.1:<wizard_port>` → `<wsl-ip>:8088` (and the gateway port);
//! `stop_with` tears it down. This is **best-effort**: `netsh portproxy`
//! typically needs an elevated shell, so if the remap can't be established the
//! instance still boots and a warning is logged (it's then reachable on the
//! default `localhost:8088` only). The single-instance path and any instance
//! left on the default 8088/8080 pair skip the remap (WSL2's own forward
//! already suffices). **Untested on real hardware** — the arg builders are
//! unit-tested; the live `netsh`/`wsl` behavior is verified on a Windows host.

use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{bail, Context, Result};

use crate::config;
use crate::driver::{Driver, DriverCapabilities, VmHandle, VmStatus};
use crate::instance::InstanceConfig;

/// Fixed distro name for the bare single-instance CLI path.
const DISTRO_NAME: &str = "deputyos";

pub struct WindowsDriver;

impl WindowsDriver {
    pub fn new() -> Self {
        Self
    }

    fn is_wsl_installed() -> bool {
        Command::new("wsl")
            .arg("--status")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    }

    fn distro_exists(name: &str) -> bool {
        let output = Command::new("wsl")
            .args(["--list", "--verbose"])
            .output()
            .ok();
        match output {
            Some(o) => String::from_utf8_lossy(&o.stdout).contains(name),
            None => false,
        }
    }

    fn distro_running(name: &str) -> bool {
        let output = Command::new("wsl")
            .args(["--list", "--running"])
            .output()
            .ok();
        match output {
            Some(o) => String::from_utf8_lossy(&o.stdout).contains(name),
            None => false,
        }
    }
}

impl Default for WindowsDriver {
    fn default() -> Self {
        Self::new()
    }
}

/// Per-instance WSL distro name: `deputyos-<id>` for console-created instances,
/// or the bare `deputyos` for the single-instance CLI path (no slug).
fn distro_name(cfg: &InstanceConfig) -> String {
    match cfg.instance_slug() {
        Some(slug) => format!("deputyos-{slug}"),
        None => DISTRO_NAME.to_string(),
    }
}

/// Per-instance rootfs location: `<runtime_dir>/wsl-rootfs`. Each instance
/// has its own `runtime_dir` so two imports never share a rootfs dir.
fn rootfs_dir(cfg: &InstanceConfig) -> PathBuf {
    cfg.runtime_dir.join("wsl-rootfs")
}

/// `wsl --import <name> <rootfs> <image> --version 2` — pure arg builder,
/// extracted so the per-instance import path is unit-testable without
/// spawning `wsl`.
fn import_argv(name: &str, rootfs: &Path, image: &Path) -> Vec<String> {
    vec![
        "--import".into(),
        name.to_string(),
        rootfs.display().to_string(),
        image.display().to_string(),
        "--version".into(),
        "2".into(),
    ]
}

/// `wsl -d <name> -- echo deputyOS started` — pure arg builder for the boot
/// probe (starting a WSL distro runs systemd via the distro's default user
/// init; we use a trivial `echo` to force the distro VM to spin up).
fn start_argv(name: &str) -> Vec<String> {
    vec![
        "-d".into(),
        name.to_string(),
        "--".into(),
        "echo".into(),
        "deputyOS started".into(),
    ]
}

/// `wsl --terminate <name>` — pure arg builder.
fn terminate_argv(name: &str) -> Vec<String> {
    vec!["--terminate".into(), name.to_string()]
}

/// Typed resident-agent invocation. The command portion is derived exclusively
/// from [`deputyd::AgentCommand`], never from user-provided shell text.
fn guest_agent_argv(name: &str, command: &deputyd::AgentCommand) -> Vec<String> {
    let mut args = vec![
        "-d".into(),
        name.to_string(),
        "--".into(),
        "/usr/local/bin/deputyd".into(),
    ];
    match command {
        deputyd::AgentCommand::Health => args.push("health".into()),
        deputyd::AgentCommand::PreparePause => args.push("prepare-pause".into()),
        deputyd::AgentCommand::Resume => args.push("resume".into()),
        deputyd::AgentCommand::Reclaim { drop_caches } => {
            args.push("reclaim".into());
            if *drop_caches {
                args.push("--drop-caches".into());
            }
        }
        other => {
            args.push("execute".into());
            args.push("--request-json".into());
            args.push(serde_json::to_string(other).expect("serializing typed deputyd command"));
        }
    }
    args
}

fn paused_marker(cfg: &InstanceConfig) -> PathBuf {
    cfg.runtime_dir.join("wsl-paused")
}

fn has_paused_marker(cfg: &InstanceConfig) -> bool {
    paused_marker(cfg).is_file()
}

fn set_paused_marker(cfg: &InstanceConfig) -> Result<()> {
    std::fs::create_dir_all(&cfg.runtime_dir)
        .with_context(|| format!("creating {}", cfg.runtime_dir.display()))?;
    std::fs::write(paused_marker(cfg), b"cooperative-wsl-suspend\n")
        .context("recording WSL paused state")
}

fn clear_paused_marker(cfg: &InstanceConfig) {
    match std::fs::remove_file(paused_marker(cfg)) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => eprintln!("warning: could not clear WSL paused marker: {error}"),
    }
}

/// `wsl -d <name> hostname -I` — pure arg builder to read the distro's WSL
/// virtual-NIC IP (the first token of the output). Needed for the portproxy
/// `connectaddress`, since WSL2's own auto-forward only maps guest ports to the
/// *same* `localhost` port number and can't give each distro a distinct host
/// port.
fn wsl_ip_argv(name: &str) -> Vec<String> {
    vec![
        "-d".into(),
        name.to_string(),
        "hostname".into(),
        "-I".into(),
    ]
}

/// `netsh interface portproxy add v4tov4 listenport=<host> listenaddress=127.0.0.1
/// connectport=<guest> connectaddress=<ip>` — pure arg builder. Remaps a
/// Windows-host loopback port to a guest port on the distro's WSL IP, so each
/// console instance gets its own distinct host port instead of every distro
/// contending for `localhost:8088`.
fn portproxy_add_argv(host_port: u16, guest_port: u16, wsl_ip: &str) -> Vec<String> {
    vec![
        "interface".into(),
        "portproxy".into(),
        "add".into(),
        "v4tov4".into(),
        format!("listenport={host_port}"),
        "listenaddress=127.0.0.1".into(),
        format!("connectport={guest_port}"),
        format!("connectaddress={wsl_ip}"),
    ]
}

/// `netsh interface portproxy delete v4tov4 listenport=<host> listenaddress=127.0.0.1`
/// — pure arg builder to tear down a remap on stop.
fn portproxy_delete_argv(host_port: u16) -> Vec<String> {
    vec![
        "interface".into(),
        "portproxy".into(),
        "delete".into(),
        "v4tov4".into(),
        format!("listenport={host_port}"),
        "listenaddress=127.0.0.1".into(),
    ]
}

/// Read the distro's WSL IP via `wsl -d <name> hostname -I` (first token).
/// Returns `None` if the command fails or yields no address.
fn distro_ip(name: &str) -> Option<String> {
    let out = Command::new("wsl").args(wsl_ip_argv(name)).output().ok()?;
    if !out.status.success() {
        return None;
    }
    String::from_utf8_lossy(&out.stdout)
        .split_whitespace()
        .next()
        .map(|s| s.to_string())
}

/// Best-effort per-instance host-port remap after a distro boots: map the
/// instance's `wizard_port`/`gateway_port` on Windows `127.0.0.1` to the guest
/// `:8088`/`:8080` on the distro's WSL IP. Skipped for the single-instance path
/// (ports == the guest defaults, so WSL2's own forward already suffices) and on
/// the default port pair. Emits a warning (does not fail the start) if the remap
/// can't be established — the instance still boots, just only reachable on the
/// default `localhost:8088` like before.
fn apply_port_remap(name: &str, cfg: &InstanceConfig) {
    // Nothing to remap when this instance uses the guest-default ports.
    if cfg.wizard_port == 8088 && cfg.gateway_port == 8080 {
        return;
    }
    let Some(ip) = distro_ip(name) else {
        eprintln!(
            "warning: could not read WSL IP for '{name}'; per-instance port \
             remap skipped — reachable only on the default localhost:8088."
        );
        return;
    };
    for (host, guest) in [(cfg.wizard_port, 8088u16), (cfg.gateway_port, 8080u16)] {
        // Delete any stale mapping first (idempotent), then add.
        let _ = Command::new("netsh")
            .args(portproxy_delete_argv(host))
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status();
        let ok = Command::new("netsh")
            .args(portproxy_add_argv(host, guest, &ip))
            .status()
            .map(|s| s.success())
            .unwrap_or(false);
        if !ok {
            eprintln!(
                "warning: netsh portproxy add for 127.0.0.1:{host}->{ip}:{guest} \
                 failed (needs an elevated shell?); instance '{name}' may only be \
                 reachable on localhost:{guest}."
            );
        }
    }
}

/// Tear down the per-instance port remap on stop (best-effort, idempotent).
fn clear_port_remap(cfg: &InstanceConfig) {
    if cfg.wizard_port == 8088 && cfg.gateway_port == 8080 {
        return;
    }
    for host in [cfg.wizard_port, cfg.gateway_port] {
        let _ = Command::new("netsh")
            .args(portproxy_delete_argv(host))
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status();
    }
}

impl Driver for WindowsDriver {
    fn capabilities(&self) -> DriverCapabilities {
        DriverCapabilities {
            pause_resume: true,
            // WSL dynamically returns memory for its shared utility VM, but
            // does not expose a safe per-distro live target.
            memory_balloon: false,
            guest_agent: true,
            checkpoint: false,
            per_instance_resources: false,
        }
    }

    fn check_prereq(&self) -> Result<()> {
        if !Self::is_wsl_installed() {
            bail!(
                "WSL2 is not installed. Run this in PowerShell (admin):\n  \
                 wsl --install -d Ubuntu\n  \
                 Then reboot and run this launcher again."
            );
        }
        // Verify WSL2 (not WSL1).
        let output = Command::new("wsl").args(["--status"]).output();
        if let Ok(o) = output {
            let stdout = String::from_utf8_lossy(&o.stdout);
            if !stdout.contains("WSL version: 2") && !stdout.contains("WSL 2") {
                bail!(
                    "WSL1 detected. deputyOS requires WSL2 for cgroups + systemd support.\n  \
                     Upgrade with: wsl --set-default-version 2"
                );
            }
        }
        Ok(())
    }

    fn target_for_host(&self) -> &'static str {
        "wsl2"
    }

    fn install_image(&self, image_path: &Path) -> Result<()> {
        if !image_path.is_file() {
            bail!("image not found: {}", image_path.display());
        }

        if Self::distro_exists(DISTRO_NAME) {
            println!("WSL distro '{DISTRO_NAME}' already registered — skipping import.");
            return Ok(());
        }

        let data_dir = config::data_dir();
        let rootfs_dir = data_dir.join("wsl-rootfs");
        std::fs::create_dir_all(&rootfs_dir).ok();

        println!(
            "importing WSL distro '{DISTRO_NAME}' from {} ...",
            image_path.display()
        );
        let status = Command::new("wsl")
            .args(import_argv(DISTRO_NAME, &rootfs_dir, image_path))
            .status();

        match status {
            Ok(s) if s.success() => {
                println!("WSL distro '{DISTRO_NAME}' imported.");
                Ok(())
            }
            Ok(s) => {
                bail!("wsl --import exited with status {}", s.code().unwrap_or(-1));
            }
            Err(e) => {
                bail!("failed to run wsl --import: {e}");
            }
        }
    }

    /// Per-instance import: register `deputyos-<id>` into the instance's own
    /// rootfs dir. Idempotent (skips if the named distro already exists).
    fn install_image_with(&self, image_path: &Path, cfg: &InstanceConfig) -> Result<()> {
        if !image_path.is_file() {
            bail!("image not found: {}", image_path.display());
        }
        let name = distro_name(cfg);
        if Self::distro_exists(&name) {
            println!("WSL distro '{name}' already registered — skipping import.");
            return Ok(());
        }
        let rootfs = rootfs_dir(cfg);
        if let Some(parent) = rootfs.parent() {
            std::fs::create_dir_all(parent).ok();
        }
        println!(
            "importing WSL distro '{name}' from {} ...",
            image_path.display()
        );
        let status = Command::new("wsl")
            .args(import_argv(&name, &rootfs, image_path))
            .status();
        match status {
            Ok(s) if s.success() => {
                println!("WSL distro '{name}' imported.");
                Ok(())
            }
            Ok(s) => bail!("wsl --import exited with status {}", s.code().unwrap_or(-1)),
            Err(e) => bail!("failed to run wsl --import: {e}"),
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
        let name = distro_name(cfg);
        if !Self::distro_exists(&name) {
            bail!("WSL distro '{name}' not found — run install for this instance first.");
        }
        if has_paused_marker(cfg) {
            return self.resume_with(cfg);
        }
        if Self::distro_running(&name) {
            println!("WSL distro '{name}' is already running.");
            return Ok(VmHandle {
                id: format!("{name}-wsl"),
            });
        }
        println!("starting WSL distro '{name}'...");
        let status = Command::new("wsl")
            .args(start_argv(&name))
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status();
        match status {
            Ok(s) if s.success() => {
                // Give this instance its own host port (WSL2's own forward only
                // maps guest->same-numbered localhost, so distinct instances
                // would otherwise collide on localhost:8088). Best-effort.
                apply_port_remap(&name, cfg);
                println!(
                    "WSL distro '{name}' started. Wizard at {}",
                    cfg.wizard_url()
                );
                Ok(VmHandle {
                    id: format!("{name}-wsl"),
                })
            }
            Ok(s) => bail!("wsl start exited with status {}", s.code().unwrap_or(-1)),
            Err(e) => bail!("failed to run wsl: {e}"),
        }
    }

    /// Per-instance stop: `wsl --terminate deputyos-<id>`. Idempotent.
    fn stop_with(&self, cfg: &InstanceConfig) -> Result<()> {
        let name = distro_name(cfg);
        // Tear down the port remap regardless of running state (idempotent).
        clear_port_remap(cfg);
        clear_paused_marker(cfg);
        if !Self::distro_exists(&name) || !Self::distro_running(&name) {
            return Ok(());
        }
        println!("terminating WSL distro '{name}'...");
        let status = Command::new("wsl").args(terminate_argv(&name)).status();
        match status {
            Ok(s) if s.success() => Ok(()),
            Ok(s) => bail!(
                "wsl --terminate exited with status {}",
                s.code().unwrap_or(-1)
            ),
            Err(e) => bail!("failed to run wsl --terminate: {e}"),
        }
    }

    /// Per-instance status: liveness of `deputyos-<id>`. The surfaced URL
    /// reflects the instance's allocated wizard port (see the module-level
    /// caveat about WSL2's same-number localhost forwarding).
    fn status_with(&self, cfg: &InstanceConfig) -> Result<VmStatus> {
        let name = distro_name(cfg);
        if Self::distro_running(&name) {
            Ok(VmStatus::Running {
                handle: VmHandle {
                    id: format!("{name}-wsl"),
                },
                urls: vec![cfg.wizard_url()],
            })
        } else if has_paused_marker(cfg) {
            Ok(VmStatus::Paused {
                handle: VmHandle {
                    id: format!("{name}-wsl"),
                },
            })
        } else {
            Ok(VmStatus::Stopped)
        }
    }

    /// WSL has no per-distro hypervisor pause operation. Because deputyOS
    /// controls the image, we use a cooperative suspend: quiesce and reclaim
    /// through deputyd, terminate the distro, and retain a host-side paused
    /// marker so resume is distinct from a normal start.
    fn pause_with(&self, cfg: &InstanceConfig) -> Result<()> {
        let name = distro_name(cfg);
        if has_paused_marker(cfg) && !Self::distro_running(&name) {
            return Ok(());
        }
        if !Self::distro_running(&name) {
            bail!("WSL distro '{name}' is not running");
        }

        if self
            .guest_agent_with(cfg, deputyd::AgentCommand::PreparePause)
            .is_ok()
        {
            let _ =
                self.guest_agent_with(cfg, deputyd::AgentCommand::Reclaim { drop_caches: true });
        } else {
            // Community images have no resident overlay. Flush the distro
            // before WSL's terminate-as-suspend fallback.
            let _ = Command::new("wsl")
                .args(["-d", &name, "-u", "root", "--", "sync"])
                .status();
        }

        clear_port_remap(cfg);
        let status = Command::new("wsl").args(terminate_argv(&name)).status();
        match status {
            Ok(status) if status.success() => set_paused_marker(cfg),
            Ok(status) => {
                let _ = self.guest_agent_with(cfg, deputyd::AgentCommand::Resume);
                bail!(
                    "wsl --terminate exited with status {}",
                    status.code().unwrap_or(-1)
                )
            }
            Err(error) => {
                let _ = self.guest_agent_with(cfg, deputyd::AgentCommand::Resume);
                bail!("failed to run wsl --terminate: {error}")
            }
        }
    }

    fn resume_with(&self, cfg: &InstanceConfig) -> Result<VmHandle> {
        let name = distro_name(cfg);
        if Self::distro_running(&name) {
            let _ = self.guest_agent_with(cfg, deputyd::AgentCommand::Resume);
            clear_paused_marker(cfg);
            apply_port_remap(&name, cfg);
            return Ok(VmHandle {
                id: format!("{name}-wsl"),
            });
        }
        if !has_paused_marker(cfg) {
            bail!("WSL distro '{name}' is stopped, not paused");
        }

        let status = Command::new("wsl")
            .args(start_argv(&name))
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status();
        match status {
            Ok(status) if status.success() => {}
            Ok(status) => bail!(
                "wsl start exited with status {}",
                status.code().unwrap_or(-1)
            ),
            Err(error) => bail!("failed to run wsl: {error}"),
        }
        let _ = self.guest_agent_with(cfg, deputyd::AgentCommand::Resume);
        clear_paused_marker(cfg);
        apply_port_remap(&name, cfg);
        Ok(VmHandle {
            id: format!("{name}-wsl"),
        })
    }

    fn set_memory_with(&self, _cfg: &InstanceConfig, _target_mib: u64) -> Result<()> {
        bail!(
            "WSL2 memory is ballooned for the shared utility VM, not per distro; \
             use deputyOS reclaim/pause or configure global WSL memory in .wslconfig"
        )
    }

    fn guest_agent_with(
        &self,
        cfg: &InstanceConfig,
        command: deputyd::AgentCommand,
    ) -> Result<deputyd::AgentResult> {
        let name = distro_name(cfg);
        if !Self::distro_running(&name) {
            bail!("WSL distro '{name}' is not running");
        }
        let output = Command::new("wsl")
            .args(guest_agent_argv(&name, &command))
            .output()
            .with_context(|| format!("executing resident agent in WSL distro '{name}'"))?;
        if !output.status.success() {
            bail!(
                "resident agent exited {}: {}",
                output.status.code().unwrap_or(-1),
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
    fn distro_name_uses_instance_slug_when_present() {
        let cfg = cfg_for_slug(Some("abc-123"));
        assert_eq!(distro_name(&cfg), "deputyos-abc-123");
    }

    #[test]
    fn distro_name_falls_back_to_bare_for_single_instance() {
        let cfg = cfg_for_slug(None);
        assert_eq!(distro_name(&cfg), DISTRO_NAME);
    }

    #[test]
    fn rootfs_dir_is_under_runtime_dir() {
        let cfg = cfg_for_slug(Some("abc-123"));
        assert_eq!(rootfs_dir(&cfg), PathBuf::from("/data/run/wsl-rootfs"));
    }

    #[test]
    fn import_argv_shape() {
        let args = import_argv(
            "deputyos-abc-123",
            &PathBuf::from("/data/run/wsl-rootfs"),
            &PathBuf::from("/cache/deputyos-wsl2.tar.gz"),
        );
        assert_eq!(
            args,
            vec![
                "--import",
                "deputyos-abc-123",
                "/data/run/wsl-rootfs",
                "/cache/deputyos-wsl2.tar.gz",
                "--version",
                "2",
            ]
        );
    }

    #[test]
    fn start_argv_targets_named_distro() {
        let args = start_argv("deputyos-abc-123");
        assert_eq!(
            args,
            vec!["-d", "deputyos-abc-123", "--", "echo", "deputyOS started"]
        );
    }

    #[test]
    fn terminate_argv_targets_named_distro() {
        let args = terminate_argv("deputyos-abc-123");
        assert_eq!(args, vec!["--terminate", "deputyos-abc-123"]);
    }

    #[test]
    fn guest_agent_argv_is_typed_and_allow_listed() {
        assert_eq!(
            guest_agent_argv(
                "deputyos-abc-123",
                &deputyd::AgentCommand::Reclaim { drop_caches: true }
            ),
            vec![
                "-d",
                "deputyos-abc-123",
                "--",
                "/usr/local/bin/deputyd",
                "reclaim",
                "--drop-caches",
            ]
        );
    }

    #[test]
    fn paused_marker_is_scoped_to_instance_runtime_dir() {
        let cfg = cfg_for_slug(Some("abc-123"));
        assert_eq!(paused_marker(&cfg), PathBuf::from("/data/run/wsl-paused"));
    }

    #[test]
    fn wsl_ip_argv_shape() {
        assert_eq!(
            wsl_ip_argv("deputyos-abc-123"),
            vec!["-d", "deputyos-abc-123", "hostname", "-I"]
        );
    }

    #[test]
    fn portproxy_add_argv_maps_host_to_guest_on_wsl_ip() {
        let args = portproxy_add_argv(17088, 8088, "172.20.1.5");
        assert_eq!(
            args,
            vec![
                "interface",
                "portproxy",
                "add",
                "v4tov4",
                "listenport=17088",
                "listenaddress=127.0.0.1",
                "connectport=8088",
                "connectaddress=172.20.1.5",
            ]
        );
    }

    #[test]
    fn portproxy_delete_argv_targets_host_listenport() {
        let args = portproxy_delete_argv(17080);
        assert_eq!(
            args,
            vec![
                "interface",
                "portproxy",
                "delete",
                "v4tov4",
                "listenport=17080",
                "listenaddress=127.0.0.1",
            ]
        );
    }
}
