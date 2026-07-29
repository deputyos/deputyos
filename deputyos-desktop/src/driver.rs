//! `Driver` trait — the per-platform VM lifecycle interface.
//!
//! Each host OS has a different best-in-class virtualization story:
//!
//! - **Linux**: qemu-system + KVM (universal, apt/dnf/pacman).
//! - **Windows**: WSL2 (Microsoft's `wsl --install` is one PowerShell command
//!   on Win10 21H2+ / Win11).
//! - **macOS**: UTM (free, App Store, Vz.framework on Apple Silicon).
//!
//! The launcher binary is per-platform — we cross-compile one binary per
//! target triple. So at compile time the launcher only ever sees its own
//! host's driver; the others are gated out via `#[cfg(target_os)]`.
//!
//! [`current_driver`] returns the right driver for the host. If the host
//! is not one of the three supported platforms, it returns a stub that
//! errors helpfully on every method.

use std::path::Path;

use anyhow::{bail, Result};
use serde::{Deserialize, Serialize};

/// Opaque handle to a started VM. The Linux driver wraps a qemu PID;
/// Windows + macOS may wrap a `wsl distro name` or `utm uuid` respectively.
#[derive(Debug, Clone, Serialize)]
pub struct VmHandle {
    /// Free-form id for diagnostics. On Linux this is the qemu PID as a string.
    pub id: String,
}

/// Current liveness of the VM. Serializes for the Tauri console as
/// `{"Running": {handle, urls}}` or the string `"Stopped"` (serde's default
/// enum representation — the console frontend reads exactly that shape).
#[derive(Debug, Clone, Serialize)]
pub enum VmStatus {
    /// VM is running. `urls` lists endpoints the user can hit (typically
    /// `http://localhost:8088` for the wizard).
    Running { handle: VmHandle, urls: Vec<String> },
    /// VM/distro is intentionally paused. The handle remains stable so it can
    /// be resumed without creating a new instance.
    Paused { handle: VmHandle },
    /// VM not running. Either never started or `stop`'d.
    Stopped,
}

/// Backend features surfaced to the CLI/UI so unsupported controls are hidden
/// rather than failing after the user clicks them.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct DriverCapabilities {
    pub pause_resume: bool,
    pub memory_balloon: bool,
    pub guest_agent: bool,
    pub checkpoint: bool,
    pub per_instance_resources: bool,
}

/// Per-platform VM lifecycle.
///
/// All methods are blocking. Callers compose them as: `check_prereq → install
/// → start → status → stop`. `target_for_host` is consulted by the manifest
/// layer to decide which artefact to download.
///
/// `Send + Sync` is required so a `Box<dyn Driver>` can live in the Tauri
/// console's shared `State` (Tauri needs `State<T>: Send + Sync`). Every driver
/// impl is a zero-field unit struct, so this is free.
pub trait Driver: Send + Sync {
    fn capabilities(&self) -> DriverCapabilities {
        DriverCapabilities::default()
    }

    /// Verify the platform prerequisite (qemu+KVM / WSL2 / UTM) is installed
    /// and ready. On failure: return Err with a user-facing install hint.
    fn check_prereq(&self) -> Result<()>;

    /// The manifest `target` field this host needs. Linux x86_64 →
    /// `qemu-x86_64`, Linux aarch64 → `qemu-aarch64`, Windows → `wsl2`,
    /// macOS → `macos-qemu`.
    fn target_for_host(&self) -> &'static str;

    /// Import / register the cached image with the host's tooling. Idempotent.
    fn install_image(&self, image_path: &Path) -> Result<()>;

    /// Start the VM. Idempotent — a running VM is a no-op (returns the
    /// existing handle).
    fn start(&self) -> Result<VmHandle>;

    /// Graceful shutdown. Idempotent — a stopped VM is a no-op.
    fn stop(&self) -> Result<()>;

    /// Current VM liveness.
    fn status(&self) -> Result<VmStatus>;

    /// Quiesce guest workloads and pause the instance.
    fn pause(&self) -> Result<()> {
        self.pause_with(&crate::instance::InstanceConfig::from_env())
    }

    /// Resume a paused instance and thaw its workloads.
    fn resume(&self) -> Result<VmHandle> {
        self.resume_with(&crate::instance::InstanceConfig::from_env())
    }

    /// Set the guest-visible memory target. Backends validate it against the
    /// instance's configured minimum/maximum envelope.
    fn set_memory(&self, target_mib: u64) -> Result<()> {
        self.set_memory_with(&crate::instance::InstanceConfig::from_env(), target_mib)
    }

    /// Per-instance variants. The desktop console manages multiple named
    /// agents at once; these take an [`InstanceConfig`] (ports + dirs + seed)
    /// so each VM gets its own cache dir, runtime dir, and host port pair
    /// instead of reading the global `config::*` getters. The default impls
    /// delegate to the single-instance `start`/`stop`/`status`/
    /// `install_image` (which ignore `cfg`) — so `UnsupportedDriver` and the
    /// bare single-instance CLI are unchanged. Only drivers that support
    /// multi-instance (Linux, and as of M9.5 Windows/macOS) override them.
    fn start_with(&self, cfg: &crate::instance::InstanceConfig) -> Result<VmHandle> {
        let _ = cfg;
        self.start()
    }
    fn stop_with(&self, cfg: &crate::instance::InstanceConfig) -> Result<()> {
        let _ = cfg;
        self.stop()
    }
    fn status_with(&self, cfg: &crate::instance::InstanceConfig) -> Result<VmStatus> {
        let _ = cfg;
        self.status()
    }
    /// Per-instance image import/register. The default delegates to the
    /// single-instance [`Driver::install_image`] (which ignores `cfg`).
    /// Windows/macOS override this so each instance registers a uniquely-named
    /// WSL distro / UTM VM (keyed by instance id) instead of one shared
    /// `deputyos`. Linux needs no override — its `install_image` just validates
    /// the image path; the per-instance qcow2 is resolved at `start_with`
    /// from `cfg.cache_dir`.
    fn install_image_with(
        &self,
        image_path: &Path,
        cfg: &crate::instance::InstanceConfig,
    ) -> Result<()> {
        let _ = cfg;
        self.install_image(image_path)
    }

    fn pause_with(&self, _cfg: &crate::instance::InstanceConfig) -> Result<()> {
        bail!(
            "pause/resume is not supported by the {} backend",
            self.target_for_host()
        )
    }

    fn resume_with(&self, _cfg: &crate::instance::InstanceConfig) -> Result<VmHandle> {
        bail!(
            "pause/resume is not supported by the {} backend",
            self.target_for_host()
        )
    }

    fn set_memory_with(
        &self,
        _cfg: &crate::instance::InstanceConfig,
        _target_mib: u64,
    ) -> Result<()> {
        bail!(
            "dynamic memory is not supported by the {} backend",
            self.target_for_host()
        )
    }

    /// Execute one typed resident-agent command through the platform's guest
    /// channel. No backend accepts arbitrary shell input through this API.
    fn guest_agent_with(
        &self,
        _cfg: &crate::instance::InstanceConfig,
        _command: deputyd::AgentCommand,
    ) -> Result<deputyd::AgentResult> {
        bail!(
            "resident guest agent is not reachable through the {} backend",
            self.target_for_host()
        )
    }
}

/// Driver for hosts not yet supported. Every method errors with a clear
/// "unsupported host" message. Compiled in only when the host is neither
/// linux/windows/macos.
pub struct UnsupportedDriver {
    pub os: &'static str,
}

impl Driver for UnsupportedDriver {
    fn check_prereq(&self) -> Result<()> {
        bail!(
            "deputyos-desktop has no driver for host OS '{}'. \
             Supported: linux, windows, macos. \
             See docs/11-roadmap.md § M2.5.",
            self.os
        );
    }
    fn target_for_host(&self) -> &'static str {
        "unsupported"
    }
    fn install_image(&self, _image_path: &Path) -> Result<()> {
        self.check_prereq()
    }
    fn start(&self) -> Result<VmHandle> {
        self.check_prereq()?;
        unreachable!("check_prereq always errors on unsupported hosts");
    }
    fn stop(&self) -> Result<()> {
        self.check_prereq()
    }
    fn status(&self) -> Result<VmStatus> {
        self.check_prereq()?;
        Ok(VmStatus::Stopped)
    }
}

/// Pick the right driver for this host at compile time.
///
/// **Honest about per-platform binaries**: the launcher binary you ship to
/// a Mac contains *only* the macOS driver; the Linux binary contains *only*
/// the Linux driver. There's no runtime dispatch — `cfg` gates the foreign
/// drivers out entirely. This keeps the binary tiny and avoids dragging
/// `wsl --import` symbol references into a Linux build.
pub fn current_driver() -> Box<dyn Driver> {
    #[cfg(target_os = "linux")]
    {
        Box::new(crate::drivers::linux::LinuxDriver::new())
    }
    #[cfg(target_os = "windows")]
    {
        Box::new(crate::drivers::windows::WindowsDriver::new())
    }
    #[cfg(target_os = "macos")]
    {
        Box::new(crate::drivers::macos::MacOsDriver::new())
    }
    #[cfg(not(any(target_os = "linux", target_os = "windows", target_os = "macos")))]
    {
        Box::new(UnsupportedDriver {
            os: std::env::consts::OS,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unsupported_driver_errors_helpfully() {
        let d = UnsupportedDriver { os: "haiku" };
        let err = d.check_prereq().expect_err("should error");
        let msg = format!("{err:#}");
        assert!(msg.contains("haiku"), "got: {msg}");
        assert!(msg.contains("M2.5"), "got: {msg}");
    }

    #[test]
    fn unsupported_driver_target_label() {
        let d = UnsupportedDriver { os: "haiku" };
        assert_eq!(d.target_for_host(), "unsupported");
    }

    #[test]
    fn vm_status_variants_construct() {
        let h = VmHandle { id: "12345".into() };
        let _ = VmStatus::Running {
            handle: h.clone(),
            urls: vec!["http://localhost:8088".into()],
        };
        let _ = VmStatus::Stopped;
        let _ = VmStatus::Paused { handle: h.clone() };
        assert_eq!(h.id, "12345");
    }

    #[test]
    fn current_driver_returns_a_driver() {
        // Sanity: `current_driver()` should always return something on
        // any test host. We don't exercise the methods here — that's the
        // integration test's job.
        let d = current_driver();
        // target_for_host should be one of our four known strings.
        let t = d.target_for_host();
        assert!(
            matches!(
                t,
                "qemu-x86_64" | "qemu-aarch64" | "wsl2" | "macos-qemu" | "unsupported"
            ),
            "unexpected target: {t}"
        );
    }
}
