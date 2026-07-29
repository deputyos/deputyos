//! `deputyctl commands poll` — device-side async command-queue poller (M9.4).
//!
//! The sibling API (`api-deputyos-com/api/src/fleet.rs`) exposes an async
//! command queue: an account owner enqueues a command for a device
//! (`POST /api/v1/devices/:id/commands`), the device polls pending commands
//! (`GET /api/v1/devices/:id/commands/pending`, backup_token Bearer auth),
//! executes them, and acks the result
//! (`POST /api/v1/devices/:id/commands/:cmd_id/result`). The server side is
//! complete; **this module is the device-side poller that drains the queue**.
//! Without it, queued commands are dead-letter — offline command queueing
//! can't land.
//!
//! ## Safety model
//!
//! The poller executes the **allow-listed resident protocol v2** command set:
//! health/capabilities, lifecycle and snapshot coordination, memory/resource
//! control, active-workload lifecycle, signed update and repair runs, tunnel
//! control, and bounded status/log/event reads. `ping` and `restart-agent`
//! remain protocol-v1 aliases during rolling upgrades.
//!
//! Any other command is acked `unsupported` **without executing**, so a
//! malicious or buggy enqueue can never trigger arbitrary code on the device.
//! The allow-list is explicit in [`SystemExecutor`]; adding a command is a
//! deliberate code change here, not a server-side knob.
//!
//! ## Identity
//!
//! Reads `device_id` from `/etc/deputyos/account.json` (written by the wizard
//! Account step) and the device capability secret `backup_token` from
//! `/etc/deputyos/backup-token`. Both env-overridable for hermetic tests
//! (`DEPUTYOS_ACCOUNT_FILE`, `DEPUTYOS_BACKUP_TOKEN_FILE`). The API base is
//! `DEPUTYOS_API_BASE` or `https://api.deputyos.com`.

use std::time::Duration;

use anyhow::{anyhow, bail, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::{apibase, paths};

const DEFAULT_INTERVAL_SECS: u64 = 30;

/// One queued command, as returned by `GET .../commands/pending` and enqueued
/// by `POST .../commands`. Mirrors `DeviceCommand` in the sibling `fleet.rs`.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct DeviceCommand {
    pub id: String,
    pub command: String,
    #[serde(default)]
    pub params: serde_json::Value,
    #[serde(default)]
    pub status: String,
    #[serde(default)]
    pub created_at: String,
    #[serde(default)]
    pub expires_at: Option<String>,
    #[serde(default)]
    pub actor: Option<deputyd::ActorContext>,
    #[serde(default = "default_protocol")]
    pub protocol: u16,
}

fn default_protocol() -> u16 {
    1
}

/// The outcome of executing one command — what we POST back as the result.
#[derive(Debug, Clone)]
pub struct ExecOutcome {
    /// `"completed"` | `"failed"` | `"unsupported"`.
    pub status: String,
    pub result: serde_json::Value,
}

/// Executes a queued command. The trait lets tests stub execution so the
/// poll→execute→ack transport flow is unit-testable against an `httpmock`
/// origin without touching systemd or the active profile.
pub trait Executor {
    fn execute(
        &self,
        id: &str,
        command: &str,
        params: &serde_json::Value,
        actor: Option<&deputyd::ActorContext>,
    ) -> ExecOutcome;
}

/// The real executor translates cloud operation names to typed `deputyd`
/// commands; everything else is refused as `unsupported`.
pub struct SystemExecutor;

impl Executor for SystemExecutor {
    fn execute(
        &self,
        id: &str,
        command: &str,
        params: &serde_json::Value,
        actor: Option<&deputyd::ActorContext>,
    ) -> ExecOutcome {
        let agent_command = match queued_agent_command(command, params) {
            Ok(command) => command,
            Err(error) => {
                return ExecOutcome {
                    status: "unsupported".into(),
                    result: json!({ "error": error.to_string() }),
                }
            }
        };
        let mut request = deputyd::AgentRequest::new(id, agent_command);
        if let Some(actor) = actor {
            request = request.with_actor(actor.clone());
        }
        match deputyd::request(std::path::Path::new(deputyd::DEFAULT_SOCKET), &request)
            .and_then(deputyd::ensure_success)
        {
            Ok(result) => ExecOutcome {
                status: "completed".into(),
                result: serde_json::to_value(result)
                    .unwrap_or_else(|error| json!({"serialization_error": error.to_string()})),
            },
            Err(error) => ExecOutcome {
                status: "failed".into(),
                result: json!({ "error": format!("{error:#}") }),
            },
        }
    }
}

pub fn queued_agent_command(
    command: &str,
    params: &serde_json::Value,
) -> Result<deputyd::AgentCommand> {
    use deputyd::{AgentCommand as A, SnapshotAction as S, WorkloadAction as W};
    let bool_param = |key: &str| params.get(key).and_then(serde_json::Value::as_bool);
    let u64_param = |key: &str| params.get(key).and_then(serde_json::Value::as_u64);
    let u16_param = |key: &str| {
        u64_param(key)
            .map(u16::try_from)
            .transpose()
            .map_err(|_| anyhow!("{key} is out of range"))
    };
    Ok(match command {
        "agent.health" | "ping" => A::Health,
        "agent.capabilities" => A::Capabilities,
        "lifecycle.prepare_pause" => A::PreparePause,
        "lifecycle.resume" => A::Resume,
        "memory.reclaim" => A::Reclaim {
            drop_caches: bool_param("drop_caches").unwrap_or(false),
        },
        "workload.status" => A::Workload { action: W::Status },
        "workload.start" => A::Workload { action: W::Start },
        "workload.stop" => A::Workload { action: W::Stop },
        "workload.restart" | "restart-agent" => A::Workload { action: W::Restart },
        "workload.reconcile" => A::Workload {
            action: W::Reconcile,
        },
        "resources.set" => A::SetResources {
            memory_high_bytes: u64_param("memory_high_bytes"),
            memory_max_bytes: u64_param("memory_max_bytes"),
            cpu_quota_percent: u16_param("cpu_quota_percent")?,
            io_weight: u16_param("io_weight")?,
        },
        "snapshot.prepare" => A::Snapshot { action: S::Prepare },
        "snapshot.complete" => A::Snapshot {
            action: S::Complete,
        },
        "update.status" => A::UpdateStatus,
        "update.run" => A::UpdateRun,
        "repair.status" => A::Repair { run: false },
        "repair.run" => A::Repair { run: true },
        "network.status" => A::NetworkStatus,
        "tunnel.restart" => A::TunnelRestart,
        "storage.status" => A::StorageStatus,
        "backup.status" => A::Backup { run: false },
        "backup.run" => A::Backup { run: true },
        "logs.tail" => A::Logs {
            lines: u16_param("lines")?.unwrap_or(100),
        },
        "events.list" => A::Events {
            limit: u16_param("limit")?.unwrap_or(50),
        },
        other => bail!("unsupported command: {other}"),
    })
}

/// This device's queue identity: `device_id` (from `account.json`) + the
/// `backup_token` used as the Bearer credential for the pending/result routes.
#[derive(Debug, Clone)]
pub struct DeviceIdentity {
    pub device_id: String,
    pub backup_token: String,
}

/// Load the device identity from the appliance files. Errors clearly if the
/// device isn't registered yet (no account.json / no device_id) or the backup
/// token is missing/empty — the caller (systemd unit) surfaces that in the
/// journal and the unit retries per its `Restart=` policy.
pub fn load_identity() -> Result<DeviceIdentity> {
    let account_path = paths::account_file();
    let account_raw = std::fs::read_to_string(&account_path).with_context(|| {
        format!(
            "reading {} (device not registered?)",
            account_path.display()
        )
    })?;
    #[derive(Deserialize)]
    struct AccountLabel {
        #[serde(default)]
        device_id: Option<String>,
    }
    let label: AccountLabel = serde_json::from_str(&account_raw).context("parsing account.json")?;
    let device_id = label
        .device_id
        .filter(|s| !s.is_empty())
        .ok_or_else(|| anyhow!("account.json has no device_id (device not registered yet)"))?;

    let token_path = paths::cloud_backup_token_file();
    let backup_token = std::fs::read_to_string(&token_path)
        .with_context(|| format!("reading {}", token_path.display()))?
        .trim()
        .to_string();
    if backup_token.is_empty() {
        bail!("backup token at {} is empty", token_path.display());
    }
    Ok(DeviceIdentity {
        device_id,
        backup_token,
    })
}

/// The poller. Holds identity + an executor + an HTTP agent + the loop
/// interval. Construct via [`Poller::new`] (production) or build directly in
/// tests with a stub [`Executor`] and a mock base URL.
pub struct Poller {
    base_url: String,
    identity: DeviceIdentity,
    executor: Box<dyn Executor>,
    interval: Duration,
    agent: ureq::Agent,
}

impl Poller {
    /// Production constructor: real [`SystemExecutor`], identity from disk,
    /// `DEPUTYOS_API_BASE` override honored, 30s default interval.
    pub fn new(api_base: Option<&str>, interval_secs: Option<u64>) -> Result<Self> {
        let identity = load_identity()?;
        let base_url = apibase::resolve(api_base);
        let interval = Duration::from_secs(interval_secs.unwrap_or(DEFAULT_INTERVAL_SECS));
        let agent = ureq::AgentBuilder::new()
            .timeout_connect(Duration::from_secs(10))
            .timeout(Duration::from_secs(30))
            .build();
        Ok(Self {
            base_url,
            identity,
            executor: Box::new(SystemExecutor),
            interval,
            agent,
        })
    }

    fn pending_url(&self) -> String {
        format!(
            "{}/api/v1/devices/{}/commands/pending",
            self.base_url.trim_end_matches('/'),
            self.identity.device_id
        )
    }

    fn result_url(&self, cmd_id: &str) -> String {
        format!(
            "{}/api/v1/devices/{}/commands/{}/result",
            self.base_url.trim_end_matches('/'),
            self.identity.device_id,
            cmd_id
        )
    }

    /// `GET .../commands/pending` → the pending command list. An empty list,
    /// a 401 (bad/revoked token), or a transport error all surface as `Err`;
    /// the loop logs + backs off rather than spinning.
    pub fn fetch_pending(&self) -> Result<Vec<DeviceCommand>> {
        let resp = self
            .agent
            .get(&self.pending_url())
            .set(
                "Authorization",
                &format!("Bearer {}", self.identity.backup_token),
            )
            .call()
            .map_err(|e| map_ureq_err("fetching pending commands", e))?;
        let commands: Vec<DeviceCommand> = resp
            .into_json()
            .context("decoding pending commands response")?;
        Ok(commands)
    }

    /// `POST .../commands/:cmd_id/result` with the outcome. Best-effort: a
    /// failed ack is logged (the command stays pending server-side and will
    /// be re-delivered next poll, which is the desired at-least-once shape).
    pub fn ack(&self, cmd_id: &str, outcome: &ExecOutcome) -> Result<()> {
        let body = json!({ "result": outcome.result, "status": outcome.status });
        self.agent
            .post(&self.result_url(cmd_id))
            .set(
                "Authorization",
                &format!("Bearer {}", self.identity.backup_token),
            )
            .send_json(body)
            .map_err(|e| map_ureq_err("acking command result", e))?;
        Ok(())
    }

    /// One drain pass: fetch pending, execute + ack each. Returns the count
    /// drained. Errors short-circuit only on a *fetch* failure (we can't
    /// execute what we can't read); per-command execution/ack failures are
    /// logged and counted as drained so one bad command doesn't kill the loop.
    pub fn drain_once(&mut self) -> Result<usize> {
        let pending = self.fetch_pending()?;
        let n = pending.len();
        for cmd in pending {
            let outcome = if !(1..=deputyd::PROTOCOL_VERSION).contains(&cmd.protocol) {
                ExecOutcome {
                    status: "unsupported".into(),
                    result: json!({
                        "error": format!(
                            "unsupported resident-agent protocol {}",
                            cmd.protocol
                        )
                    }),
                }
            } else if command_expired(&cmd) {
                ExecOutcome {
                    status: "expired".into(),
                    result: json!({"error": "command expired before delivery"}),
                }
            } else {
                self.executor
                    .execute(&cmd.id, &cmd.command, &cmd.params, cmd.actor.as_ref())
            };
            let status = outcome.status.clone();
            if let Err(e) = self.ack(&cmd.id, &outcome) {
                tracing::warn!(command_id = %cmd.id, command = %cmd.command, error = %e, "ack failed");
            } else {
                tracing::info!(command_id = %cmd.id, command = %cmd.command, status = %status, "command drained");
            }
        }
        Ok(n)
    }

    /// Loop forever: drain, sleep, repeat. Fetch failures are logged + the
    /// loop backs off for `interval` (avoids a tight spin on a down API).
    pub fn run_forever(&mut self) -> Result<()> {
        loop {
            match self.drain_once() {
                Ok(0) => tracing::debug!("no pending commands"),
                Ok(n) => tracing::info!(count = n, "drained commands"),
                Err(e) => tracing::warn!(error = %e, "poll pass failed; backing off"),
            }
            std::thread::sleep(self.interval);
        }
    }
}

fn command_expired(command: &DeviceCommand) -> bool {
    let Some(expires_at) = command
        .expires_at
        .as_deref()
        .and_then(|value| value.parse::<u64>().ok())
    else {
        return false;
    };
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(0);
    now >= expires_at
}

/// Map a `ureq::Error` to an anyhow with context. Status errors (4xx/5xx)
/// carry the body so a 401 surfaces as "… 401 Unauthorized: <body>".
fn map_ureq_err(ctx: &str, e: ureq::Error) -> anyhow::Error {
    match e {
        ureq::Error::Status(code, resp) => {
            let body = resp.into_string().unwrap_or_default();
            anyhow!("{ctx}: HTTP {code}: {}", body.trim())
        }
        other => anyhow!("{ctx}: {other}"),
    }
}

/// Resolve the API base for the CLI: `--api-base` flag > `DEPUTYOS_API_BASE`
/// env > `/etc/deputyos/api-base` > the compiled default. See [`apibase::resolve`]
/// — the command poller honors a custom backend persisted by the wizard.
pub fn resolve_api_base(flag: Option<&str>) -> String {
    apibase::resolve(flag)
}

/// Entry point for `deputyctl commands poll --once`. Drains a single pass and
/// prints a one-line summary; nonzero exit on a fetch failure so the systemd
/// unit's `Restart=` policy surfaces a flaky API.
pub fn run_once(api_base: Option<&str>) -> Result<u8> {
    let mut poller = Poller::new(api_base, None)?;
    match poller.drain_once() {
        Ok(n) => {
            println!("commands: drained {n} pending command(s)");
            Ok(0)
        }
        Err(e) => {
            eprintln!("commands: {e:#}");
            Ok(1)
        }
    }
}

/// Entry point for `deputyctl commands poll` (loop mode) — the systemd unit's
/// `ExecStart`. Runs until killed by systemd.
pub fn run_loop(api_base: Option<&str>, interval_secs: Option<u64>) -> Result<u8> {
    let mut poller = Poller::new(api_base, interval_secs)?;
    poller.run_forever().context("command poller loop exited")?;
    Ok(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use httpmock::MockServer;
    use std::sync::{Arc, Mutex};

    /// A stub executor that records the commands it saw + returns a canned
    /// outcome per command name. Lets us assert the poll→exec→ack flow
    /// against an httpmock origin without touching systemd.
    struct FakeExecutor {
        seen: Arc<Mutex<Vec<(String, serde_json::Value)>>>,
    }
    impl Executor for FakeExecutor {
        fn execute(
            &self,
            _id: &str,
            command: &str,
            params: &serde_json::Value,
            _actor: Option<&deputyd::ActorContext>,
        ) -> ExecOutcome {
            self.seen
                .lock()
                .expect("exec lock")
                .push((command.to_string(), params.clone()));
            match command {
                "ping" => ExecOutcome {
                    status: "completed".into(),
                    result: json!({ "pong": true }),
                },
                "boom" => ExecOutcome {
                    status: "failed".into(),
                    result: json!({ "error": "kaboom" }),
                },
                other => ExecOutcome {
                    status: "unsupported".into(),
                    result: json!({ "error": format!("unsupported: {other}") }),
                },
            }
        }
    }

    fn poller_against(server: &MockServer, exec: FakeExecutor) -> Poller {
        Poller {
            base_url: server.base_url(),
            identity: DeviceIdentity {
                device_id: "dev-123".into(),
                backup_token: "tok-secret".into(),
            },
            executor: Box::new(exec),
            interval: Duration::from_millis(1),
            agent: ureq::AgentBuilder::new()
                .timeout_connect(Duration::from_secs(5))
                .timeout(Duration::from_secs(15))
                .build(),
        }
    }

    #[test]
    fn fetch_pending_sets_bearer_backup_token() {
        let s = MockServer::start();
        s.mock(|w, t| {
            w.method(httpmock::Method::GET)
                .path("/api/v1/devices/dev-123/commands/pending")
                .header("Authorization", "Bearer tok-secret");
            t.status(200).json_body(serde_json::json!([
                { "id": "c1", "command": "ping", "params": {}, "status": "pending", "created_at": "1" }
            ]));
        });
        let p = poller_against(
            &s,
            FakeExecutor {
                seen: Arc::new(Mutex::new(vec![])),
            },
        );
        let cmds = p.fetch_pending().expect("fetch ok");
        assert_eq!(cmds.len(), 1);
        assert_eq!(cmds[0].id, "c1");
        assert_eq!(cmds[0].command, "ping");
    }

    #[test]
    fn drain_once_executes_and_acks_each_command() {
        let s = MockServer::start();
        s.mock(|w, t| {
            w.method(httpmock::Method::GET)
                .path("/api/v1/devices/dev-123/commands/pending");
            t.status(200).json_body(serde_json::json!([
                { "id": "c1", "command": "ping", "params": {"k": 1}, "status": "pending", "created_at": "1" },
                { "id": "c2", "command": "boom", "params": null, "status": "pending", "created_at": "2" },
                { "id": "c3", "command": "wat", "params": {}, "status": "pending", "created_at": "3" }
            ]));
        });
        let ack1 = s.mock(|w, t| {
            w.method(httpmock::Method::POST)
                .path("/api/v1/devices/dev-123/commands/c1/result")
                .header("Authorization", "Bearer tok-secret");
            t.status(200).body("");
        });
        let ack2 = s.mock(|w, t| {
            w.method(httpmock::Method::POST)
                .path("/api/v1/devices/dev-123/commands/c2/result");
            t.status(200).body("");
        });
        let ack3 = s.mock(|w, t| {
            w.method(httpmock::Method::POST)
                .path("/api/v1/devices/dev-123/commands/c3/result");
            t.status(200).body("");
        });
        let seen = Arc::new(Mutex::new(vec![]));
        let mut p = poller_against(&s, FakeExecutor { seen: seen.clone() });
        let n = p.drain_once().expect("drain ok");
        assert_eq!(n, 3);
        // Each command was executed exactly once with its params.
        let seen = seen.lock().expect("seen lock").clone();
        assert_eq!(seen.len(), 3);
        assert_eq!(seen[0].0, "ping");
        assert_eq!(seen[0].1, json!({"k": 1}));
        assert_eq!(seen[1].0, "boom");
        assert_eq!(seen[2].0, "wat");
        // Each ack hit the server with the right status body.
        ack1.assert_hits(1);
        ack2.assert_hits(1);
        ack3.assert_hits(1);
    }

    #[test]
    fn ack_body_carries_status_and_result() {
        let s = MockServer::start();
        let ack = s.mock(|w, t| {
            w.method(httpmock::Method::POST)
                .path("/api/v1/devices/dev-123/commands/c1/result")
                .header("Authorization", "Bearer tok-secret")
                .json_body(serde_json::json!({
                    "result": { "pong": true },
                    "status": "completed"
                }));
            t.status(200).body("");
        });
        let p = poller_against(
            &s,
            FakeExecutor {
                seen: Arc::new(Mutex::new(vec![])),
            },
        );
        p.ack(
            "c1",
            &ExecOutcome {
                status: "completed".into(),
                result: json!({ "pong": true }),
            },
        )
        .expect("ack ok");
        ack.assert_hits(1);
    }

    #[test]
    fn fetch_pending_401_surfaces_as_error() {
        let s = MockServer::start();
        s.mock(|w, t| {
            w.method(httpmock::Method::GET)
                .path("/api/v1/devices/dev-123/commands/pending");
            t.status(401).body("unauthorized");
        });
        let p = poller_against(
            &s,
            FakeExecutor {
                seen: Arc::new(Mutex::new(vec![])),
            },
        );
        let err = p.fetch_pending().expect_err("should error");
        let msg = format!("{err:#}");
        assert!(msg.contains("401"), "msg: {msg}");
    }

    #[test]
    fn drain_once_empty_pending_is_zero() {
        let s = MockServer::start();
        s.mock(|w, t| {
            w.method(httpmock::Method::GET)
                .path("/api/v1/devices/dev-123/commands/pending");
            t.status(200).json_body(serde_json::json!([]));
        });
        let mut p = poller_against(
            &s,
            FakeExecutor {
                seen: Arc::new(Mutex::new(vec![])),
            },
        );
        assert_eq!(p.drain_once().expect("drain ok"), 0);
    }

    #[test]
    fn future_protocol_is_acked_unsupported_without_execution() {
        let server = MockServer::start();
        server.mock(|when, then| {
            when.method(httpmock::Method::GET)
                .path("/api/v1/devices/dev-123/commands/pending");
            then.status(200).json_body(serde_json::json!([{
                "id": "future-1",
                "protocol": 99,
                "command": "workload.restart",
                "params": {},
                "status": "pending",
                "created_at": "1"
            }]));
        });
        let ack = server.mock(|when, then| {
            when.method(httpmock::Method::POST)
                .path("/api/v1/devices/dev-123/commands/future-1/result")
                .json_body_partial(r#"{"status":"unsupported"}"#);
            then.status(200);
        });
        let seen = Arc::new(Mutex::new(vec![]));
        let mut poller = poller_against(&server, FakeExecutor { seen: seen.clone() });
        assert_eq!(poller.drain_once().expect("drain"), 1);
        assert!(seen.lock().expect("seen").is_empty());
        ack.assert_hits(1);
    }

    #[test]
    fn ping_maps_to_typed_health_command() {
        assert_eq!(
            queued_agent_command("ping", &json!({})).expect("mapping"),
            deputyd::AgentCommand::Health
        );
    }

    #[test]
    fn system_executor_unknown_command_is_unsupported_not_executed() {
        let error =
            queued_agent_command("rm-rf-something", &json!(null)).expect_err("must not map");
        assert!(error.to_string().contains("unsupported"));
    }

    #[test]
    fn resource_command_is_typed_and_range_checked() {
        let command = queued_agent_command(
            "resources.set",
            &json!({"memory_max_bytes": 1073741824_u64, "cpu_quota_percent": 125}),
        )
        .expect("resource mapping");
        assert_eq!(
            command,
            deputyd::AgentCommand::SetResources {
                memory_high_bytes: None,
                memory_max_bytes: Some(1_073_741_824),
                cpu_quota_percent: Some(125),
                io_weight: None,
            }
        );
    }

    #[test]
    fn resolve_api_base_flag_wins_over_env() {
        let _g = crate::env_mutex().lock().unwrap_or_else(|e| e.into_inner());
        std::env::set_var("DEPUTYOS_API_BASE", "https://env.test");
        assert_eq!(
            resolve_api_base(Some("https://flag.test")),
            "https://flag.test"
        );
        assert_eq!(resolve_api_base(None), "https://env.test");
        std::env::remove_var("DEPUTYOS_API_BASE");
        std::env::remove_var("DEPUTYOS_API_BASE_FILE");
        assert_eq!(resolve_api_base(None), apibase::DEFAULT_API_BASE);
    }

    #[test]
    fn load_identity_reads_device_id_and_token_from_env_paths() {
        let _g = crate::env_mutex().lock().unwrap_or_else(|e| e.into_inner());
        let dir = tempfile::tempdir().expect("tempdir");
        std::env::set_var(
            "DEPUTYOS_ACCOUNT_FILE",
            dir.path().join("account.json").to_str().expect("utf8"),
        );
        std::env::set_var(
            "DEPUTYOS_BACKUP_TOKEN_FILE",
            dir.path().join("backup-token").to_str().expect("utf8"),
        );
        std::fs::write(
            dir.path().join("account.json"),
            serde_json::json!({ "device_id": "dev-abc", "registered": true }).to_string(),
        )
        .expect("write account");
        std::fs::write(dir.path().join("backup-token"), "tok-abc\n").expect("write token");
        let id = load_identity().expect("identity ok");
        assert_eq!(id.device_id, "dev-abc");
        assert_eq!(id.backup_token, "tok-abc");
        std::env::remove_var("DEPUTYOS_ACCOUNT_FILE");
        std::env::remove_var("DEPUTYOS_BACKUP_TOKEN_FILE");
    }

    #[test]
    fn load_identity_errors_when_device_id_missing() {
        let _g = crate::env_mutex().lock().unwrap_or_else(|e| e.into_inner());
        let dir = tempfile::tempdir().expect("tempdir");
        std::env::set_var(
            "DEPUTYOS_ACCOUNT_FILE",
            dir.path().join("account.json").to_str().expect("utf8"),
        );
        std::env::set_var(
            "DEPUTYOS_BACKUP_TOKEN_FILE",
            dir.path().join("backup-token").to_str().expect("utf8"),
        );
        std::fs::write(
            dir.path().join("account.json"),
            serde_json::json!({}).to_string(),
        )
        .expect("write account");
        std::fs::write(dir.path().join("backup-token"), "tok\n").expect("write token");
        let err = load_identity().expect_err("no device_id should error");
        assert!(format!("{err:#}").contains("device_id"));
        std::env::remove_var("DEPUTYOS_ACCOUNT_FILE");
        std::env::remove_var("DEPUTYOS_BACKUP_TOKEN_FILE");
    }

    #[test]
    fn load_identity_errors_when_token_empty() {
        let _g = crate::env_mutex().lock().unwrap_or_else(|e| e.into_inner());
        let dir = tempfile::tempdir().expect("tempdir");
        std::env::set_var(
            "DEPUTYOS_ACCOUNT_FILE",
            dir.path().join("account.json").to_str().expect("utf8"),
        );
        std::env::set_var(
            "DEPUTYOS_BACKUP_TOKEN_FILE",
            dir.path().join("backup-token").to_str().expect("utf8"),
        );
        std::fs::write(
            dir.path().join("account.json"),
            serde_json::json!({ "device_id": "dev-x" }).to_string(),
        )
        .expect("write account");
        std::fs::write(dir.path().join("backup-token"), "  \n").expect("write empty token");
        let err = load_identity().expect_err("empty token should error");
        assert!(format!("{err:#}").contains("empty"));
        std::env::remove_var("DEPUTYOS_ACCOUNT_FILE");
        std::env::remove_var("DEPUTYOS_BACKUP_TOKEN_FILE");
    }
}
