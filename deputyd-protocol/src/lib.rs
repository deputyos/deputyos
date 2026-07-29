//! Open, implementation-neutral protocol for deputyOS guest capabilities.
//!
//! Every official deputyOS image includes the proprietary resident server.
//! Host tools use this public crate without depending on that implementation.

use std::io::Write;
#[cfg(unix)]
use std::io::{BufRead, BufReader};
#[cfg(unix)]
use std::os::unix::net::UnixStream;
use std::path::Path;

use anyhow::{anyhow, bail, Context, Result};
use serde::{Deserialize, Serialize};

pub const PROTOCOL_VERSION: u16 = 2;
pub const MIN_PROTOCOL_VERSION: u16 = 1;
pub const DEFAULT_SOCKET: &str = "/run/deputyos/deputyd.sock";
pub const MAX_LOG_LINES: u16 = 500;
pub const MAX_EVENT_ITEMS: u16 = 200;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AgentRequest {
    pub protocol: u16,
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub actor: Option<ActorContext>,
    pub command: AgentCommand,
}

impl AgentRequest {
    pub fn new(id: impl Into<String>, command: AgentCommand) -> Self {
        Self {
            protocol: PROTOCOL_VERSION,
            id: id.into(),
            actor: None,
            command,
        }
    }

    pub fn with_actor(mut self, actor: ActorContext) -> Self {
        self.actor = Some(actor);
        self
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ActorContext {
    pub kind: String,
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "command", rename_all = "snake_case")]
pub enum AgentCommand {
    Health,
    Capabilities,
    PreparePause,
    Resume,
    Reclaim {
        #[serde(default)]
        drop_caches: bool,
    },
    Workload {
        action: WorkloadAction,
    },
    SetResources {
        #[serde(default)]
        memory_high_bytes: Option<u64>,
        #[serde(default)]
        memory_max_bytes: Option<u64>,
        #[serde(default)]
        cpu_quota_percent: Option<u16>,
        #[serde(default)]
        io_weight: Option<u16>,
    },
    Snapshot {
        action: SnapshotAction,
    },
    UpdateStatus,
    UpdateRun,
    Repair {
        #[serde(default)]
        run: bool,
    },
    NetworkStatus,
    TunnelRestart,
    StorageStatus,
    Backup {
        #[serde(default)]
        run: bool,
    },
    Logs {
        #[serde(default = "default_log_lines")]
        lines: u16,
    },
    Events {
        #[serde(default = "default_event_items")]
        limit: u16,
    },
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum WorkloadAction {
    Status,
    Start,
    Stop,
    Restart,
    Reconcile,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SnapshotAction {
    Prepare,
    Complete,
}

fn default_log_lines() -> u16 {
    100
}

fn default_event_items() -> u16 {
    50
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AgentResponse {
    pub protocol: u16,
    pub id: String,
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<AgentResult>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum AgentResult {
    Health {
        report: HealthReport,
    },
    State {
        state: LifecycleState,
    },
    Reclaimed {
        compacted: bool,
        caches_dropped: bool,
    },
    Capabilities {
        report: CapabilityReport,
    },
    Workload {
        report: WorkloadReport,
    },
    Resources {
        report: ResourceReport,
    },
    Update {
        report: UpdateReport,
    },
    Repair {
        report: RepairReport,
    },
    Network {
        report: NetworkReport,
    },
    Storage {
        report: StorageReport,
    },
    Backup {
        report: BackupReport,
    },
    Logs {
        unit: String,
        lines: Vec<String>,
        truncated: bool,
    },
    Events {
        events: Vec<AgentEvent>,
        truncated: bool,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CapabilityReport {
    pub protocol: u16,
    pub minimum_protocol: u16,
    pub commands: Vec<String>,
    pub max_log_lines: u16,
    pub max_event_items: u16,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WorkloadReport {
    pub profile: String,
    pub unit: String,
    pub active: bool,
    pub state: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ResourceReport {
    pub slice: String,
    pub memory_high_bytes: Option<u64>,
    pub memory_max_bytes: Option<u64>,
    pub cpu_quota_percent: Option<u16>,
    pub io_weight: Option<u16>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct UpdateReport {
    pub slots: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RepairReport {
    pub requested: bool,
    pub service_active: bool,
    pub last_report: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct NetworkReport {
    pub tunnel_active: bool,
    pub policy: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct StorageReport {
    pub filesystems: Vec<FilesystemReport>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct BackupReport {
    pub requested: bool,
    pub service_active: bool,
    pub timer_active: bool,
    pub last_status: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct FilesystemReport {
    pub mount: String,
    pub total_bytes: u64,
    pub used_bytes: u64,
    pub available_bytes: u64,
    pub used_percent: u8,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AgentEvent {
    pub at_unix: u64,
    pub request_id: String,
    pub command: String,
    pub ok: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub actor: Option<ActorContext>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum LifecyclePhase {
    Active,
    Quiesced,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct LifecycleState {
    pub phase: LifecyclePhase,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub changed_at_unix: Option<u64>,
}

impl Default for LifecycleState {
    fn default() -> Self {
        Self {
            phase: LifecyclePhase::Active,
            changed_at_unix: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct HealthReport {
    pub agent_version: String,
    pub protocol: u16,
    pub lifecycle: LifecycleState,
    pub memory: MemoryReport,
    pub uptime_seconds: Option<f64>,
    pub memory_pressure: Option<String>,
    pub active_profile: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct MemoryReport {
    pub total_bytes: Option<u64>,
    pub available_bytes: Option<u64>,
    pub free_bytes: Option<u64>,
    pub swap_total_bytes: Option<u64>,
    pub swap_free_bytes: Option<u64>,
}

#[cfg(unix)]
pub fn request(socket: &Path, request: &AgentRequest) -> Result<AgentResponse> {
    let mut stream = UnixStream::connect(socket).with_context(|| {
        format!(
            "connecting to optional resident service {}",
            socket.display()
        )
    })?;
    serde_json::to_writer(&mut stream, request).context("encoding resident request")?;
    stream.write_all(b"\n").context("terminating request")?;
    let mut line = String::new();
    BufReader::new(stream)
        .read_line(&mut line)
        .context("reading resident response")?;
    if line.is_empty() {
        bail!("resident service closed the socket without a response");
    }
    serde_json::from_str(&line).context("decoding resident response")
}

#[cfg(not(unix))]
pub fn request(_socket: &Path, _request: &AgentRequest) -> Result<AgentResponse> {
    bail!("the optional resident Unix-socket protocol is unavailable on this host")
}

pub fn capability_report() -> CapabilityReport {
    CapabilityReport {
        protocol: PROTOCOL_VERSION,
        minimum_protocol: MIN_PROTOCOL_VERSION,
        commands: [
            "agent.health",
            "agent.capabilities",
            "lifecycle.prepare_pause",
            "lifecycle.resume",
            "memory.reclaim",
            "workload.status",
            "workload.start",
            "workload.stop",
            "workload.restart",
            "workload.reconcile",
            "resources.set",
            "snapshot.prepare",
            "snapshot.complete",
            "update.status",
            "update.run",
            "repair.status",
            "repair.run",
            "network.status",
            "tunnel.restart",
            "storage.status",
            "backup.status",
            "backup.run",
            "logs.tail",
            "events.list",
        ]
        .into_iter()
        .map(str::to_string)
        .collect(),
        max_log_lines: MAX_LOG_LINES,
        max_event_items: MAX_EVENT_ITEMS,
    }
}

pub fn ensure_success(response: AgentResponse) -> Result<AgentResult> {
    if !response.ok {
        bail!(
            "{}",
            response
                .error
                .unwrap_or_else(|| "resident request failed".to_string())
        );
    }
    response
        .result
        .ok_or_else(|| anyhow!("resident returned success without a result"))
}
