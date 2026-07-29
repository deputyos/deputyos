//! Local-agent operations — a thin wrapper over `deputyos-desktop`'s
//! named-instance registry + driver, exposing the lifecycle the console UI
//! needs.
//!
//! The registry persists at `deputyos-desktop::config::data_dir()/instances.json`
//! (env-overridable via `DEPUTYOS_DESKTOP_DATA_DIR`), so the console scopes its
//! instances by pointing that env at a per-console data dir. [`InstanceOps`]
//! holds the host [`Driver`] (compile-time-selected) and drives it per-instance
//! via `Driver::*_with`.
//!
//! Local-agent management uses qemu/KVM on Linux, WSL2 on Windows, and UTM on
//! macOS. Linux and Windows expose per-instance localhost ports; UTM instances
//! use their distinct shared-network guest IPs. Remote management (fleet +
//! outbound tunnel) is independent of the driver and works on every supported
//! desktop platform.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use crate::config;
use crate::driver::{current_driver, Driver, DriverCapabilities, VmStatus};
use crate::instance::{Instance, InstanceConfig, Registry, ResourceSpec};
use crate::{download, manifest};

/// Local-agent lifecycle ops. Holds the host driver; the registry is loaded
/// from disk on each call (small file, single-process console).
pub struct InstanceOps {
    driver: Box<dyn Driver>,
}

impl InstanceOps {
    pub fn new() -> Self {
        Self {
            driver: current_driver(),
        }
    }

    /// The target this host's driver builds images for (`qemu-x86_64`, …).
    /// Captured into each new [`Instance`] so the console knows which
    /// artefact to install for it.
    pub fn target(&self) -> &'static str {
        self.driver.target_for_host()
    }

    /// List all registered instances.
    pub fn list(&self) -> Result<Vec<Instance>> {
        Ok(Registry::load()?.instances)
    }

    /// Create a new named instance, persist it, and return it.
    pub fn create(
        &self,
        name: &str,
        profile: Option<String>,
        manifest_url: Option<String>,
        channel: Option<String>,
    ) -> Result<Instance> {
        let mut reg = Registry::load()?;
        let inst = reg.create_instance(name, self.target(), profile, manifest_url, channel)?;
        reg.upsert(inst.clone());
        reg.save()?;
        Ok(inst)
    }

    /// Delete an instance: stop it if running, drop the registry entry, and
    /// best-effort remove its per-instance cache/runtime dirs.
    pub fn delete(&self, id: &str) -> Result<()> {
        let mut reg = Registry::load()?;
        let inst = reg
            .instances
            .iter()
            .find(|instance| instance.id == id || instance.name == id)
            .ok_or_else(|| anyhow::anyhow!("no instance matching {id:?}"))?
            .clone();
        // Stop if running (best-effort — ignore "not running").
        let cfg = InstanceConfig::from_instance(&inst);
        let _ = self.driver.stop_with(&cfg);
        // Remove registry entry + persist.
        reg.remove(&inst.id);
        reg.save()?;
        // Best-effort dir cleanup.
        let _ = std::fs::remove_dir_all(&inst.cache_dir);
        let _ = std::fs::remove_dir_all(&inst.runtime_dir);
        Ok(())
    }

    /// Start an instance. Idempotent. Returns the wizard URL to open.
    pub fn start(&self, id: &str) -> Result<String> {
        let cfg = self.cfg_for(id)?;
        let _handle = self.driver.start_with(&cfg).context("starting instance")?;
        let url = self.reachable_wizard_url(&cfg);
        self.set_status(id, "running");
        Ok(url)
    }

    /// Stop an instance. Idempotent.
    pub fn stop(&self, id: &str) -> Result<()> {
        let cfg = self.cfg_for(id)?;
        self.driver.stop_with(&cfg).context("stopping instance")?;
        self.set_status(id, "stopped");
        Ok(())
    }

    /// Cooperatively quiesce guest workloads, reclaim idle memory where the
    /// backend supports it, then pause/suspend the instance.
    pub fn pause(&self, id: &str) -> Result<()> {
        let cfg = self.cfg_for(id)?;
        self.driver.pause_with(&cfg).context("pausing instance")?;
        self.set_status(id, "paused");
        Ok(())
    }

    /// Resume the host instance and thaw its in-image workload slice.
    pub fn resume(&self, id: &str) -> Result<String> {
        let cfg = self.cfg_for(id)?;
        self.driver.resume_with(&cfg).context("resuming instance")?;
        self.set_status(id, "running");
        Ok(self.reachable_wizard_url(&cfg))
    }

    pub fn set_memory(&self, id: &str, target_mib: u64) -> Result<()> {
        let cfg = self.cfg_for(id)?;
        self.driver
            .set_memory_with(&cfg, target_mib)
            .context("setting live memory target")
    }

    pub fn agent_health(&self, id: &str) -> Result<deputyd::AgentResult> {
        let cfg = self.cfg_for(id)?;
        self.driver
            .guest_agent_with(&cfg, deputyd::AgentCommand::Health)
            .context("querying resident agent")
    }

    /// Persist the boot-time resource envelope. Running or paused instances
    /// must be stopped before vCPU/maximum-memory changes are applied.
    pub fn configure_resources(&self, id: &str, resources: ResourceSpec) -> Result<Instance> {
        let resources = resources.validate()?;
        let current = self.status(id)?;
        if !matches!(current, VmStatus::Stopped) {
            anyhow::bail!("stop the instance before changing its resource envelope");
        }
        let mut registry = Registry::load()?;
        let instance = registry
            .instances
            .iter_mut()
            .find(|instance| instance.id == id || instance.name == id)
            .ok_or_else(|| anyhow::anyhow!("no instance matching {id:?}"))?;
        instance.resources = resources;
        let updated = instance.clone();
        registry.save()?;
        Ok(updated)
    }

    pub fn capabilities(&self) -> DriverCapabilities {
        self.driver.capabilities()
    }

    /// Current liveness of an instance.
    pub fn status(&self, id: &str) -> Result<VmStatus> {
        let cfg = self.cfg_for(id)?;
        self.driver.status_with(&cfg)
    }

    /// The wizard URL for an instance (open in a webview).
    pub fn wizard_url(&self, id: &str) -> Result<String> {
        let cfg = self.cfg_for(id)?;
        Ok(self.reachable_wizard_url(&cfg))
    }

    /// Install (or update) the image for an instance: fetch + verify the
    /// manifest, download + sha + minisign-verify the artefact into the
    /// instance's cache dir, then hand it to the driver. Mirrors
    /// `deputyos-desktop/src/main.rs::cmd_install` but parameterised by the
    /// instance's target/cache/manifest.
    pub fn install(&self, id: &str) -> Result<()> {
        let inst = self.instance(id)?;
        let target = &inst.target;
        let cache_dir = inst.cache_dir.clone();
        std::fs::create_dir_all(&cache_dir)
            .with_context(|| format!("creating instance cache dir {}", cache_dir.display()))?;

        let manifest_url = inst
            .manifest_url
            .clone()
            .unwrap_or_else(config::manifest_url);
        let pubkey = config::pubkey_path();

        let src = manifest::fetch_and_verify(&manifest_url, &pubkey)
            .context("fetching + verifying manifest")?;
        let artefact = manifest::pick_artefact(&src, target, inst.profile.as_deref())?;
        let (img_url, sig_url) = manifest::artefact_urls(&src, artefact)?;

        let img_dest = cached_artefact_path(&cache_dir, target, &artefact.filename);
        let sig_dest = sig_path_for(&img_dest);
        download::download_and_verify(
            &img_url,
            &sig_url,
            &img_dest,
            &sig_dest,
            &artefact.sha256,
            &pubkey,
        )?;
        self.driver
            .install_image_with(&img_dest, &InstanceConfig::from_instance(&inst))?;
        Ok(())
    }

    // ---- helpers ----

    fn instance(&self, id: &str) -> Result<Instance> {
        let reg = Registry::load()?;
        reg.instances
            .iter()
            .find(|instance| instance.id == id || instance.name == id)
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("no instance matching {id:?}"))
    }

    fn cfg_for(&self, id: &str) -> Result<InstanceConfig> {
        Ok(InstanceConfig::from_instance(&self.instance(id)?))
    }

    fn reachable_wizard_url(&self, cfg: &InstanceConfig) -> String {
        match self.driver.status_with(cfg) {
            Ok(VmStatus::Running { urls, .. }) => urls
                .into_iter()
                .find(|url| url.ends_with(":8088") || url.contains(":8088/"))
                .unwrap_or_else(|| cfg.wizard_url()),
            _ => cfg.wizard_url(),
        }
    }

    /// Update `last_status` and persist. Best-effort: a failed save doesn't
    /// fail the lifecycle op that produced the status.
    fn set_status(&self, id: &str, status: &str) {
        if let Ok(mut reg) = Registry::load() {
            if let Some(inst) = reg
                .instances
                .iter_mut()
                .find(|instance| instance.id == id || instance.name == id)
            {
                inst.last_status = Some(status.to_string());
                let _ = reg.save();
            }
        }
    }

    /// Borrow the underlying driver (for the GUI to surface prereq errors).
    pub fn driver(&self) -> &dyn Driver {
        self.driver.as_ref()
    }
}

impl Default for InstanceOps {
    fn default() -> Self {
        Self::new()
    }
}

/// Resolve the on-disk cache path for an artefact: `<cache>/deputyos-<target>.<ext>`.
/// Matches `deputyos-desktop/src/main.rs::cached_artefact_path` so the driver's
/// `start_with` (which looks for exactly that name) finds it.
fn cached_artefact_path(cache_dir: &Path, target: &str, filename: &str) -> PathBuf {
    cache_dir.join(format!("deputyos-{target}.{}", artefact_suffix(filename)))
}

/// The sidecar `.minisig` path for a given artefact path. Matches
/// `deputyos-desktop/src/main.rs`'s convention.
fn sig_path_for(img: &Path) -> PathBuf {
    img.with_extension(format!(
        "{}.minisig",
        img.extension()
            .and_then(|s| s.to_str())
            .unwrap_or("artefact")
    ))
}

/// Extract the artefact's file extension, collapsing `.tar.gz` to `tar.gz`
/// (matches `deputyos-desktop::artefact_suffix`).
fn artefact_suffix(filename: &str) -> String {
    let lower = filename.to_ascii_lowercase();
    if lower.ends_with(".tar.gz") {
        return "tar.gz".to_string();
    }
    Path::new(filename)
        .extension()
        .and_then(|s| s.to_str())
        .filter(|s| !s.is_empty())
        .unwrap_or("img")
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn artefact_suffix_handles_qcow2_and_tar_gz() {
        assert_eq!(
            artefact_suffix("deputyos-openclaw-qemu-x86_64-2026.6.22-dev.qcow2"),
            "qcow2"
        );
        assert_eq!(
            artefact_suffix("deputyos-openclaw-wsl2-1.0.tar.gz"),
            "tar.gz"
        );
        assert_eq!(artefact_suffix("blob.img"), "img");
        assert_eq!(artefact_suffix("noext"), "img");
    }

    #[test]
    fn cached_artefact_path_matches_driver_expectation() {
        let p = cached_artefact_path(
            Path::new("/cache"),
            "qemu-x86_64",
            "deputyos-openclaw-qemu-x86_64-2026.6.22-dev.qcow2",
        );
        // The Linux driver's start_with looks for exactly this name.
        assert_eq!(p, PathBuf::from("/cache/deputyos-qemu-x86_64.qcow2"));
    }

    #[test]
    fn sig_path_appends_minisig() {
        let p = sig_path_for(&PathBuf::from("/cache/deputyos-qemu-x86_64.qcow2"));
        assert_eq!(
            p,
            PathBuf::from("/cache/deputyos-qemu-x86_64.qcow2.minisig")
        );
    }

    #[test]
    fn instance_ops_target_matches_current_driver() {
        let ops = InstanceOps::new();
        let t = ops.target();
        // On a built launcher target this is one of the known strings.
        assert!(
            matches!(
                t,
                "qemu-x86_64" | "qemu-aarch64" | "wsl2" | "macos-qemu" | "unsupported"
            ),
            "unexpected target: {t}"
        );
    }
}
