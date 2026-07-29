//! Named-instance registry + per-instance runtime config.
//!
//! The desktop launcher was originally single-instance: the `Driver` methods
//! read ports/cache/pid from global `config::*` getters, so a second `start`
//! collides with the first. The desktop console needs to run and manage
//! **multiple** local agents at once, so we layer a small persisted registry
//! on top:
//!
//! - [`Instance`] is a named record (uuid id, user-facing name, per-instance
//!   ports + cache/runtime dirs). Persisted as `instances.json` under
//!   `config::data_dir()` — which is already env-overridable
//!   (`DEPUTYOS_DESKTOP_DATA_DIR`), so integration tests that set that var get
//!   an isolated registry for free.
//! - [`InstanceConfig`] is the narrower struct the `Driver::*_with` methods
//!   consume — just the ports/dirs/seed needed to boot one VM. It has an
//!   [`InstanceConfig::from_env`] back-compat path that reads the same
//!   `config::*` getters the single-instance CLI always used, so the existing
//!   `start()`/`stop()`/`status()` trait methods (which now delegate to the
//!   `*_with` forms) behave exactly as before.
//!
//! The console creates instances with distinct port pairs (allocated via
//! [`allocate_port_pair`]) and distinct cache/runtime dirs, so two VMs never
//! fight over one qcow2 or one PID file.

use std::net::TcpListener;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::config;

/// Requested compute envelope for one deputy. The guest boots with
/// `memory_max_mib`; balloon-capable backends may reclaim down to
/// `memory_min_mib` while it is idle or paused.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub struct ResourceSpec {
    pub vcpus: u16,
    pub memory_min_mib: u64,
    pub memory_max_mib: u64,
    pub auto_balloon: bool,
}

impl Default for ResourceSpec {
    fn default() -> Self {
        Self {
            vcpus: 2,
            memory_min_mib: 1024,
            memory_max_mib: 4096,
            auto_balloon: true,
        }
    }
}

impl ResourceSpec {
    pub fn validate(self) -> Result<Self> {
        if self.vcpus == 0 {
            anyhow::bail!("vcpus must be at least 1");
        }
        if self.memory_min_mib < 256 {
            anyhow::bail!("minimum memory must be at least 256 MiB");
        }
        if self.memory_max_mib < self.memory_min_mib {
            anyhow::bail!("maximum memory must be greater than or equal to minimum memory");
        }
        Ok(self)
    }

    fn from_env() -> Self {
        let defaults = Self::default();
        let vcpus = env_parse("DEPUTYOS_DESKTOP_VCPUS").unwrap_or(defaults.vcpus);
        let memory_min_mib =
            env_parse("DEPUTYOS_DESKTOP_MEMORY_MIN_MIB").unwrap_or(defaults.memory_min_mib);
        let memory_max_mib =
            env_parse("DEPUTYOS_DESKTOP_MEMORY_MAX_MIB").unwrap_or(defaults.memory_max_mib);
        let auto_balloon = std::env::var("DEPUTYOS_DESKTOP_AUTO_BALLOON")
            .ok()
            .map(|value| !matches!(value.as_str(), "0" | "false" | "no"))
            .unwrap_or(defaults.auto_balloon);
        Self {
            vcpus,
            memory_min_mib,
            memory_max_mib,
            auto_balloon,
        }
    }
}

/// One managed local agent. Stored in [`Registry`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Instance {
    /// uuid v4 string; the registry slug and the canonical id surfaced to
    /// the console UI.
    pub id: String,
    /// User-facing name, unique within the registry (enforced at create time).
    pub name: String,
    /// Image target this instance boots, e.g. `qemu-x86_64`. Captured at
    /// creation from `Driver::target_for_host` so the console knows which
    /// artefact to install for this instance.
    pub target: String,
    /// Optional profile id (`openclaw`, `hermes`, …). `None` = default.
    pub profile: Option<String>,
    /// Host-side port forwarded to the in-VM wizard (:8088 in the guest).
    pub wizard_port: u16,
    /// Host-side port forwarded to the in-VM chat/relay (:8080 in the guest).
    pub gateway_port: u16,
    /// Per-instance image cache dir. Each instance gets its own so two VMs
    /// don't fight over one qcow2.
    pub cache_dir: PathBuf,
    /// Per-instance runtime dir (the PID file lives here).
    pub runtime_dir: PathBuf,
    /// Override of the manifest URL for this instance. `None` = use
    /// `config::manifest_url()` (the global default).
    pub manifest_url: Option<String>,
    /// Channel override (`dev`/`beta`/`stable`). `None` = global default.
    pub channel: Option<String>,
    /// Creation time, unix seconds. Dep-free (no chrono) — the console
    /// formats for display.
    pub created_at: u64,
    /// Last observed liveness, for dashboard state. `"running"` / `"stopped"`
    /// / `None` (never started).
    pub last_status: Option<String>,
    /// CPU and memory envelope. `serde(default)` keeps registries written by
    /// older desktop versions forward-compatible.
    #[serde(default)]
    pub resources: ResourceSpec,
}

/// Persisted collection of instances. On-disk format is JSON with one
/// `instances` array; missing file = empty registry (first run).
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Registry {
    pub instances: Vec<Instance>,
}

impl Registry {
    /// Path the registry is stored at: `config::data_dir()/instances.json`.
    /// Reuses the env-overridable `DEPUTYOS_DESKTOP_DATA_DIR`, so tests that set
    /// that var get an isolated registry for free.
    fn path() -> PathBuf {
        config::data_dir().join("instances.json")
    }

    /// Load the registry. A missing file is not an error — it yields an empty
    /// registry (first run / fresh data dir).
    pub fn load() -> Result<Self> {
        let path = Self::path();
        match std::fs::read(&path) {
            Ok(bytes) => serde_json::from_slice(&bytes)
                .with_context(|| format!("parsing {}", path.display())),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(anyhow::anyhow!("reading {}: {e}", path.display())),
        }
    }

    /// Atomically persist the registry (temp file + rename). Creates the
    /// parent data dir if needed.
    pub fn save(&self) -> Result<()> {
        let path = Self::path();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("creating data dir {}", parent.display()))?;
        }
        let tmp = path.with_extension("json.tmp");
        let bytes = serde_json::to_vec_pretty(self).context("serializing registry")?;
        std::fs::write(&tmp, bytes).with_context(|| format!("writing {}", tmp.display()))?;
        std::fs::rename(&tmp, &path)
            .with_context(|| format!("renaming {} -> {}", tmp.display(), path.display()))?;
        Ok(())
    }

    /// Look up an instance by id.
    pub fn get(&self, id: &str) -> Option<&Instance> {
        self.instances.iter().find(|i| i.id == id)
    }

    /// Mutable lookup by id.
    pub fn get_mut(&mut self, id: &str) -> Option<&mut Instance> {
        self.instances.iter_mut().find(|i| i.id == id)
    }

    /// Insert or replace an instance by id.
    pub fn upsert(&mut self, inst: Instance) {
        if let Some(existing) = self.instances.iter_mut().find(|i| i.id == inst.id) {
            *existing = inst;
        } else {
            self.instances.push(inst);
        }
    }

    /// Remove an instance by id. Returns true if an instance was removed.
    pub fn remove(&mut self, id: &str) -> bool {
        let before = self.instances.len();
        self.instances.retain(|i| i.id != id);
        self.instances.len() != before
    }

    /// Whether `name` is already taken by another instance (pass `None` as
    /// `except_id` for a fresh create, or the instance's own id when renaming
    /// in place so it doesn't collide with itself).
    pub fn name_taken(&self, name: &str, except_id: Option<&str>) -> bool {
        self.instances
            .iter()
            .any(|i| i.name == name && Some(i.id.as_str()) != except_id)
    }

    /// Build a new [`Instance`] ready to boot, allocating a free port pair and
    /// per-instance cache/runtime dirs under `config::data_dir()/instances/<id>/`.
    /// Checks name uniqueness against the existing registry and errors clearly
    /// on collision. Does NOT insert — the caller upserts after it decides the
    /// record is good (the console does, then persists).
    ///
    /// Layout: each instance gets its own cache + runtime dir keyed by id, so
    /// two VMs never share a qcow2 or a PID file.
    #[allow(clippy::too_many_arguments)]
    pub fn create_instance(
        &self,
        name: &str,
        target: &str,
        profile: Option<String>,
        manifest_url: Option<String>,
        channel: Option<String>,
    ) -> Result<Instance> {
        if self.name_taken(name, None) {
            anyhow::bail!("an instance named {name:?} already exists in this registry");
        }
        let mut exclude: Vec<u16> = Vec::new();
        for i in &self.instances {
            exclude.push(i.wizard_port);
            exclude.push(i.gateway_port);
        }
        let (wizard_port, gateway_port) = allocate_port_pair(&exclude)
            .ok_or_else(|| anyhow::anyhow!("no free loopback port pair for instance {name:?}"))?;
        let id = new_id();
        let base = config::data_dir().join("instances").join(&id);
        let inst = Instance {
            id,
            name: name.to_string(),
            target: target.to_string(),
            profile,
            wizard_port,
            gateway_port,
            cache_dir: base.join("cache"),
            runtime_dir: base.join("run"),
            manifest_url,
            channel,
            created_at: now_unix(),
            last_status: None,
            resources: ResourceSpec::default(),
        };
        Ok(inst)
    }
}

/// Generate a fresh v4 uuid instance id.
pub fn new_id() -> String {
    uuid::Uuid::new_v4().to_string()
}

/// Per-instance runtime configuration consumed by `Driver::*_with`. Narrower
/// than [`Instance`] — just what the driver needs to boot one VM.
#[derive(Debug, Clone)]
pub struct InstanceConfig {
    pub wizard_port: u16,
    pub gateway_port: u16,
    pub cache_dir: PathBuf,
    pub runtime_dir: PathBuf,
    /// Optional NoCloud seed ISO (the local dev loop uses this to inject
    /// `DEPUTYOS_API_BASE`). Per-instance so a console-managed agent can carry
    /// its own seed; `None` = no seed (production path).
    pub seed_iso: Option<PathBuf>,
    pub resources: ResourceSpec,
}

impl InstanceConfig {
    /// Build the config from an [`Instance`] record.
    pub fn from_instance(i: &Instance) -> Self {
        Self {
            wizard_port: i.wizard_port,
            gateway_port: i.gateway_port,
            cache_dir: i.cache_dir.clone(),
            runtime_dir: i.runtime_dir.clone(),
            // Instances created by the console don't carry a seed; the dev
            // loop's seed is a separate (env-driven) path.
            seed_iso: None,
            resources: i.resources,
        }
    }

    /// Back-compat path: read the same `config::*` getters the single-instance
    /// CLI always used, so the trait's default `start()`/`stop()`/`status()`
    /// (which delegate to the `*_with` forms via this) behave exactly as before.
    pub fn from_env() -> Self {
        Self {
            wizard_port: config::wizard_host_port(),
            gateway_port: config::gateway_host_port(),
            cache_dir: config::cache_dir(),
            runtime_dir: config::runtime_dir(),
            seed_iso: std::env::var_os("DEPUTYOS_DESKTOP_SEED_ISO").map(PathBuf::from),
            resources: ResourceSpec::from_env(),
        }
    }

    /// Wizard URL for this instance. Honors the global
    /// `DEPUTYOS_DESKTOP_WIZARD_URL` override (the single-instance escape hatch)
    /// when set; otherwise builds `http://localhost:<wizard_port>`. The
    /// console does not set WIZARD_URL, so multi-instance uses port-based urls.
    pub fn wizard_url(&self) -> String {
        if let Ok(u) = std::env::var("DEPUTYOS_DESKTOP_WIZARD_URL") {
            return u;
        }
        format!("http://localhost:{}", self.wizard_port)
    }

    /// Per-instance slug derived from the cache_dir layout. Console-created
    /// instances nest `cache_dir` under `<data_dir>/instances/<id>/cache`, so
    /// the parent directory's file name *is* the instance id (the same uuid
    /// stored in [`Instance::id`]). The single-instance [`InstanceConfig::from_env`]
    /// path has no such nesting (its `cache_dir` is the global cache dir), so
    /// this returns `None` and the driver falls back to the bare fixed name
    /// (`deputyos`). Used by the Windows/macOS drivers (M9.5) to key the WSL
    /// distro name / UTM VM name per instance so two console-managed agents
    /// register as independent distros/VMs instead of colliding on one
    /// `deputyos`.
    pub fn instance_slug(&self) -> Option<String> {
        // Console-created instances nest cache_dir as
        // `<data_dir>/instances/<id>/cache`. We require that shape — the
        // parent of cache_dir is the id, and its parent in turn is literally
        // `instances` — so a single-instance `from_env()` path like
        // `/data/cache` (parent `data`, no `instances` grandparent) is not
        // mistaken for an instance slug.
        let id_dir = self.cache_dir.parent()?;
        let grandparent = id_dir.parent()?;
        if grandparent.file_name().and_then(|s| s.to_str()) != Some("instances") {
            return None;
        }
        let id = id_dir.file_name()?.to_str()?;
        if id.is_empty() {
            return None;
        }
        Some(id.to_string())
    }
}

fn env_parse<T: std::str::FromStr>(name: &str) -> Option<T> {
    std::env::var(name).ok()?.parse().ok()
}

/// The PID filename within an instance's runtime dir. Constant — uniqueness
/// comes from each instance having its own `runtime_dir`, not from the
/// filename. Kept here so both the driver and tests agree on the name.
pub const PID_FILENAME: &str = "deputyos-desktop.pid";

/// Allocate a (wizard_port, gateway_port) pair of free loopback ports,
/// skipping any in `exclude` (the ports already used by other instances).
/// There is an inherent TOCTOU between the bind-drop here and the qemu bind
/// later, but for a desktop tool that's acceptable — on `EADDRINUSE` the
/// console retries with a fresh pair. Returns `None` only if the OS refused
/// to hand back a free port after several tries (extremely unlikely).
pub fn allocate_port_pair(exclude: &[u16]) -> Option<(u16, u16)> {
    let wizard = bind_ephemeral(exclude)?;
    let gateway = bind_ephemeral(&{
        let mut v = exclude.to_vec();
        v.push(wizard);
        v
    })?;
    Some((wizard, gateway))
}

/// Bind `127.0.0.1:0`, read back the assigned port, drop the listener, and
/// return the port — skipping any in `exclude`. Retries a bounded number of
/// times so a persistently unlucky exclude set doesn't loop forever.
fn bind_ephemeral(exclude: &[u16]) -> Option<u16> {
    for _ in 0..64 {
        let listener = TcpListener::bind("127.0.0.1:0").ok()?;
        let port = listener.local_addr().ok()?.port();
        drop(listener);
        if !exclude.contains(&port) {
            return Some(port);
        }
    }
    None
}

/// Current time as unix seconds. Dep-free (no chrono) — used for
/// `Instance::created_at`.
pub fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registry_round_trips_through_json() {
        let inst = Instance {
            id: "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee".into(),
            name: "agent-1".into(),
            target: "qemu-x86_64".into(),
            profile: Some("openclaw".into()),
            wizard_port: 17088,
            gateway_port: 17080,
            cache_dir: PathBuf::from("/tmp/cache-1"),
            runtime_dir: PathBuf::from("/tmp/run-1"),
            manifest_url: None,
            channel: None,
            created_at: 1_700_000_000,
            last_status: Some("running".into()),
            resources: ResourceSpec::default(),
        };
        let reg = Registry {
            instances: vec![inst.clone()],
        };
        let json = serde_json::to_string(&reg).expect("ser");
        let back: Registry = serde_json::from_str(&json).expect("de");
        assert_eq!(back.instances.len(), 1);
        assert_eq!(back.instances[0].id, inst.id);
        assert_eq!(back.instances[0].name, "agent-1");
        assert_eq!(back.instances[0].wizard_port, 17088);
    }

    #[test]
    fn upsert_replaces_by_id_and_appends_new() {
        let mut reg = Registry::default();
        let inst = Instance {
            id: "id-1".into(),
            name: "a".into(),
            target: "qemu-x86_64".into(),
            profile: None,
            wizard_port: 1,
            gateway_port: 2,
            cache_dir: PathBuf::new(),
            runtime_dir: PathBuf::new(),
            manifest_url: None,
            channel: None,
            created_at: 0,
            last_status: None,
            resources: ResourceSpec::default(),
        };
        reg.upsert(inst.clone());
        assert_eq!(reg.instances.len(), 1);
        // Replace same id with a new name.
        let mut updated = inst.clone();
        updated.name = "a-renamed".into();
        reg.upsert(updated);
        assert_eq!(reg.instances.len(), 1, "upsert by id replaces, not appends");
        assert_eq!(reg.instances[0].name, "a-renamed");
        // Append a different id.
        let mut other = inst.clone();
        other.id = "id-2".into();
        other.name = "b".into();
        reg.upsert(other);
        assert_eq!(reg.instances.len(), 2);
    }

    #[test]
    fn remove_returns_true_only_when_present() {
        let mut reg = Registry::default();
        let inst = Instance {
            id: "id-1".into(),
            name: "a".into(),
            target: "qemu-x86_64".into(),
            profile: None,
            wizard_port: 1,
            gateway_port: 2,
            cache_dir: PathBuf::new(),
            runtime_dir: PathBuf::new(),
            manifest_url: None,
            channel: None,
            created_at: 0,
            last_status: None,
            resources: ResourceSpec::default(),
        };
        reg.upsert(inst);
        assert!(reg.remove("id-1"));
        assert!(!reg.remove("id-1"), "second remove finds nothing");
        assert!(reg.instances.is_empty());
    }

    #[test]
    fn name_taken_respects_except_id() {
        let mut reg = Registry::default();
        let inst = Instance {
            id: "id-1".into(),
            name: "a".into(),
            target: "qemu-x86_64".into(),
            profile: None,
            wizard_port: 1,
            gateway_port: 2,
            cache_dir: PathBuf::new(),
            runtime_dir: PathBuf::new(),
            manifest_url: None,
            channel: None,
            created_at: 0,
            last_status: None,
            resources: ResourceSpec::default(),
        };
        reg.upsert(inst);
        assert!(reg.name_taken("a", None));
        assert!(!reg.name_taken("a", Some("id-1")), "own id is allowed");
        assert!(!reg.name_taken("b", None));
    }

    #[test]
    fn load_missing_file_is_empty_registry() {
        let _g = crate::config::test_env_lock()
            .lock()
            .expect("test_env_lock poisoned");
        let dir = tempfile::tempdir().expect("tempdir");
        // Point the data dir at an empty tempdir — no instances.json there.
        std::env::set_var(
            "DEPUTYOS_DESKTOP_DATA_DIR",
            dir.path().to_str().expect("utf8"),
        );
        let reg = Registry::load().expect("missing file = empty registry");
        assert!(reg.instances.is_empty());
        std::env::remove_var("DEPUTYOS_DESKTOP_DATA_DIR");
    }

    #[test]
    fn save_then_load_round_trips() {
        let _g = crate::config::test_env_lock()
            .lock()
            .expect("test_env_lock poisoned");
        let dir = tempfile::tempdir().expect("tempdir");
        std::env::set_var(
            "DEPUTYOS_DESKTOP_DATA_DIR",
            dir.path().to_str().expect("utf8"),
        );
        let mut reg = Registry::default();
        reg.upsert(Instance {
            id: "id-1".into(),
            name: "a".into(),
            target: "qemu-x86_64".into(),
            profile: None,
            wizard_port: 1,
            gateway_port: 2,
            cache_dir: PathBuf::new(),
            runtime_dir: PathBuf::new(),
            manifest_url: None,
            channel: None,
            created_at: 0,
            last_status: None,
            resources: ResourceSpec::default(),
        });
        reg.save().expect("save");
        assert!(dir.path().join("instances.json").is_file());
        let back = Registry::load().expect("load");
        assert_eq!(back.instances.len(), 1);
        assert_eq!(back.instances[0].id, "id-1");
        std::env::remove_var("DEPUTYOS_DESKTOP_DATA_DIR");
    }

    #[test]
    fn from_env_reads_config_getters() {
        let _g = crate::config::test_env_lock()
            .lock()
            .expect("test_env_lock poisoned");
        let dir = tempfile::tempdir().expect("tempdir");
        std::env::set_var(
            "DEPUTYOS_DESKTOP_CACHE_DIR",
            dir.path().join("c").to_str().expect("utf8"),
        );
        std::env::set_var(
            "DEPUTYOS_DESKTOP_RUNTIME_DIR",
            dir.path().join("r").to_str().expect("utf8"),
        );
        std::env::set_var("DEPUTYOS_DESKTOP_WIZARD_PORT", "18088");
        std::env::set_var("DEPUTYOS_DESKTOP_GATEWAY_PORT", "18080");
        std::env::remove_var("DEPUTYOS_DESKTOP_SEED_ISO");
        let cfg = InstanceConfig::from_env();
        assert_eq!(cfg.wizard_port, 18088);
        assert_eq!(cfg.gateway_port, 18080);
        assert_eq!(cfg.cache_dir, dir.path().join("c"));
        assert_eq!(cfg.runtime_dir, dir.path().join("r"));
        assert!(cfg.seed_iso.is_none());
        std::env::remove_var("DEPUTYOS_DESKTOP_CACHE_DIR");
        std::env::remove_var("DEPUTYOS_DESKTOP_RUNTIME_DIR");
        std::env::remove_var("DEPUTYOS_DESKTOP_WIZARD_PORT");
        std::env::remove_var("DEPUTYOS_DESKTOP_GATEWAY_PORT");
    }

    #[test]
    fn from_env_seed_iso_reads_env() {
        let _g = crate::config::test_env_lock()
            .lock()
            .expect("test_env_lock poisoned");
        std::env::set_var("DEPUTYOS_DESKTOP_SEED_ISO", "/tmp/seed.iso");
        let cfg = InstanceConfig::from_env();
        assert_eq!(cfg.seed_iso, Some(PathBuf::from("/tmp/seed.iso")));
        std::env::remove_var("DEPUTYOS_DESKTOP_SEED_ISO");
    }

    #[test]
    fn wizard_url_honors_global_override() {
        let _g = crate::config::test_env_lock()
            .lock()
            .expect("test_env_lock poisoned");
        let prev = std::env::var("DEPUTYOS_DESKTOP_WIZARD_URL").ok();
        std::env::set_var("DEPUTYOS_DESKTOP_WIZARD_URL", "http://explicit.test:9999");
        let cfg = InstanceConfig {
            wizard_port: 8088,
            gateway_port: 8080,
            cache_dir: PathBuf::new(),
            runtime_dir: PathBuf::new(),
            seed_iso: None,
            resources: ResourceSpec::default(),
        };
        assert_eq!(cfg.wizard_url(), "http://explicit.test:9999");
        match prev {
            Some(v) => std::env::set_var("DEPUTYOS_DESKTOP_WIZARD_URL", v),
            None => std::env::remove_var("DEPUTYOS_DESKTOP_WIZARD_URL"),
        }
    }

    #[test]
    fn wizard_url_defaults_to_localhost_port() {
        let _g = crate::config::test_env_lock()
            .lock()
            .expect("test_env_lock poisoned");
        let prev = std::env::var("DEPUTYOS_DESKTOP_WIZARD_URL").ok();
        std::env::remove_var("DEPUTYOS_DESKTOP_WIZARD_URL");
        let cfg = InstanceConfig {
            wizard_port: 7088,
            gateway_port: 7080,
            cache_dir: PathBuf::new(),
            runtime_dir: PathBuf::new(),
            seed_iso: None,
            resources: ResourceSpec::default(),
        };
        assert_eq!(cfg.wizard_url(), "http://localhost:7088");
        if let Some(v) = prev {
            std::env::set_var("DEPUTYOS_DESKTOP_WIZARD_URL", v);
        }
    }

    #[test]
    fn allocate_port_pair_returns_two_distinct_free_ports() {
        let pair = allocate_port_pair(&[]);
        let (w, g) = pair.expect("a free pair exists on any test host");
        assert_ne!(w, g, "wizard and gateway ports must differ");
        // Both should be ephemeral (>1024) and bindable right now.
        assert!(w > 1024);
        assert!(g > 1024);
        assert!(
            TcpListener::bind(("127.0.0.1", w)).is_ok(),
            "wizard port {w} should be re-bindable after release"
        );
    }

    #[test]
    fn allocate_port_pair_respects_exclude() {
        let pair = allocate_port_pair(&[]).expect("pair");
        let (w, g) = pair;
        // Allocating again excluding the first pair must not reuse either.
        let pair2 = allocate_port_pair(&[w, g]).expect("pair2");
        let (w2, g2) = pair2;
        assert_ne!(w2, w);
        assert_ne!(w2, g);
        assert_ne!(g2, w);
        assert_ne!(g2, g);
    }

    #[test]
    fn instance_slug_reads_id_from_cache_dir_layout() {
        // Console-created instances nest cache under instances/<id>/cache.
        let cfg = InstanceConfig {
            wizard_port: 8088,
            gateway_port: 8080,
            cache_dir: PathBuf::from("/data/instances/abc-123/cache"),
            runtime_dir: PathBuf::from("/data/instances/abc-123/run"),
            seed_iso: None,
            resources: ResourceSpec::default(),
        };
        assert_eq!(cfg.instance_slug().as_deref(), Some("abc-123"));
    }

    #[test]
    fn instance_slug_none_for_single_instance_env_path() {
        // from_env() single-instance path: cache_dir is the global cache dir,
        // not nested under instances/<id>/ → no slug → driver uses bare name.
        let cfg = InstanceConfig {
            wizard_port: 8088,
            gateway_port: 8080,
            cache_dir: PathBuf::from("/data/cache"),
            runtime_dir: PathBuf::from("/data/run"),
            seed_iso: None,
            resources: ResourceSpec::default(),
        };
        assert_eq!(cfg.instance_slug(), None);
    }

    #[test]
    fn instance_slug_none_when_parent_is_instances_dir() {
        // Defensive: a malformed path ending in /instances/cache must not
        // return "instances" as a slug.
        let cfg = InstanceConfig {
            wizard_port: 8088,
            gateway_port: 8080,
            cache_dir: PathBuf::from("/data/instances/cache"),
            runtime_dir: PathBuf::from("/data/run"),
            seed_iso: None,
            resources: ResourceSpec::default(),
        };
        assert_eq!(cfg.instance_slug(), None);
    }
}
