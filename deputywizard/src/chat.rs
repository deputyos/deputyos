//! Built-in private web chat (`/chat`).
//!
//! Per `docs/01-getting-started.md`, the wizard ships a tiny chat surface so
//! the user can talk to the agent **before** wiring up Telegram/Slack. The
//! chat just relays JSON to the active profile's local HTTP API. We don't
//! implement the agent — we forward whatever the user types and render
//! whatever JSON comes back.
//!
//! Endpoint discovery: the active profile's manifest declares
//! `[health].http_check = "http://127.0.0.1:8080/healthz"`. We strip the
//! trailing path segment and try `<base>/chat`. If the agent doesn't
//! implement that route (404 / connection refused / non-JSON 5xx) the page
//! shows a graceful "the agent doesn't expose a known chat endpoint yet —
//! wire up a channel from the wizard /channels step instead".
//!
//! History: appended to `~/.openclaw/chat-history.jsonl` (or the active
//! profile's `data_dir`). One JSON object per line, newest at the top of
//! the rendered list. Path is overridable via `DEPUTYWIZARD_CHAT_HISTORY`
//! for tests.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::{Deserialize, Serialize};

const CHAT_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatTurn {
    pub role: String,
    pub content: String,
    pub at: String,
}

/// Resolve the chat history file. Honours `DEPUTYWIZARD_CHAT_HISTORY` first,
/// then falls back to `<data_dir>/chat-history.jsonl` per the active profile,
/// then `./.chat-history.jsonl` for dev.
pub fn history_path(data_dir: Option<&Path>) -> PathBuf {
    if let Ok(p) = std::env::var("DEPUTYWIZARD_CHAT_HISTORY") {
        return PathBuf::from(p);
    }
    if let Some(d) = data_dir {
        return d.join("chat-history.jsonl");
    }
    PathBuf::from("./.chat-history.jsonl")
}

/// Append a turn to the history file. Best-effort: a missing parent dir is
/// created; an unwritable path is logged and swallowed (this is a dev
/// convenience, not a system of record).
pub fn append_turn(path: &Path, turn: &ChatTurn) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            let _ = std::fs::create_dir_all(parent);
        }
    }
    let line = serde_json::to_string(turn).unwrap_or_else(|_| "{}".into());
    use std::io::Write;
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?;
    writeln!(f, "{line}")?;
    Ok(())
}

/// Load history (oldest-first) from the JSONL file. Missing file → empty Vec.
/// Malformed lines are skipped.
pub fn load_history(path: &Path) -> Vec<ChatTurn> {
    let raw = match std::fs::read_to_string(path) {
        Ok(r) => r,
        Err(_) => return Vec::new(),
    };
    raw.lines()
        .filter_map(|l| serde_json::from_str::<ChatTurn>(l).ok())
        .collect()
}

/// Talk to the agent. Returns either the reply or a structured failure the
/// caller can render. `endpoint` is the HTTP base (`http://127.0.0.1:8080`).
#[derive(Debug)]
pub enum AgentReply {
    Ok(String),
    Unavailable(String),
}

pub fn ask_agent(endpoint: &str, message: &str) -> AgentReply {
    let url = format!("{}/chat", endpoint.trim_end_matches('/'));
    let agent = ureq::AgentBuilder::new()
        .timeout_connect(CHAT_TIMEOUT)
        .timeout_read(CHAT_TIMEOUT)
        .timeout_write(CHAT_TIMEOUT)
        .build();
    let body = serde_json::json!({ "message": message });
    match agent.post(&url).send_json(body) {
        Ok(resp) => match resp.into_string() {
            Ok(s) => match serde_json::from_str::<serde_json::Value>(&s) {
                Ok(v) => match v.get("reply").and_then(|r| r.as_str()) {
                    Some(r) => AgentReply::Ok(r.to_string()),
                    None => AgentReply::Unavailable(
                        "agent responded but did not include a `reply` field".into(),
                    ),
                },
                Err(_) => AgentReply::Unavailable("agent reply was not JSON".into()),
            },
            Err(e) => AgentReply::Unavailable(format!("could not read reply body: {e}")),
        },
        Err(ureq::Error::Status(404, _)) => AgentReply::Unavailable(
            "the agent doesn't expose a known chat endpoint yet — wire up a channel from the \
             wizard /channels step instead"
                .into(),
        ),
        Err(ureq::Error::Status(s, _)) => {
            AgentReply::Unavailable(format!("agent returned HTTP {s}"))
        }
        Err(ureq::Error::Transport(t)) => {
            AgentReply::Unavailable(format!("agent unreachable: {t}"))
        }
    }
}

/// Derive the agent HTTP base URL from a manifest's `[health].http_check`.
/// `"http://127.0.0.1:8080/healthz"` → `"http://127.0.0.1:8080"`.
pub fn agent_base_from_health_check(http_check: &str) -> String {
    if let Ok(parsed) = url_lite_parse(http_check) {
        return parsed;
    }
    http_check.trim_end_matches("/healthz").to_string()
}

/// Tiny URL parser: keep scheme + authority, drop the path. We don't pull in
/// the `url` crate for one operation. Accepts `http://host:port/anything` →
/// `http://host:port`.
fn url_lite_parse(s: &str) -> Result<String, ()> {
    let (scheme, rest) = s.split_once("://").ok_or(())?;
    let authority_end = rest.find('/').unwrap_or(rest.len());
    let authority = &rest[..authority_end];
    if authority.is_empty() {
        return Err(());
    }
    Ok(format!("{scheme}://{authority}"))
}

/// In-memory share of the most recent reply so the test can assert on it.
/// Production doesn't read this; it's just the route's natural shape.
#[derive(Debug, Default, Clone)]
pub struct ChatState {
    pub history_path: Arc<Mutex<Option<PathBuf>>>,
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn parses_health_check_to_base() {
        assert_eq!(
            agent_base_from_health_check("http://127.0.0.1:8080/healthz"),
            "http://127.0.0.1:8080"
        );
        assert_eq!(
            agent_base_from_health_check("http://localhost:1234/some/other/path"),
            "http://localhost:1234"
        );
    }

    #[test]
    fn append_and_load_history_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("hist.jsonl");
        append_turn(
            &p,
            &ChatTurn {
                role: "user".into(),
                content: "hi".into(),
                at: "2026-04-26T00:00:00Z".into(),
            },
        )
        .unwrap();
        append_turn(
            &p,
            &ChatTurn {
                role: "assistant".into(),
                content: "hello".into(),
                at: "2026-04-26T00:00:01Z".into(),
            },
        )
        .unwrap();
        let h = load_history(&p);
        assert_eq!(h.len(), 2);
        assert_eq!(h[0].role, "user");
        assert_eq!(h[1].content, "hello");
    }

    #[test]
    fn agent_unreachable_is_graceful() {
        // Pick a port and immediately drop — connection refused.
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = l.local_addr().unwrap();
        drop(l);
        let endpoint = format!("http://{addr}");
        match ask_agent(&endpoint, "hi") {
            AgentReply::Unavailable(_) => {}
            other => panic!("expected Unavailable, got {other:?}"),
        }
    }
}
