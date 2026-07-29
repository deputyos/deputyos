//! Linux driver — qemu-system + KVM.
//!
//! Spawns qemu directly with KVM acceleration and `hostfwd` rules that map
//! the VM's :8088 (wizard) and :8080 (chat/relay) onto the host's loopback.
//! No bundled qemu — the launcher mandates `qemu-system-x86_64` (or
//! `qemu-system-aarch64` on aarch64 hosts) is on PATH and prints a
//! distro-aware install hint if it isn't.
//!
//! ## Lifecycle
//!
//! - `start` writes the qemu PID to `<runtime>/deputyos-desktop.pid`.
//! - `status` reads the PID and uses `kill -0` to check liveness; if the
//!   process is gone we treat the VM as stopped (and remove the stale
//!   PID file).
//! - `stop` reads the PID and sends SIGTERM. We do NOT wait for shutdown —
//!   the qemu monitor may take 10s to drain disk caches; the user can run
//!   `status` to confirm.
//!
//! ## Why no rustix process-signaling
//!
//! `rustix::process::kill_process_group` exists but its signal-targeting
//! semantics differ subtly between Linux flavours, and we want a
//! kill-by-PID with SIGTERM. The simplest correct path is `libc::kill`,
//! which is exposed via std on Unix without an extra dep — but std doesn't
//! expose it. We shell out to `/bin/kill -TERM <pid>`, which is universal,
//! POSIX, and zero-dep. (`unsafe_code = "forbid"` workspace lint blocks
//! direct libc calls anyway.)

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use anyhow::{anyhow, bail, Context, Result};

use crate::driver::{Driver, DriverCapabilities, VmHandle, VmStatus};
use crate::drivers::qemu_control::{guest_command, QmpClient};
use crate::instance::{InstanceConfig, ResourceSpec, PID_FILENAME};

const QMP_FILENAME: &str = "qmp.sock";
const QGA_FILENAME: &str = "qga.sock";

/// File where the qemu PID is staged for `status` / `stop` to find, for the
/// given instance config. Each instance has its own `runtime_dir` so the
/// constant filename never collides between two VMs.
fn pid_file_for(cfg: &InstanceConfig) -> PathBuf {
    cfg.runtime_dir.join(PID_FILENAME)
}

fn qmp_socket_for(cfg: &InstanceConfig) -> PathBuf {
    cfg.runtime_dir.join(QMP_FILENAME)
}

fn qga_socket_for(cfg: &InstanceConfig) -> PathBuf {
    cfg.runtime_dir.join(QGA_FILENAME)
}

/// Linux driver instance. Stateless — every call resolves runtime state
/// from the PID file on disk.
pub struct LinuxDriver;

impl LinuxDriver {
    pub fn new() -> Self {
        Self
    }

    /// Choose `qemu-system-x86_64` or `qemu-system-aarch64` based on host
    /// CPU. Hard-coded on `std::env::consts::ARCH` — same logic as
    /// `target_for_host`.
    fn qemu_binary() -> &'static str {
        match std::env::consts::ARCH {
            "x86_64" => "qemu-system-x86_64",
            "aarch64" => "qemu-system-aarch64",
            // Fall through — we'll error in `check_prereq` with the host arch.
            _ => "qemu-system-x86_64",
        }
    }

    /// Resolve the qemu binary path, honouring `PATH` env. Returns Err if
    /// the binary isn't on PATH (the prereq check uses this).
    fn resolve_qemu() -> Result<PathBuf> {
        which_on_path(Self::qemu_binary()).ok_or_else(|| {
            anyhow!(
                "{} not found on PATH. \
                 Install with: sudo apt install qemu-system-x86 cpu-checker  (Debian/Ubuntu) \
                 — or: sudo dnf install qemu-system-x86  (Fedora) \
                 — or: sudo pacman -S qemu-full  (Arch)",
                Self::qemu_binary()
            )
        })
    }
}

impl Default for LinuxDriver {
    fn default() -> Self {
        Self::new()
    }
}

impl Driver for LinuxDriver {
    fn capabilities(&self) -> DriverCapabilities {
        DriverCapabilities {
            pause_resume: true,
            memory_balloon: true,
            guest_agent: true,
            checkpoint: false,
            per_instance_resources: true,
        }
    }

    fn check_prereq(&self) -> Result<()> {
        // 1. qemu-system-* on PATH.
        let _ = Self::resolve_qemu()?;
        // 2. KVM device — soft warn (qemu falls back to TCG) but don't fail.
        //    A no-KVM host is dramatically slower but still functional.
        if !Path::new("/dev/kvm").exists() {
            eprintln!(
                "warn: /dev/kvm not present; VM will use TCG (slow). \
                 If you have hardware virtualization, install kvm and reboot."
            );
        }
        Ok(())
    }

    fn target_for_host(&self) -> &'static str {
        match std::env::consts::ARCH {
            "x86_64" => "qemu-x86_64",
            "aarch64" => "qemu-aarch64",
            // Unsupported arch — manifest layer will emit a clear error.
            _ => "qemu-x86_64",
        }
    }

    fn install_image(&self, image_path: &Path) -> Result<()> {
        // qemu reads the image directly at start time — no copy/import step.
        if !image_path.is_file() {
            bail!("image not found at {}", image_path.display());
        }
        eprintln!("==> linux driver: image staged at {}", image_path.display());
        Ok(())
    }

    // Single-instance CLI path: delegate to the `*_with` forms using the
    // env-derived config (reads the same `config::*` getters as before, so
    // `deputyos-desktop start/stop/status` behaviour is unchanged).
    fn start(&self) -> Result<VmHandle> {
        self.start_with(&InstanceConfig::from_env())
    }
    fn stop(&self) -> Result<()> {
        self.stop_with(&InstanceConfig::from_env())
    }
    fn status(&self) -> Result<VmStatus> {
        self.status_with(&InstanceConfig::from_env())
    }

    fn start_with(&self, cfg: &InstanceConfig) -> Result<VmHandle> {
        cfg.resources.validate()?;
        // Idempotent — if a VM is already running, return the existing handle.
        if let VmStatus::Running { handle, .. } = self.status_with(cfg)? {
            eprintln!("==> linux driver: VM already running (pid {})", handle.id);
            return Ok(handle);
        }

        let qemu = Self::resolve_qemu()?;

        // We need an image path to boot — the same path install_image was
        // called with. The contract is "the image lives at
        // <cache>/deputyos-<target>.qcow2" — written by main.rs after
        // download_and_verify. For a multi-instance console the cache_dir is
        // per-instance, so each VM has its own qcow2.
        let image = cfg.cache_dir.join(format!(
            "deputyos-{}.qcow2",
            <Self as Driver>::target_for_host(self)
        ));
        if !image.is_file() {
            bail!(
                "no installed image found at {}; run `deputyos-desktop install` first",
                image.display()
            );
        }

        let pid_path = pid_file_for(cfg);
        if let Some(parent) = pid_path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("creating runtime dir {}", parent.display()))?;
        }
        let qmp_socket = qmp_socket_for(cfg);
        let qga_socket = qga_socket_for(cfg);
        // QEMU refuses to bind over stale Unix sockets left by a hard crash.
        for socket in [&qmp_socket, &qga_socket] {
            match std::fs::remove_file(socket) {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => {
                    return Err(error)
                        .with_context(|| format!("removing stale {}", socket.display()))
                }
            }
        }

        // Optional cloud-init seed: a per-instance NoCloud cidata ISO the
        // guest's first-boot picks up (the local dev loop uses this to point
        // the in-VM agent at the local API). `None` → no seed → production
        // path. The single-instance CLI gets its seed from
        // DEPUTYOS_DESKTOP_SEED_ISO via `InstanceConfig::from_env`.
        let seed_iso = match &cfg.seed_iso {
            Some(p) => {
                if !p.is_file() {
                    bail!(
                        "seed ISO points at a missing file {}; \
                         unset it or regenerate the seed",
                        p.display()
                    );
                }
                Some(p.clone())
            }
            None => None,
        };
        let mut cmd = Command::new(&qemu);
        cmd.args(qemu_argv(
            &image,
            seed_iso.as_deref(),
            cfg.wizard_port,
            cfg.gateway_port,
            cfg.resources,
            &qmp_socket,
            &qga_socket,
        )?);
        // Capture qemu's serial console (the guest's kernel + cloud-init +
        // deputywizard boot output, since `-serial mon:stdio` routes serial to
        // qemu's stdout) and qemu's own warnings (host port redirect failures,
        // missing KVM, etc. on stderr) to a log file under the instance's
        // cache dir. stdin stays null (the launcher has no monitor input);
        // qemu tolerates a null stdin with `mon:stdio` without quitting, so
        // the VM still outlives us.
        let log_path = cfg.cache_dir.join("qemu.log");
        if let Some(parent) = log_path.parent() {
            std::fs::create_dir_all(parent).ok();
        }
        let log_file = std::fs::OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(&log_path)
            .with_context(|| format!("opening qemu log {}", log_path.display()))?;
        let log_file_err = log_file
            .try_clone()
            .with_context(|| format!("cloning qemu log {}", log_path.display()))?;
        cmd.stdin(Stdio::null())
            .stdout(Stdio::from(log_file))
            .stderr(Stdio::from(log_file_err));

        eprintln!("==> linux driver: spawning {}", qemu.display());
        eprintln!("==> linux driver: qemu console log: {}", log_path.display());
        let child = cmd
            .spawn()
            .with_context(|| format!("spawning {}", qemu.display()))?;
        let pid = child.id();
        // Write PID to the runtime file. We don't keep the Child around —
        // it's a long-lived process and the launcher exits after `start`.
        std::fs::write(&pid_path, pid.to_string())
            .with_context(|| format!("writing pid file {}", pid_path.display()))?;
        std::mem::forget(child); // Don't reap on drop — we want it to outlive us.

        eprintln!("==> linux driver: VM started (pid {pid})");
        Ok(VmHandle {
            id: pid.to_string(),
        })
    }

    fn stop_with(&self, cfg: &InstanceConfig) -> Result<()> {
        let pid_path = pid_file_for(cfg);
        let raw = match std::fs::read_to_string(&pid_path) {
            Ok(s) => s,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                eprintln!("==> linux driver: no PID file; nothing to stop");
                return Ok(());
            }
            Err(e) => return Err(anyhow!("reading pid file {}: {e}", pid_path.display())),
        };
        let pid: u32 = raw
            .trim()
            .parse()
            .with_context(|| format!("parsing pid file {}", pid_path.display()))?;

        // SIGTERM via /bin/kill — POSIX, zero-dep, and respects the
        // workspace `unsafe_code = "forbid"` lint that blocks libc::kill.
        let status = Command::new("kill")
            .arg("-TERM")
            .arg(pid.to_string())
            .stderr(Stdio::piped())
            .output()
            .with_context(|| format!("running kill -TERM {pid}"))?;
        if !status.status.success() {
            // ESRCH ("no such process") is fine — process already exited.
            let stderr = String::from_utf8_lossy(&status.stderr);
            if stderr.contains("No such process") || stderr.contains("no such process") {
                eprintln!("==> linux driver: pid {pid} already gone");
            } else {
                bail!("kill -TERM {pid} failed: {}", stderr.trim());
            }
        } else {
            eprintln!("==> linux driver: SIGTERM sent to pid {pid}");
        }

        // Best-effort cleanup of the PID file. If the VM is shutting down
        // we want a subsequent `status` to report "Stopped" cleanly.
        let _ = std::fs::remove_file(&pid_path);
        Ok(())
    }

    fn status_with(&self, cfg: &InstanceConfig) -> Result<VmStatus> {
        let pid_path = pid_file_for(cfg);
        let raw = match std::fs::read_to_string(&pid_path) {
            Ok(s) => s,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(VmStatus::Stopped),
            Err(e) => return Err(anyhow!("reading pid file: {e}")),
        };
        let pid: u32 = match raw.trim().parse() {
            Ok(p) => p,
            Err(_) => {
                // Stale/corrupt — treat as stopped.
                let _ = std::fs::remove_file(&pid_path);
                return Ok(VmStatus::Stopped);
            }
        };
        // `kill -0 <pid>` — true if process exists and we have permission to
        // signal it. This is the canonical liveness probe.
        let alive = Command::new("kill")
            .arg("-0")
            .arg(pid.to_string())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false);
        if alive {
            if let Ok(mut qmp) = QmpClient::connect(&qmp_socket_for(cfg)) {
                if matches!(qmp.status().as_deref(), Ok("paused")) {
                    return Ok(VmStatus::Paused {
                        handle: VmHandle {
                            id: pid.to_string(),
                        },
                    });
                }
            }
            Ok(VmStatus::Running {
                handle: VmHandle {
                    id: pid.to_string(),
                },
                urls: vec![cfg.wizard_url()],
            })
        } else {
            // Stale PID file — clean up and report stopped.
            let _ = std::fs::remove_file(&pid_path);
            Ok(VmStatus::Stopped)
        }
    }

    fn pause_with(&self, cfg: &InstanceConfig) -> Result<()> {
        match self.status_with(cfg)? {
            VmStatus::Paused { .. } => return Ok(()),
            VmStatus::Stopped => bail!("cannot pause a stopped instance"),
            VmStatus::Running { .. } => {}
        }

        // Community images intentionally have no deputyOS resident service.
        // Managed images add cooperative quiesce/reclaim, but QMP pause and
        // ballooning remain valid host capabilities without that overlay.
        if self
            .guest_agent_with(cfg, deputyd::AgentCommand::PreparePause)
            .is_ok()
        {
            let _ =
                self.guest_agent_with(cfg, deputyd::AgentCommand::Reclaim { drop_caches: true });
        }

        let mut qmp = QmpClient::connect(&qmp_socket_for(cfg))?;
        if cfg.resources.auto_balloon {
            qmp.set_balloon_mib(cfg.resources.memory_min_mib)
                .context("ballooning paused instance to its minimum")?;
        }
        qmp.pause().context("pausing virtual CPUs")
    }

    fn resume_with(&self, cfg: &InstanceConfig) -> Result<VmHandle> {
        let handle = match self.status_with(cfg)? {
            VmStatus::Running { handle, .. } => return Ok(handle),
            VmStatus::Stopped => bail!("cannot resume a stopped instance"),
            VmStatus::Paused { handle } => handle,
        };
        let mut qmp = QmpClient::connect(&qmp_socket_for(cfg))?;
        qmp.resume().context("resuming virtual CPUs")?;
        if cfg.resources.auto_balloon {
            qmp.set_balloon_mib(cfg.resources.memory_max_mib)
                .context("restoring instance memory envelope")?;
        }
        let _ = self.guest_agent_with(cfg, deputyd::AgentCommand::Resume);
        Ok(handle)
    }

    fn set_memory_with(&self, cfg: &InstanceConfig, target_mib: u64) -> Result<()> {
        if target_mib < cfg.resources.memory_min_mib || target_mib > cfg.resources.memory_max_mib {
            bail!(
                "memory target {target_mib} MiB is outside this instance's {}-{} MiB envelope",
                cfg.resources.memory_min_mib,
                cfg.resources.memory_max_mib
            );
        }
        match self.status_with(cfg)? {
            VmStatus::Stopped => bail!("cannot balloon a stopped instance"),
            VmStatus::Paused { .. } => {
                bail!("resume the instance before changing its live memory target")
            }
            VmStatus::Running { .. } => {}
        }
        let _ = self.guest_agent_with(cfg, deputyd::AgentCommand::Reclaim { drop_caches: false });
        QmpClient::connect(&qmp_socket_for(cfg))?
            .set_balloon_mib(target_mib)
            .context("setting QEMU balloon target")
    }

    fn guest_agent_with(
        &self,
        cfg: &InstanceConfig,
        command: deputyd::AgentCommand,
    ) -> Result<deputyd::AgentResult> {
        guest_command(&qga_socket_for(cfg), command)
    }
}

/// Resolve `name` against `$PATH`. Returns `None` if not found.
fn which_on_path(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    for p in std::env::split_paths(&path) {
        let candidate = p.join(name);
        if candidate.is_file() {
            // is_file is sufficient — even if not exec-bit, qemu spawn
            // will fail loudly with EACCES rather than silently skip.
            return Some(candidate);
        }
    }
    None
}

/// Build the qemu argv (args only — the caller supplies the program via
/// `Command::new(qemu)`) for booting `image`. When `seed_iso` is `Some`,
/// attach it as a second `-drive` (a NoCloud cloud-init cidata ISO) so the
/// guest's first-boot can pick up injected config — the local dev loop uses
/// this to write `DEPUTYOS_API_BASE` into `/etc/deputyos/secrets.env` (see
/// scripts/desktop-local.sh). Extracted from `start()` so the seed-attach
/// path is unit-testable without spawning qemu.
fn qemu_argv(
    image: &Path,
    seed_iso: Option<&Path>,
    wizard_host_port: u16,
    gateway_host_port: u16,
    resources: ResourceSpec,
    qmp_socket: &Path,
    qga_socket: &Path,
) -> Result<Vec<String>> {
    // KVM if the device exists, else TCG (slow but functional).
    let accel = if Path::new("/dev/kvm").exists() {
        "kvm"
    } else {
        "tcg"
    };
    let image_str = image
        .to_str()
        .ok_or_else(|| anyhow!("non-utf8 image path"))?;
    let qmp_str = qmp_socket
        .to_str()
        .ok_or_else(|| anyhow!("non-utf8 QMP socket path"))?;
    let qga_str = qga_socket
        .to_str()
        .ok_or_else(|| anyhow!("non-utf8 QGA socket path"))?;
    let virtio_serial = if std::env::consts::ARCH == "aarch64" {
        "virtio-serial-device"
    } else {
        "virtio-serial-pci"
    };
    let mut argv: Vec<String> = vec![
        "-accel".into(),
        accel.into(),
        "-smp".into(),
        resources.vcpus.to_string(),
        "-m".into(),
        resources.memory_max_mib.to_string(),
        "-nographic".into(),
        "-serial".into(),
        "stdio".into(),
        "-monitor".into(),
        "none".into(),
        "-qmp".into(),
        format!("unix:{qmp_str},server=on,wait=off"),
        "-chardev".into(),
        format!("socket,path={qga_str},server=on,wait=off,id=qga0"),
        "-device".into(),
        virtio_serial.into(),
        "-device".into(),
        "virtserialport,chardev=qga0,name=org.qemu.guest_agent.0".into(),
        "-device".into(),
        "virtio-balloon,id=balloon0".into(),
        "-drive".into(),
        format!("if=virtio,file={image_str},format=qcow2"),
        // Host-side forwards: the guest wizard is on :8088 and the chat/relay
        // on :8080; the HOST ports are configurable (DEPUTYOS_DESKTOP_WIZARD_PORT
        // / DEPUTYOS_DESKTOP_GATEWAY_PORT) so the loop can dodge host collisions
        // by moving to the 7000 series. Defaults 8088/8080 keep production + the
        // other drivers unchanged.
        "-netdev".into(),
        format!(
            "user,id=net0,hostfwd=tcp::{wizard_host_port}-:8088,hostfwd=tcp::{gateway_host_port}-:8080"
        ),
        "-device".into(),
        "virtio-net-pci,netdev=net0".into(),
    ];
    if let Some(seed) = seed_iso {
        let seed_str = seed
            .to_str()
            .ok_or_else(|| anyhow!("non-utf8 seed ISO path"))?;
        argv.push("-drive".into());
        argv.push(format!("file={seed_str},format=raw,if=virtio"));
    }
    Ok(argv)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn target_for_host_known_arch() {
        let d = LinuxDriver::new();
        let t = <LinuxDriver as Driver>::target_for_host(&d);
        assert!(matches!(t, "qemu-x86_64" | "qemu-aarch64"), "got: {t}");
    }

    #[test]
    fn which_on_path_finds_sh() {
        // /bin/sh exists on every Linux + macOS test host.
        let p = which_on_path("sh");
        assert!(p.is_some(), "sh should be on PATH");
    }

    #[test]
    fn which_on_path_misses_garbage() {
        assert!(which_on_path("definitely-not-a-real-binary-xyzzy").is_none());
    }

    #[test]
    fn install_image_rejects_missing_path() {
        let d = LinuxDriver::new();
        let err = d
            .install_image(Path::new("/nonexistent/deputyos.qcow2"))
            .expect_err("missing image should error");
        assert!(format!("{err:#}").contains("not found"));
    }

    #[test]
    fn qemu_argv_attaches_seed_drive_when_provided() {
        let argv = qemu_argv(
            Path::new("/cache/deputyos-qemu-x86_64.qcow2"),
            Some(Path::new("/tmp/seed.iso")),
            8088,
            8080,
            ResourceSpec::default(),
            Path::new("/run/deputyos/qmp.sock"),
            Path::new("/run/deputyos/qga.sock"),
        )
        .expect("argv ok");
        // Image drive is always present.
        assert!(
            argv.iter()
                .any(|a| a.contains("deputyos-qemu-x86_64.qcow2") && a.contains("format=qcow2")),
            "image drive present: {argv:?}"
        );
        // Seed drive is attached as a raw cidata drive.
        assert!(
            argv.iter()
                .any(|a| a.contains("/tmp/seed.iso") && a.contains("format=raw")),
            "seed drive attached: {argv:?}"
        );
    }

    #[test]
    fn qemu_argv_omits_seed_drive_when_none() {
        let argv = qemu_argv(
            Path::new("/cache/deputyos.qemu-x86_64.qcow2"),
            None,
            8088,
            8080,
            ResourceSpec::default(),
            Path::new("/run/deputyos/qmp.sock"),
            Path::new("/run/deputyos/qga.sock"),
        )
        .expect("argv ok");
        assert!(
            !argv.iter().any(|a| a.contains("format=raw")),
            "no raw (seed) drive when seed is None: {argv:?}"
        );
        assert!(
            argv.iter().any(|a| a.contains("format=qcow2")),
            "image drive still present: {argv:?}"
        );
    }

    #[test]
    fn qemu_argv_forwards_configured_host_ports_to_guest_8088_8080() {
        // The local dev loop moves the host-side forwards to the 7000 series
        // (7088/7080) to dodge host port collisions; the guest wizard + relay
        // ports (8088/8080) stay fixed.
        let argv = qemu_argv(
            Path::new("/cache/deputyos-qemu-x86_64.qcow2"),
            None,
            7088,
            7080,
            ResourceSpec::default(),
            Path::new("/run/deputyos/qmp.sock"),
            Path::new("/run/deputyos/qga.sock"),
        )
        .expect("argv ok");
        assert!(
            argv.iter()
                .any(|a| a == "user,id=net0,hostfwd=tcp::7088-:8088,hostfwd=tcp::7080-:8080"),
            "netdev forwards host 7088/7080 to guest 8088/8080: {argv:?}"
        );
    }

    #[test]
    fn qemu_argv_applies_resources_balloon_qmp_and_guest_agent() {
        let resources = ResourceSpec {
            vcpus: 4,
            memory_min_mib: 768,
            memory_max_mib: 6144,
            auto_balloon: true,
        };
        let argv = qemu_argv(
            Path::new("/cache/deputyos-qemu-x86_64.qcow2"),
            None,
            8088,
            8080,
            resources,
            Path::new("/run/instance/qmp.sock"),
            Path::new("/run/instance/qga.sock"),
        )
        .expect("argv");
        assert!(argv.windows(2).any(|pair| pair == ["-smp", "4"]));
        assert!(argv.windows(2).any(|pair| pair == ["-m", "6144"]));
        assert!(argv
            .iter()
            .any(|arg| arg == "unix:/run/instance/qmp.sock,server=on,wait=off"));
        assert!(argv
            .iter()
            .any(|arg| arg == "socket,path=/run/instance/qga.sock,server=on,wait=off,id=qga0"));
        assert!(argv.iter().any(|arg| arg == "virtio-balloon,id=balloon0"));
        assert!(argv
            .iter()
            .any(|arg| arg == "virtserialport,chardev=qga0,name=org.qemu.guest_agent.0"));
    }

    #[test]
    fn status_with_no_pid_file_is_stopped() {
        let _g = crate::config::test_env_lock()
            .lock()
            .expect("test_env_lock poisoned");
        // Stage a clean runtime dir.
        let dir = tempfile::tempdir().expect("tempdir");
        let prev = std::env::var("DEPUTYOS_DESKTOP_RUNTIME_DIR").ok();
        std::env::set_var(
            "DEPUTYOS_DESKTOP_RUNTIME_DIR",
            dir.path().to_str().expect("utf8"),
        );
        let d = LinuxDriver::new();
        let s = d.status().expect("status ok");
        assert!(matches!(s, VmStatus::Stopped), "expected Stopped");
        match prev {
            Some(v) => std::env::set_var("DEPUTYOS_DESKTOP_RUNTIME_DIR", v),
            None => std::env::remove_var("DEPUTYOS_DESKTOP_RUNTIME_DIR"),
        }
    }
}
