//! Minimal QMP and QEMU Guest Agent clients used by the Linux backend.
//!
//! Both protocols are newline-delimited JSON over per-instance Unix sockets.
//! The public runtime API remains typed: QGA may execute only `/usr/local/bin/
//! deputyd` with arguments derived from [`deputyd::AgentCommand`].

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{anyhow, bail, Context, Result};
use base64::Engine;
use serde_json::{json, Value};

const IO_TIMEOUT: Duration = Duration::from_secs(5);
const GUEST_EXEC_TIMEOUT: Duration = Duration::from_secs(15);
const BALLOON_TIMEOUT: Duration = Duration::from_secs(5);

pub struct QmpClient {
    reader: BufReader<UnixStream>,
    writer: UnixStream,
}

impl QmpClient {
    pub fn connect(path: &Path) -> Result<Self> {
        let writer = connect(path, "QMP")?;
        let reader = BufReader::new(writer.try_clone().context("cloning QMP socket")?);
        let mut client = Self { reader, writer };
        let greeting = client.read_value().context("reading QMP greeting")?;
        if greeting.get("QMP").is_none() {
            bail!("invalid QMP greeting: {greeting}");
        }
        client
            .execute_with_id("qmp_capabilities", None, "capabilities")
            .context("negotiating QMP capabilities")?;
        Ok(client)
    }

    pub fn execute(&mut self, command: &str, arguments: Option<Value>) -> Result<Value> {
        self.execute_with_id(command, arguments, command)
    }

    pub fn status(&mut self) -> Result<String> {
        let response = self.execute("query-status", None)?;
        response
            .get("status")
            .and_then(Value::as_str)
            .map(str::to_string)
            .ok_or_else(|| anyhow!("query-status response omitted status: {response}"))
    }

    pub fn pause(&mut self) -> Result<()> {
        self.execute("stop", None)?;
        Ok(())
    }

    pub fn resume(&mut self) -> Result<()> {
        self.execute("cont", None)?;
        Ok(())
    }

    pub fn set_balloon_mib(&mut self, target_mib: u64) -> Result<()> {
        let target_bytes = target_mib
            .checked_mul(1024 * 1024)
            .ok_or_else(|| anyhow!("memory target overflow"))?;
        self.execute("balloon", Some(json!({ "value": target_bytes })))?;

        // Ballooning is cooperative and asynchronous. Wait briefly for the
        // guest driver, but do not fail solely because an older QEMU omits
        // query-balloon after accepting the target.
        let deadline = Instant::now() + BALLOON_TIMEOUT;
        while Instant::now() < deadline {
            let Ok(response) = self.execute("query-balloon", None) else {
                return Ok(());
            };
            if response
                .get("actual")
                .and_then(Value::as_u64)
                .is_some_and(|actual| actual <= target_bytes + 16 * 1024 * 1024)
            {
                return Ok(());
            }
            thread::sleep(Duration::from_millis(100));
        }
        // The target remains active even if the guest has not released every
        // requested page yet. Host pressure supervision can inspect it again.
        Ok(())
    }

    fn execute_with_id(
        &mut self,
        command: &str,
        arguments: Option<Value>,
        id: &str,
    ) -> Result<Value> {
        let mut request = json!({ "execute": command, "id": id });
        if let Some(arguments) = arguments {
            request["arguments"] = arguments;
        }
        write_json(&mut self.writer, &request)?;
        loop {
            let response = self.read_value()?;
            if response.get("id").and_then(Value::as_str) != Some(id) {
                // Asynchronous event; keep waiting for this command's reply.
                continue;
            }
            if let Some(error) = response.get("error") {
                bail!("QMP {command} failed: {error}");
            }
            return response
                .get("return")
                .cloned()
                .ok_or_else(|| anyhow!("QMP {command} response omitted return: {response}"));
        }
    }

    fn read_value(&mut self) -> Result<Value> {
        read_json_line(&mut self.reader)
    }
}

pub fn guest_command(path: &Path, command: deputyd::AgentCommand) -> Result<deputyd::AgentResult> {
    let mut stream = connect(path, "QEMU guest agent")?;
    // guest-sync-delimited flushes any stale partial response left by a
    // previous client and proves the channel is alive.
    qga_request(
        &mut stream,
        "guest-sync-delimited",
        Some(json!({ "id": 0x4450_5459_u64 })),
        "sync",
    )?;

    let args = deputyd_args(command);
    let response = qga_request(
        &mut stream,
        "guest-exec",
        Some(json!({
            "path": "/usr/local/bin/deputyd",
            "arg": args,
            "capture-output": true
        })),
        "exec",
    )?;
    let pid = response
        .get("pid")
        .and_then(Value::as_i64)
        .ok_or_else(|| anyhow!("guest-exec response omitted pid: {response}"))?;

    let deadline = Instant::now() + GUEST_EXEC_TIMEOUT;
    loop {
        if Instant::now() >= deadline {
            bail!("timed out waiting for deputyd guest execution");
        }
        let status = qga_request(
            &mut stream,
            "guest-exec-status",
            Some(json!({ "pid": pid })),
            "status",
        )?;
        if !status
            .get("exited")
            .and_then(Value::as_bool)
            .unwrap_or(false)
        {
            thread::sleep(Duration::from_millis(50));
            continue;
        }
        let exitcode = status.get("exitcode").and_then(Value::as_i64).unwrap_or(-1);
        let stdout = decode_capture(status.get("out-data"))?;
        let stderr = decode_capture(status.get("err-data"))?;
        if exitcode != 0 {
            bail!(
                "deputyd guest command exited {exitcode}: {}",
                String::from_utf8_lossy(&stderr).trim()
            );
        }
        return serde_json::from_slice(&stdout).with_context(|| {
            format!(
                "decoding deputyd output: {}",
                String::from_utf8_lossy(&stdout)
            )
        });
    }
}

fn qga_request(
    stream: &mut UnixStream,
    command: &str,
    arguments: Option<Value>,
    id: &str,
) -> Result<Value> {
    let mut request = json!({ "execute": command, "id": id });
    if let Some(arguments) = arguments {
        request["arguments"] = arguments;
    }
    write_json(stream, &request)?;
    let mut reader = BufReader::new(stream.try_clone().context("cloning QGA socket")?);
    loop {
        let response = read_json_line(&mut reader)?;
        if response.get("id").and_then(Value::as_str) != Some(id) {
            continue;
        }
        if let Some(error) = response.get("error") {
            bail!("QEMU guest agent {command} failed: {error}");
        }
        return response
            .get("return")
            .cloned()
            .ok_or_else(|| anyhow!("QGA {command} response omitted return: {response}"));
    }
}

fn deputyd_args(command: deputyd::AgentCommand) -> Vec<String> {
    match command {
        deputyd::AgentCommand::Health => vec!["health".to_string()],
        deputyd::AgentCommand::PreparePause => vec!["prepare-pause".to_string()],
        deputyd::AgentCommand::Resume => vec!["resume".to_string()],
        deputyd::AgentCommand::Reclaim { drop_caches } => {
            let mut args = vec!["reclaim".to_string()];
            if drop_caches {
                args.push("--drop-caches".to_string());
            }
            args
        }
        other => vec![
            "execute".to_string(),
            "--request-json".to_string(),
            serde_json::to_string(&other).expect("serializing typed deputyd command"),
        ],
    }
}

fn connect(path: &Path, label: &str) -> Result<UnixStream> {
    let stream = UnixStream::connect(path)
        .with_context(|| format!("connecting {label} {}", path.display()))?;
    stream
        .set_read_timeout(Some(IO_TIMEOUT))
        .context("setting socket read timeout")?;
    stream
        .set_write_timeout(Some(IO_TIMEOUT))
        .context("setting socket write timeout")?;
    Ok(stream)
}

fn write_json(stream: &mut UnixStream, value: &Value) -> Result<()> {
    serde_json::to_writer(&mut *stream, value).context("encoding JSON protocol request")?;
    stream
        .write_all(b"\n")
        .context("terminating JSON request")?;
    stream.flush().context("flushing JSON request")
}

fn read_json_line(reader: &mut BufReader<UnixStream>) -> Result<Value> {
    loop {
        let mut line = Vec::new();
        let count = reader
            .read_until(b'\n', &mut line)
            .context("reading JSON protocol response")?;
        if count == 0 {
            bail!("control socket closed before a response");
        }
        let mut trimmed = line.as_slice().trim_ascii();
        if let Some(without_delimiter) = trimmed.strip_prefix(&[0xff]) {
            trimmed = without_delimiter.trim_ascii();
        }
        if trimmed.is_empty() {
            continue;
        }
        return serde_json::from_slice(trimmed).with_context(|| {
            format!(
                "decoding JSON protocol response: {}",
                String::from_utf8_lossy(trimmed)
            )
        });
    }
}

fn decode_capture(value: Option<&Value>) -> Result<Vec<u8>> {
    let Some(encoded) = value.and_then(Value::as_str) else {
        return Ok(Vec::new());
    };
    base64::engine::general_purpose::STANDARD
        .decode(encoded)
        .context("decoding QGA captured output")
}

#[cfg(test)]
mod tests {
    use std::os::unix::net::UnixListener;
    use std::thread;

    use super::*;

    fn read_request(reader: &mut BufReader<UnixStream>) -> Value {
        let mut line = String::new();
        reader.read_line(&mut line).expect("read request");
        serde_json::from_str(&line).expect("request JSON")
    }

    #[test]
    fn resident_command_arguments_are_allow_listed() {
        assert_eq!(deputyd_args(deputyd::AgentCommand::Health), vec!["health"]);
        assert_eq!(
            deputyd_args(deputyd::AgentCommand::PreparePause),
            vec!["prepare-pause"]
        );
        assert_eq!(
            deputyd_args(deputyd::AgentCommand::Reclaim { drop_caches: true }),
            vec!["reclaim", "--drop-caches"]
        );
    }

    #[test]
    fn capture_decoder_handles_base64_and_missing_values() {
        let value = Value::String("eyJvayI6dHJ1ZX0=".to_string());
        assert_eq!(
            decode_capture(Some(&value)).expect("decode"),
            br#"{"ok":true}"#
        );
        assert!(decode_capture(None).expect("missing").is_empty());
    }

    #[test]
    fn qmp_negotiates_and_reads_running_status() {
        let dir = tempfile::tempdir().expect("tempdir");
        let socket = dir.path().join("qmp.sock");
        let listener = UnixListener::bind(&socket).expect("bind");
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept");
            stream
                .write_all(b"{\"QMP\":{\"version\":{},\"capabilities\":[]}}\n")
                .expect("greeting");
            let mut reader = BufReader::new(stream.try_clone().expect("clone"));
            let capabilities = read_request(&mut reader);
            assert_eq!(capabilities["execute"], "qmp_capabilities");
            stream
                .write_all(b"{\"return\":{},\"id\":\"capabilities\"}\n")
                .expect("capabilities response");
            let status = read_request(&mut reader);
            assert_eq!(status["execute"], "query-status");
            stream
                .write_all(
                    b"{\"return\":{\"running\":true,\"status\":\"running\"},\"id\":\"query-status\"}\n",
                )
                .expect("status response");
        });

        let mut client = QmpClient::connect(&socket).expect("QMP client");
        assert_eq!(client.status().expect("status"), "running");
        server.join().expect("server");
    }

    #[test]
    fn qga_executes_only_deputyd_and_decodes_result() {
        let dir = tempfile::tempdir().expect("tempdir");
        let socket = dir.path().join("qga.sock");
        let listener = UnixListener::bind(&socket).expect("bind");
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept");
            let mut reader = BufReader::new(stream.try_clone().expect("clone"));

            let sync = read_request(&mut reader);
            assert_eq!(sync["execute"], "guest-sync-delimited");
            stream
                .write_all(b"\xff{\"return\":1146115161,\"id\":\"sync\"}\n")
                .expect("sync response");

            let exec = read_request(&mut reader);
            assert_eq!(exec["execute"], "guest-exec");
            assert_eq!(exec["arguments"]["path"], "/usr/local/bin/deputyd");
            assert_eq!(exec["arguments"]["arg"], json!(["health"]));
            stream
                .write_all(b"{\"return\":{\"pid\":7},\"id\":\"exec\"}\n")
                .expect("exec response");

            let status = read_request(&mut reader);
            assert_eq!(status["execute"], "guest-exec-status");
            let result = deputyd::AgentResult::Health {
                report: deputyd::HealthReport {
                    agent_version: "test".to_string(),
                    protocol: deputyd::PROTOCOL_VERSION,
                    lifecycle: deputyd::LifecycleState::default(),
                    memory: deputyd::MemoryReport::default(),
                    uptime_seconds: None,
                    memory_pressure: None,
                    active_profile: Some("openclaw".to_string()),
                },
            };
            let output = serde_json::to_vec(&result).expect("agent result");
            let encoded = base64::engine::general_purpose::STANDARD.encode(output);
            let response = json!({
                "return": {
                    "exited": true,
                    "exitcode": 0,
                    "out-data": encoded
                },
                "id": "status"
            });
            serde_json::to_writer(&mut stream, &response).expect("status response");
            stream.write_all(b"\n").expect("newline");
        });

        let result = guest_command(&socket, deputyd::AgentCommand::Health).expect("guest command");
        let deputyd::AgentResult::Health { report } = result else {
            panic!("expected health result");
        };
        assert_eq!(report.active_profile.as_deref(), Some("openclaw"));
        server.join().expect("server");
    }
}
