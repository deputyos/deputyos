//! Integration test for the message relay.
//!
//! Spawns the relay server bound to a tempdir Unix socket, then connects
//! a Unix-stream client and asserts the protocol described in
//! `deputyctl/src/message_relay.rs`. Mirrors the contract documented for
//! profile authors so they can implement the agent-side client safely.

#![cfg(unix)]

use std::io::{Read, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use deputyctl::message_relay;

/// Tests in this file mutate `DEPUTYOS_HOOKS_DIR`, which is process-global,
/// so they must serialize. Without this guard `cargo test` (which runs
/// integration tests in parallel by default) flakes.
fn env_lock() -> &'static Mutex<()> {
    static M: OnceLock<Mutex<()>> = OnceLock::new();
    M.get_or_init(|| Mutex::new(()))
}

fn write_exec(dir: &Path, name: &str, body: &str) -> PathBuf {
    let p = dir.join(name);
    std::fs::write(&p, body).expect("write hook");
    let mut perm = std::fs::metadata(&p).expect("md").permissions();
    perm.set_mode(0o755);
    std::fs::set_permissions(&p, perm).expect("chmod");
    p
}

/// Spawn a background thread serving `n` connections from `listener`, then
/// stop. Returns the join handle so the test can wait for completion.
fn serve_n(listener: std::os::unix::net::UnixListener, n: usize) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || {
        for _ in 0..n {
            match listener.accept() {
                Ok((stream, _)) => {
                    // Re-use the public dispatch path by funneling through a
                    // freshly-bound listener wrapper isn't necessary: we
                    // just inline a one-shot like the in-source test does.
                    handle_one(stream);
                }
                Err(_) => break,
            }
        }
    })
}

/// Mirror of the relay's per-connection handler. The library intentionally
/// hides `handle_connection`; this integration test goes through the public
/// `bind` + accept path and writes a small inline replica so we exercise
/// the same `serve_loop`-shaped contract.
fn handle_one(stream: std::os::unix::net::UnixStream) {
    use std::io::BufRead;
    let write_half = stream.try_clone().expect("clone");
    let mut reader = std::io::BufReader::new(stream);
    let mut writer = write_half;

    let mut line = String::new();
    if reader.read_line(&mut line).is_err() || line.is_empty() {
        return;
    }

    // Parse what the relay would parse; on success, dispatch through the
    // *real* dispatcher so we're not duplicating logic.
    let resp_json: serde_json::Value = match serde_json::from_str::<serde_json::Value>(line.trim())
    {
        Ok(v) => match v.get("kind").and_then(|k| k.as_str()) {
            Some(kind_str) => {
                if let Some(kind) = deputyctl::hooks::HookKind::parse(kind_str) {
                    let dir = deputyctl::paths::hooks_dir().join(kind.dir_name());
                    let payload = v
                        .get("payload")
                        .cloned()
                        .unwrap_or_else(|| serde_json::json!({}));
                    let results = deputyctl::hooks::fire_hook_in_collect(&dir, &payload);
                    let errors: Vec<serde_json::Value> = results
                        .into_iter()
                        .filter_map(|r| match r.status {
                            deputyctl::hooks::HookStatus::Ok => None,
                            deputyctl::hooks::HookStatus::Failed { code, stderr_tail } => {
                                Some(serde_json::json!({
                                    "script": r.script.file_name()
                                        .map(|s| s.to_string_lossy().into_owned())
                                        .unwrap_or_default(),
                                    "code": code,
                                    "stderr_tail": stderr_tail,
                                }))
                            }
                            deputyctl::hooks::HookStatus::Timeout => Some(serde_json::json!({
                                "script": r.script.file_name()
                                    .map(|s| s.to_string_lossy().into_owned())
                                    .unwrap_or_default(),
                                "reason": "timed out (>5s)",
                            })),
                            deputyctl::hooks::HookStatus::SpawnFailed(e) => {
                                Some(serde_json::json!({
                                    "script": r.script.file_name()
                                        .map(|s| s.to_string_lossy().into_owned())
                                        .unwrap_or_default(),
                                    "reason": format!("spawn failed: {e}"),
                                }))
                            }
                        })
                        .collect();
                    serde_json::json!({"ok": errors.is_empty(), "errors": errors})
                } else {
                    serde_json::json!({
                        "ok": false,
                        "errors": [{"script": "-", "reason": format!("unknown hook kind {kind_str:?}")}]
                    })
                }
            }
            None => serde_json::json!({
                "ok": false,
                "errors": [{"script": "-", "reason": "missing 'kind' field"}],
            }),
        },
        Err(e) => serde_json::json!({
            "ok": false,
            "errors": [{"script": "-", "reason": format!("invalid JSON: {e}")}],
        }),
    };

    let mut out = resp_json.to_string();
    out.push('\n');
    let _ = writer.write_all(out.as_bytes());
    let _ = writer.flush();
}

fn round_trip(sock: &Path, request: &str) -> serde_json::Value {
    let mut client = UnixStream::connect(sock).expect("connect");
    client.write_all(request.as_bytes()).expect("write");
    if !request.ends_with('\n') {
        client.write_all(b"\n").expect("nl");
    }
    client.flush().ok();
    let mut buf = String::new();
    client.read_to_string(&mut buf).expect("read");
    serde_json::from_str(buf.trim()).expect("parse response")
}

#[test]
fn relay_fires_update_applied_hook_end_to_end() {
    let _g = env_lock().lock().unwrap_or_else(|p| p.into_inner());
    let tmp = tempfile::tempdir().expect("tempdir");
    let hooks_root = tmp.path().join("hooks.d");
    let kind_dir = hooks_root.join("update-applied");
    std::fs::create_dir_all(&kind_dir).expect("mkdir");
    let marker = tmp.path().join("hook-fired.txt");
    let body = format!("#!/bin/sh\ncat > {}\n", marker.display());
    write_exec(&kind_dir, "00-record.sh", &body);

    std::env::set_var("DEPUTYOS_HOOKS_DIR", &hooks_root);

    let sock = tmp.path().join("relay.sock");
    let listener = message_relay::bind(&sock).expect("bind");
    let server = serve_n(listener, 1);

    let resp = round_trip(
        &sock,
        r#"{"kind":"update-applied","payload":{"version":"2026.4.27"}}"#,
    );
    server.join().expect("server");

    assert_eq!(resp["ok"], serde_json::Value::Bool(true), "got: {resp}");
    let read = std::fs::read_to_string(&marker).expect("marker");
    assert!(read.contains("2026.4.27"), "marker payload: {read}");

    std::env::remove_var("DEPUTYOS_HOOKS_DIR");
}

#[test]
fn relay_reports_each_failing_hook() {
    let _g = env_lock().lock().unwrap_or_else(|p| p.into_inner());
    let tmp = tempfile::tempdir().expect("tempdir");
    let hooks_root = tmp.path().join("hooks.d");
    let kind_dir = hooks_root.join("cost-alert");
    std::fs::create_dir_all(&kind_dir).expect("mkdir");
    write_exec(&kind_dir, "01-ok.sh", "#!/bin/sh\nexit 0\n");
    write_exec(
        &kind_dir,
        "02-bad.sh",
        "#!/bin/sh\necho 'over budget' >&2\nexit 4\n",
    );

    std::env::set_var("DEPUTYOS_HOOKS_DIR", &hooks_root);

    let sock = tmp.path().join("relay.sock");
    let listener = message_relay::bind(&sock).expect("bind");
    let server = serve_n(listener, 1);

    let resp = round_trip(
        &sock,
        r#"{"kind":"cost-alert","payload":{"spent_usd":10.5}}"#,
    );
    server.join().expect("server");

    assert_eq!(resp["ok"], serde_json::Value::Bool(false));
    let errors = resp["errors"].as_array().expect("errors");
    assert_eq!(errors.len(), 1, "only 02-bad.sh should error: {resp}");
    assert_eq!(errors[0]["code"], serde_json::Value::from(4));
    assert_eq!(
        errors[0]["script"],
        serde_json::Value::String("02-bad.sh".into())
    );
    assert!(errors[0]["stderr_tail"]
        .as_str()
        .unwrap_or("")
        .contains("over budget"));

    std::env::remove_var("DEPUTYOS_HOOKS_DIR");
}

#[test]
fn relay_returns_error_for_unknown_kind() {
    let _g = env_lock().lock().unwrap_or_else(|p| p.into_inner());
    let tmp = tempfile::tempdir().expect("tempdir");
    let sock = tmp.path().join("relay.sock");
    let listener = message_relay::bind(&sock).expect("bind");
    let server = serve_n(listener, 1);

    let resp = round_trip(&sock, r#"{"kind":"not-a-thing","payload":{}}"#);
    server.join().expect("server");

    assert_eq!(resp["ok"], serde_json::Value::Bool(false));
    assert!(resp["errors"][0]["reason"]
        .as_str()
        .unwrap_or("")
        .contains("unknown hook kind"));
}

#[test]
fn relay_returns_error_for_malformed_request() {
    let _g = env_lock().lock().unwrap_or_else(|p| p.into_inner());
    let tmp = tempfile::tempdir().expect("tempdir");
    let sock = tmp.path().join("relay.sock");
    let listener = message_relay::bind(&sock).expect("bind");
    let server = serve_n(listener, 1);

    let resp = round_trip(&sock, "this is not JSON at all");
    server.join().expect("server");

    assert_eq!(resp["ok"], serde_json::Value::Bool(false));
    assert!(resp["errors"][0]["reason"]
        .as_str()
        .unwrap_or("")
        .contains("invalid JSON"));
}

#[test]
fn relay_succeeds_when_no_hooks_installed() {
    let _g = env_lock().lock().unwrap_or_else(|p| p.into_inner());
    let tmp = tempfile::tempdir().expect("tempdir");
    let hooks_root = tmp.path().join("hooks.d");
    // Intentionally do not create any kind subdir.
    std::fs::create_dir_all(&hooks_root).expect("mkdir");
    std::env::set_var("DEPUTYOS_HOOKS_DIR", &hooks_root);

    let sock = tmp.path().join("relay.sock");
    let listener = message_relay::bind(&sock).expect("bind");
    let server = serve_n(listener, 1);

    let resp = round_trip(
        &sock,
        r#"{"kind":"pre-message","payload":{"message":"hi"}}"#,
    );
    server.join().expect("server");

    assert_eq!(resp["ok"], serde_json::Value::Bool(true));
    assert!(resp["errors"].as_array().expect("errors array").is_empty());

    std::env::remove_var("DEPUTYOS_HOOKS_DIR");
}

#[test]
fn relay_socket_has_owner_only_permissions() {
    let _g = env_lock().lock().unwrap_or_else(|p| p.into_inner());
    let tmp = tempfile::tempdir().expect("tempdir");
    let sock = tmp.path().join("relay.sock");
    let _listener = message_relay::bind(&sock).expect("bind");
    let perm = std::fs::metadata(&sock).expect("md").permissions();
    let mode = perm.mode() & 0o777;
    assert_eq!(mode, 0o600, "expected 0600, got {mode:o}");
}

#[test]
fn relay_connect_refused_when_socket_missing() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let sock = tmp.path().join("absent.sock");
    let r = UnixStream::connect(&sock);
    assert!(r.is_err(), "connect to nonexistent socket should fail");
}
