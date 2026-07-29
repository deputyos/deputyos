//! User-hooks dispatcher (M7 anticipation skeleton).
//!
//! Per `docs/11-roadmap.md` §M7, the runtime fires four hook kinds at
//! relevant points: `pre-message`, `post-message`, `cost-alert`, and
//! `update-applied`. Each is a directory of executable scripts under
//! `/etc/deputyos/hooks.d/<kind>/`. Hooks receive a JSON payload on stdin.
//!
//! This module is the dispatcher only — the runtime fires hooks; users
//! drop scripts. The dispatcher:
//!   * Walks the `<kind>` directory in alphabetical order.
//!   * Runs each executable file with the JSON payload piped on stdin.
//!   * Enforces a 5-second timeout per script (kill on overrun).
//!   * Aggregates exit codes; logs warnings for any nonzero/timeout, but
//!     never propagates the failure to the caller.
//!
//! `update::run_apply` is wired to fire `UpdateApplied` after staging.
//! `cost::evaluate` (Lane M5) fires `CostAlert` when a budget threshold is
//! crossed. `PreMessage` / `PostMessage` are fired by `message_relay`,
//! which serves a Unix-domain socket the agent process talks to.
//!
//! Hook payload schemas live in `deputyctl/etc/hook-payload-schemas.json`.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use anyhow::Result;

use crate::paths;

/// Hook kinds. The string form maps to the on-disk subdirectory name.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HookKind {
    PreMessage,
    PostMessage,
    CostAlert,
    UpdateApplied,
}

impl HookKind {
    pub fn dir_name(self) -> &'static str {
        match self {
            HookKind::PreMessage => "pre-message",
            HookKind::PostMessage => "post-message",
            HookKind::CostAlert => "cost-alert",
            HookKind::UpdateApplied => "update-applied",
        }
    }

    /// Parse a wire-format kind string (`"pre-message"` etc.) to a [`HookKind`].
    ///
    /// Used by the relay to decode incoming JSON `{"kind": "..."}` lines.
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "pre-message" => Some(Self::PreMessage),
            "post-message" => Some(Self::PostMessage),
            "cost-alert" => Some(Self::CostAlert),
            "update-applied" => Some(Self::UpdateApplied),
            _ => None,
        }
    }

    /// All known kinds — used by `list_installed_hooks`-callers and tests.
    pub fn all() -> &'static [HookKind] {
        &[
            HookKind::PreMessage,
            HookKind::PostMessage,
            HookKind::CostAlert,
            HookKind::UpdateApplied,
        ]
    }
}

/// Per-hook timeout. Hooks that exceed it are killed and logged as failed.
const HOOK_TIMEOUT: Duration = Duration::from_secs(5);

/// Maximum bytes of stderr captured per hook for diagnostics. Tail-truncated
/// past this — keeps the dispatcher bounded if a hook is chatty.
const STDERR_TAIL_LIMIT: usize = 1024;

/// Per-script execution result. Returned by [`fire_hook_in_collect`] so the
/// relay can surface failures back to the agent in its JSON response.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HookResult {
    pub script: PathBuf,
    pub status: HookStatus,
}

/// Terminal status of a single hook script invocation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HookStatus {
    Ok,
    Failed { code: i32, stderr_tail: String },
    Timeout,
    SpawnFailed(String),
}

impl HookStatus {
    pub fn is_ok(&self) -> bool {
        matches!(self, HookStatus::Ok)
    }
}

/// Walk the hook dir for `kind` and run each executable script.
///
/// `payload` is serialized as compact JSON and piped to each script's stdin.
/// Aggregated outcome is logged via `tracing::warn!` for failures; the
/// function always returns `Ok(())` so callers can ignore the result.
pub fn fire_hook(kind: HookKind, payload: &serde_json::Value) -> Result<()> {
    let dir = paths::hooks_dir().join(kind.dir_name());
    fire_hook_in(&dir, payload)
}

/// Same as [`fire_hook`] but at an explicit directory. Used by tests.
pub fn fire_hook_in(dir: &Path, payload: &serde_json::Value) -> Result<()> {
    let _ = fire_hook_in_collect(dir, payload);
    Ok(())
}

/// Same as [`fire_hook_in`] but returns the per-script outcomes so callers
/// (like the message relay) can surface failures programmatically.
///
/// Order of returned results matches lexical order on disk. Non-executable
/// entries and non-files are silently skipped (they aren't hooks). Errors
/// reading the directory itself are logged and yield an empty Vec.
pub fn fire_hook_in_collect(dir: &Path, payload: &serde_json::Value) -> Vec<HookResult> {
    if !dir.is_dir() {
        tracing::debug!(dir = %dir.display(), "no hooks installed");
        return Vec::new();
    }
    let mut entries: Vec<PathBuf> = match std::fs::read_dir(dir) {
        Ok(it) => it.flatten().map(|e| e.path()).collect(),
        Err(e) => {
            tracing::warn!(err = %e, dir = %dir.display(), "could not read hooks dir");
            return Vec::new();
        }
    };
    entries.sort();

    let payload_bytes = serde_json::to_vec(payload).unwrap_or_else(|_| b"{}".to_vec());

    let mut results = Vec::new();
    for path in entries {
        if !is_executable_file(&path) {
            continue;
        }
        let status = run_one(&path, &payload_bytes);
        match &status {
            HookStatus::Ok => {
                tracing::debug!(hook = %path.display(), "hook ok");
            }
            HookStatus::Failed { code, stderr_tail } => {
                let name = path
                    .file_name()
                    .map(|s| s.to_string_lossy().into_owned())
                    .unwrap_or_else(|| path.display().to_string());
                tracing::warn!(
                    hook = %name,
                    code,
                    stderr_tail = %stderr_tail,
                    "hook exited nonzero"
                );
            }
            HookStatus::Timeout => {
                tracing::warn!(hook = %path.display(), "hook timed out (>5s); killed");
            }
            HookStatus::SpawnFailed(e) => {
                tracing::warn!(hook = %path.display(), err = %e, "hook spawn failed");
            }
        }
        results.push(HookResult {
            script: path,
            status,
        });
    }
    results
}

/// List installed hook scripts (executable, regular files) for a given kind.
///
/// The CLI surface is frozen and does not expose a `hooks list` subcommand,
/// but this helper is library-only — used by future audit tooling and the
/// relay's diagnostic logging.
pub fn list_installed_hooks(kind: HookKind) -> Vec<PathBuf> {
    let dir = paths::hooks_dir().join(kind.dir_name());
    list_installed_hooks_in(&dir)
}

/// Test-friendly variant of [`list_installed_hooks`].
pub fn list_installed_hooks_in(dir: &Path) -> Vec<PathBuf> {
    if !dir.is_dir() {
        return Vec::new();
    }
    let mut out: Vec<PathBuf> = match std::fs::read_dir(dir) {
        Ok(it) => it
            .flatten()
            .map(|e| e.path())
            .filter(|p| is_executable_file(p))
            .collect(),
        Err(_) => return Vec::new(),
    };
    out.sort();
    out
}

#[cfg(unix)]
fn is_executable_file(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    match std::fs::metadata(path) {
        Ok(md) => md.is_file() && (md.permissions().mode() & 0o111 != 0),
        Err(_) => false,
    }
}

#[cfg(not(unix))]
fn is_executable_file(path: &Path) -> bool {
    path.is_file()
}

fn run_one(path: &Path, payload: &[u8]) -> HookStatus {
    let mut child = match Command::new(path)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
    {
        Ok(c) => c,
        Err(e) => return HookStatus::SpawnFailed(format!("{e}")),
    };
    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(payload);
        // Drop closes the pipe, signalling EOF to the script.
    }

    let started = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                let stderr_tail = child.stderr.take().map(read_tail).unwrap_or_default();
                return if status.success() {
                    HookStatus::Ok
                } else {
                    HookStatus::Failed {
                        code: status.code().unwrap_or(-1),
                        stderr_tail,
                    }
                };
            }
            Ok(None) => {
                if started.elapsed() >= HOOK_TIMEOUT {
                    let _ = child.kill();
                    let _ = child.wait();
                    return HookStatus::Timeout;
                }
                std::thread::sleep(Duration::from_millis(50));
            }
            Err(e) => return HookStatus::SpawnFailed(format!("{e}")),
        }
    }
}

/// Read up to [`STDERR_TAIL_LIMIT`] bytes from the hook's stderr stream and
/// return the trailing window as a UTF-8-lossy string. Best-effort; an I/O
/// error returns an empty string.
fn read_tail<R: std::io::Read>(mut r: R) -> String {
    let mut buf = Vec::new();
    let mut chunk = [0u8; 256];
    loop {
        match r.read(&mut chunk) {
            Ok(0) => break,
            Ok(n) => {
                buf.extend_from_slice(&chunk[..n]);
                if buf.len() > STDERR_TAIL_LIMIT * 4 {
                    let drop = buf.len() - STDERR_TAIL_LIMIT;
                    buf.drain(..drop);
                }
            }
            Err(_) => break,
        }
    }
    if buf.len() > STDERR_TAIL_LIMIT {
        let start = buf.len() - STDERR_TAIL_LIMIT;
        buf.drain(..start);
    }
    String::from_utf8_lossy(&buf).trim().to_string()
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
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
    fn dispatcher_pipes_payload_and_succeeds() {
        let dir = tempfile::tempdir().expect("tempdir");
        let hooks = dir.path().join("update-applied");
        std::fs::create_dir_all(&hooks).expect("mkdir");
        let marker = dir.path().join("marker.txt");
        let script = format!("#!/bin/sh\ncat > \"{}\"\n", marker.display());
        write_exec(&hooks, "01-record.sh", &script);

        let payload = serde_json::json!({"version": "2026.4.27"});
        let results = fire_hook_in_collect(&hooks, &payload);
        assert_eq!(results.len(), 1, "expected one hook result: {results:?}");
        assert_eq!(
            results[0].status,
            HookStatus::Ok,
            "record hook failed: {results:?}"
        );
        let read = std::fs::read_to_string(&marker).expect("marker");
        assert!(read.contains("2026.4.27"), "got: {read}");
    }

    #[test]
    fn dispatcher_swallows_nonzero_exit() {
        let dir = tempfile::tempdir().expect("tempdir");
        let hooks = dir.path().join("update-applied");
        std::fs::create_dir_all(&hooks).expect("mkdir");
        write_exec(&hooks, "fail.sh", "#!/bin/sh\nexit 17\n");
        // Must not propagate.
        fire_hook_in(&hooks, &serde_json::json!({})).expect("fire");
    }

    #[test]
    fn dispatcher_kills_long_running_hook() {
        let dir = tempfile::tempdir().expect("tempdir");
        let hooks = dir.path().join("update-applied");
        std::fs::create_dir_all(&hooks).expect("mkdir");
        write_exec(&hooks, "slow.sh", "#!/bin/sh\nsleep 30\n");
        let start = Instant::now();
        fire_hook_in(&hooks, &serde_json::json!({})).expect("fire");
        let dur = start.elapsed();
        // 5s timeout + small slack.
        assert!(
            dur < Duration::from_secs(8),
            "dispatcher should have killed; took {dur:?}"
        );
    }

    #[test]
    fn missing_dir_is_a_no_op() {
        let dir = tempfile::tempdir().expect("tempdir");
        let hooks = dir.path().join("does-not-exist");
        fire_hook_in(&hooks, &serde_json::json!({})).expect("fire");
        assert!(fire_hook_in_collect(&hooks, &serde_json::json!({})).is_empty());
    }

    #[test]
    fn collect_returns_per_script_results() {
        let dir = tempfile::tempdir().expect("tempdir");
        let hooks = dir.path().join("post-message");
        std::fs::create_dir_all(&hooks).expect("mkdir");
        write_exec(&hooks, "01-ok.sh", "#!/bin/sh\nexit 0\n");
        write_exec(
            &hooks,
            "02-fail.sh",
            "#!/bin/sh\necho 'oh no' >&2\nexit 9\n",
        );

        let results = fire_hook_in_collect(&hooks, &serde_json::json!({}));
        assert_eq!(results.len(), 2);
        assert!(results[0].status.is_ok());
        match &results[1].status {
            HookStatus::Failed { code, stderr_tail } => {
                assert_eq!(*code, 9);
                assert!(stderr_tail.contains("oh no"), "got: {stderr_tail}");
            }
            other => panic!("expected Failed, got {other:?}"),
        }
    }

    #[test]
    fn list_installed_hooks_filters_non_executable() {
        let dir = tempfile::tempdir().expect("tempdir");
        let hooks = dir.path().join("pre-message");
        std::fs::create_dir_all(&hooks).expect("mkdir");
        write_exec(&hooks, "10-real.sh", "#!/bin/sh\nexit 0\n");
        // .disabled file: regular but not executable.
        std::fs::write(hooks.join("99-skip.sh.disabled"), "#!/bin/sh\nexit 0\n")
            .expect("write disabled");
        let listed = list_installed_hooks_in(&hooks);
        assert_eq!(listed.len(), 1);
        assert!(listed[0].ends_with("10-real.sh"));
    }

    #[test]
    fn parse_kind_round_trip() {
        for k in HookKind::all() {
            assert_eq!(HookKind::parse(k.dir_name()), Some(*k));
        }
        assert_eq!(HookKind::parse("nope"), None);
    }
}
