//! In-process Unix-domain-socket relay that fires `pre-message` /
//! `post-message` / `cost-alert` (and, for symmetry, `update-applied`)
//! hooks on behalf of an external agent process.
//!
//! ## Why a relay?
//!
//! The agent processes shipped by deputyOS (OpenClaw — Node, Hermes — Python)
//! live outside this Rust crate. They need to invoke the dispatcher
//! ([`crate::hooks::fire_hook`]) at message boundaries. Three options were
//! considered:
//!
//! 1. **Shell out to `deputyctl message-relay --kind pre-message`.** Adds
//!    fork/exec latency on every message — unacceptable at chat cadence.
//! 2. **Unix-domain socket; deputyctl serves.** Sub-millisecond round trip,
//!    no per-message process spawn, language-agnostic — any client that
//!    speaks line-delimited JSON works. *Picked.*
//! 3. **Embed a Rust stub via Node-NAPI / PyO3.** Tightest coupling, but
//!    forces every profile to build native extensions. Rejected.
//!
//! ## Wire protocol (contract; see `docs/02-profiles.md` for profile authors)
//!
//! - **Transport:** `SOCK_STREAM` Unix-domain socket; default path
//!   `/run/deputyos/relay.sock` (overridable by the `--internal-run-relay
//!   <PATH>` flag).
//! - **Encoding:** newline-delimited UTF-8 JSON. One request per connection
//!   keeps reasoning trivial; the agent reconnects per event (cheap on
//!   the same host).
//! - **Request:** `{"kind": "pre-message"|"post-message"|"cost-alert"|
//!   "update-applied", "payload": <object>}`
//! - **Response:** `{"ok": <bool>, "errors": [{"script": "<path>",
//!   "code": <int>, "stderr_tail": "<string>"}, …]}`. `ok` is `true` iff
//!   every fired hook exited zero (or no hooks were installed).
//! - **Errors during parse:** `{"ok": false, "errors":
//!   [{"reason": "<message>"}]}` — connection then closes.
//! - **Hook payload schemas:** see `deputyctl/etc/hook-payload-schemas.json`.
//!
//! The `deputyctl` binary exposes the server side via a hidden top-level
//! flag `--internal-run-relay <SOCKET>` so it does not pollute `--help`
//! and is not part of the frozen surface (`docs/02-profiles.md`). Not a
//! subcommand.

use std::path::{Path, PathBuf};

#[cfg(unix)]
use std::io::{BufRead, BufReader, Write};
#[cfg(unix)]
use std::os::unix::net::{UnixListener, UnixStream};

#[cfg(unix)]
use anyhow::Context;
use anyhow::Result;
#[cfg(unix)]
use serde::{Deserialize, Serialize};

#[cfg(unix)]
use crate::hooks::{self, HookKind, HookStatus};
#[cfg(unix)]
use crate::paths;

/// Default socket path used when the agent and relay aren't co-configured.
/// Lives under `/run/deputyos/` to match the systemd-managed runtime dir.
pub fn default_socket_path() -> PathBuf {
    if let Ok(p) = std::env::var("DEPUTYOS_RELAY_SOCKET") {
        return PathBuf::from(p);
    }
    PathBuf::from("/run/deputyos/relay.sock")
}

/// Serve the relay on `socket_path` until the process is killed.
///
/// Each accepted connection is read as a single newline-terminated JSON
/// request, dispatched to [`crate::hooks::fire_hook_in_collect`] against
/// the configured hooks dir, and replied to with one JSON line. Connection
/// then closes.
///
/// One connection at a time is handled inline — this matches the agent's
/// expected cadence (a handful of events per message) and keeps the relay
/// dependency-free (no async runtime). If the load profile changes, this
/// is the natural place to add a thread pool.
#[cfg(unix)]
pub fn run_relay(socket_path: &Path) -> Result<()> {
    let listener = bind(socket_path)?;
    tracing::info!(socket = %socket_path.display(), "relay listening");
    serve_loop(listener);
    Ok(())
}

/// The appliance relay uses Unix-domain sockets and is unavailable on Windows.
#[cfg(not(unix))]
pub fn run_relay(_socket_path: &Path) -> Result<()> {
    anyhow::bail!("message relay requires Unix-domain sockets")
}

/// Bind a `UnixListener` at `socket_path`, removing any stale socket file
/// first. Permissions: 0600 — only the owner (the agent service user) can
/// connect. Caller is responsible for ensuring the parent dir exists.
#[cfg(unix)]
pub fn bind(socket_path: &Path) -> Result<UnixListener> {
    if let Some(parent) = socket_path.parent() {
        if !parent.as_os_str().is_empty() && !parent.is_dir() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("create socket parent {}", parent.display()))?;
        }
    }
    // Best-effort cleanup: any previous run that crashed leaves a stale
    // file; bind() would fail with EADDRINUSE otherwise.
    let _ = std::fs::remove_file(socket_path);
    let listener = UnixListener::bind(socket_path)
        .with_context(|| format!("bind {}", socket_path.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let perm = std::fs::Permissions::from_mode(0o600);
        let _ = std::fs::set_permissions(socket_path, perm);
    }
    Ok(listener)
}

/// Drive an already-bound listener. Split out so tests can supply their own
/// listener bound to a tempdir socket.
#[cfg(unix)]
pub fn serve_loop(listener: UnixListener) {
    for stream in listener.incoming() {
        match stream {
            Ok(s) => {
                if let Err(e) = handle_connection(s) {
                    tracing::warn!(err = %e, "relay connection failed");
                }
            }
            Err(e) => {
                tracing::warn!(err = %e, "relay accept failed");
            }
        }
    }
}

/// Handle one connection: read a single JSON request line, fire the
/// matching hook directory, write a single JSON response line, close.
#[cfg(unix)]
fn handle_connection(stream: UnixStream) -> Result<()> {
    // Wrap once for a buffered read; clone the underlying socket for the
    // write half so both halves of the connection can be used independently.
    let write_half = stream.try_clone().context("clone unix stream")?;
    let mut reader = BufReader::new(stream);
    let mut writer = write_half;

    let mut line = String::new();
    let n = reader.read_line(&mut line).context("read request")?;
    if n == 0 {
        // Empty connection — nothing to do.
        return Ok(());
    }

    let resp = match serde_json::from_str::<RelayRequest>(line.trim()) {
        Ok(req) => process(req),
        Err(e) => RelayResponse::parse_error(format!("invalid JSON request: {e}")),
    };
    let mut out = serde_json::to_string(&resp).unwrap_or_else(|_| {
        r#"{"ok":false,"errors":[{"reason":"response serialization failed"}]}"#.to_string()
    });
    out.push('\n');
    writer.write_all(out.as_bytes()).context("write response")?;
    writer.flush().ok();
    Ok(())
}

/// Map a parsed [`RelayRequest`] to the matching hooks dir + dispatch.
#[cfg(unix)]
fn process(req: RelayRequest) -> RelayResponse {
    let kind = match HookKind::parse(&req.kind) {
        Some(k) => k,
        None => {
            return RelayResponse::parse_error(format!(
                "unknown hook kind {:?}; want one of: pre-message, post-message, cost-alert, update-applied",
                req.kind
            ));
        }
    };
    let dir = paths::hooks_dir().join(kind.dir_name());
    let payload = req
        .payload
        .unwrap_or(serde_json::Value::Object(Default::default()));
    let results = hooks::fire_hook_in_collect(&dir, &payload);
    RelayResponse::from_results(&results)
}

#[cfg(unix)]
#[derive(Debug, Deserialize)]
struct RelayRequest {
    kind: String,
    /// Optional — the dispatcher tolerates an empty `{}` payload.
    payload: Option<serde_json::Value>,
}

#[cfg(unix)]
#[derive(Debug, Serialize)]
pub struct RelayResponse {
    pub ok: bool,
    pub errors: Vec<RelayError>,
}

#[cfg(unix)]
#[derive(Debug, Serialize)]
pub struct RelayError {
    /// Filename of the failing script (or "-" for parse errors).
    pub script: String,
    /// Exit code if applicable; `null` for spawn errors / parse errors.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub code: Option<i32>,
    /// Trailing `STDERR_TAIL_LIMIT` bytes of stderr.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub stderr_tail: String,
    /// Free-form reason for non-script-exit failures.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub reason: String,
}

#[cfg(unix)]
impl RelayResponse {
    fn parse_error(reason: String) -> Self {
        Self {
            ok: false,
            errors: vec![RelayError {
                script: "-".to_string(),
                code: None,
                stderr_tail: String::new(),
                reason,
            }],
        }
    }

    fn from_results(results: &[hooks::HookResult]) -> Self {
        let mut errors = Vec::new();
        for r in results {
            let script = r
                .script
                .file_name()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_else(|| r.script.display().to_string());
            match &r.status {
                HookStatus::Ok => {}
                HookStatus::Failed { code, stderr_tail } => errors.push(RelayError {
                    script,
                    code: Some(*code),
                    stderr_tail: stderr_tail.clone(),
                    reason: String::new(),
                }),
                HookStatus::Timeout => errors.push(RelayError {
                    script,
                    code: None,
                    stderr_tail: String::new(),
                    reason: "timed out (>5s)".to_string(),
                }),
                HookStatus::SpawnFailed(e) => errors.push(RelayError {
                    script,
                    code: None,
                    stderr_tail: String::new(),
                    reason: format!("spawn failed: {e}"),
                }),
            }
        }
        Self {
            ok: errors.is_empty(),
            errors,
        }
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::os::unix::fs::PermissionsExt;

    fn write_exec(dir: &Path, name: &str, body: &str) -> PathBuf {
        let p = dir.join(name);
        std::fs::write(&p, body).expect("write hook");
        let mut perm = std::fs::metadata(&p).expect("md").permissions();
        perm.set_mode(0o755);
        std::fs::set_permissions(&p, perm).expect("chmod");
        p
    }

    #[test]
    fn parse_request_round_trip() {
        let raw = r#"{"kind":"pre-message","payload":{"message":"hello"}}"#;
        let req: RelayRequest = serde_json::from_str(raw).expect("parse");
        assert_eq!(req.kind, "pre-message");
        assert!(req.payload.is_some());
    }

    #[test]
    fn parse_request_allows_missing_payload() {
        let raw = r#"{"kind":"update-applied"}"#;
        let req: RelayRequest = serde_json::from_str(raw).expect("parse");
        assert_eq!(req.kind, "update-applied");
        assert!(req.payload.is_none());
    }

    #[test]
    fn process_unknown_kind_yields_error() {
        let req = RelayRequest {
            kind: "totally-bogus".into(),
            payload: None,
        };
        let resp = process(req);
        assert!(!resp.ok);
        assert_eq!(resp.errors.len(), 1);
        assert!(resp.errors[0].reason.contains("unknown hook kind"));
    }

    #[test]
    fn handle_connection_invokes_hook_and_returns_ok() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let hooks_root = tmp.path().join("hooks.d");
        let kind_dir = hooks_root.join("update-applied");
        std::fs::create_dir_all(&kind_dir).expect("mkdir");
        let marker = tmp.path().join("marker.txt");
        let script = format!("#!/bin/sh\ncat > {}\n", marker.display());
        write_exec(&kind_dir, "00-record.sh", &script);

        let sock = tmp.path().join("relay.sock");
        let listener = bind(&sock).expect("bind");

        let _g = crate::env_mutex().lock().unwrap_or_else(|p| p.into_inner());
        std::env::set_var("DEPUTYOS_HOOKS_DIR", &hooks_root);

        // Spawn the server on a thread so we can connect from the same test.
        let server = std::thread::spawn(move || {
            // Accept exactly one connection so the test joins cleanly.
            if let Ok((s, _)) = listener.accept() {
                let _ = handle_connection(s);
            }
        });

        let mut client = UnixStream::connect(&sock).expect("connect");
        client
            .write_all(b"{\"kind\":\"update-applied\",\"payload\":{\"version\":\"2026.4.27\"}}\n")
            .expect("write");
        client.flush().ok();

        let mut buf = String::new();
        client.read_to_string(&mut buf).expect("read");
        server.join().expect("server thread");

        let resp: serde_json::Value = serde_json::from_str(buf.trim()).expect("parse resp");
        assert_eq!(resp["ok"], serde_json::Value::Bool(true), "got: {buf}");
        assert!(
            marker.is_file(),
            "expected hook to have created marker file"
        );
        let read = std::fs::read_to_string(&marker).expect("marker");
        assert!(read.contains("2026.4.27"), "marker: {read}");

        std::env::remove_var("DEPUTYOS_HOOKS_DIR");
    }

    #[test]
    fn handle_connection_reports_failed_hook() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let hooks_root = tmp.path().join("hooks.d");
        let kind_dir = hooks_root.join("cost-alert");
        std::fs::create_dir_all(&kind_dir).expect("mkdir");
        write_exec(
            &kind_dir,
            "00-fail.sh",
            "#!/bin/sh\necho boom >&2\nexit 7\n",
        );

        let sock = tmp.path().join("relay.sock");
        let listener = bind(&sock).expect("bind");

        let _g = crate::env_mutex().lock().unwrap_or_else(|p| p.into_inner());
        std::env::set_var("DEPUTYOS_HOOKS_DIR", &hooks_root);

        let server = std::thread::spawn(move || {
            if let Ok((s, _)) = listener.accept() {
                let _ = handle_connection(s);
            }
        });

        let mut client = UnixStream::connect(&sock).expect("connect");
        client
            .write_all(b"{\"kind\":\"cost-alert\",\"payload\":{}}\n")
            .expect("write");

        let mut buf = String::new();
        client.read_to_string(&mut buf).expect("read");
        server.join().expect("server thread");

        let resp: serde_json::Value = serde_json::from_str(buf.trim()).expect("parse");
        assert_eq!(resp["ok"], serde_json::Value::Bool(false));
        let errors = resp["errors"].as_array().expect("errors");
        assert_eq!(errors.len(), 1);
        assert_eq!(errors[0]["code"], serde_json::Value::from(7));
        assert!(
            errors[0]["stderr_tail"]
                .as_str()
                .unwrap_or("")
                .contains("boom"),
            "got: {buf}"
        );

        std::env::remove_var("DEPUTYOS_HOOKS_DIR");
    }

    #[test]
    fn handle_connection_rejects_malformed_json() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let sock = tmp.path().join("relay.sock");
        let listener = bind(&sock).expect("bind");

        let server = std::thread::spawn(move || {
            if let Ok((s, _)) = listener.accept() {
                let _ = handle_connection(s);
            }
        });

        let mut client = UnixStream::connect(&sock).expect("connect");
        client.write_all(b"not-json-at-all\n").expect("write");

        let mut buf = String::new();
        client.read_to_string(&mut buf).expect("read");
        server.join().expect("server thread");

        let resp: serde_json::Value = serde_json::from_str(buf.trim()).expect("parse");
        assert_eq!(resp["ok"], serde_json::Value::Bool(false));
        assert!(resp["errors"][0]["reason"]
            .as_str()
            .unwrap_or("")
            .contains("invalid JSON"));
    }

    #[test]
    fn bind_overwrites_stale_socket() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let sock = tmp.path().join("stale.sock");
        // Pre-create a regular file at the target path; bind() should win.
        std::fs::write(&sock, b"junk").expect("seed");
        let listener = bind(&sock).expect("bind");
        // Sanity: we can connect.
        let connector = std::thread::spawn(move || {
            if let Ok((mut s, _)) = listener.accept() {
                let _ = s.write_all(b"\n");
            }
        });
        let mut c = UnixStream::connect(&sock).expect("connect");
        let mut buf = String::new();
        let _ = c.read_to_string(&mut buf);
        connector.join().ok();
    }

    #[test]
    fn default_socket_path_honours_env() {
        let _g = crate::env_mutex().lock().unwrap_or_else(|p| p.into_inner());
        std::env::set_var("DEPUTYOS_RELAY_SOCKET", "/tmp/deputyos-relay-test.sock");
        assert_eq!(
            default_socket_path(),
            PathBuf::from("/tmp/deputyos-relay-test.sock")
        );
        std::env::remove_var("DEPUTYOS_RELAY_SOCKET");
        assert_eq!(
            default_socket_path(),
            PathBuf::from("/run/deputyos/relay.sock")
        );
    }
}
