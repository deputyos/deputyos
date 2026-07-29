//! `deputyctl audit` — local audit-event spool and cloud flush.
//!
//! The appliance keeps audit events as newline-delimited JSON locally first.
//! Cloud upload is a batch operation so offline/air-gapped devices can keep
//! operating and flush later.

use std::fs::{self, OpenOptions};
use std::io::{Read, Write};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{anyhow, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::paths;

#[derive(Debug)]
pub struct EmitOpts {
    pub kind: String,
    pub payload: String,
}

#[derive(Debug)]
pub struct ListOpts {
    pub last: usize,
    pub json: bool,
}

#[derive(Debug)]
pub struct FlushOpts {
    pub api_base: String,
    pub dry_run: bool,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct AuditEvent {
    pub schema_version: u8,
    pub id: String,
    pub kind: String,
    pub occurred_unix_ms: u128,
    pub profile: Option<String>,
    pub payload: Value,
}

pub fn run_emit(opts: EmitOpts) -> Result<u8> {
    let payload: Value = serde_json::from_str(&opts.payload)
        .with_context(|| "payload must be valid JSON, for example --payload '{}'")?;
    let event = AuditEvent {
        schema_version: 1,
        id: new_event_id(),
        kind: validate_kind(&opts.kind)?.to_string(),
        occurred_unix_ms: now_ms(),
        profile: paths::read_active_profile_id(),
        payload,
    };
    append_event(&event)?;
    println!("{}", serde_json::to_string(&event)?);
    Ok(0)
}

pub fn run_list(opts: ListOpts) -> Result<u8> {
    let events = read_events()?;
    let start = events.len().saturating_sub(opts.last);
    let slice = &events[start..];
    if opts.json {
        println!("{}", serde_json::to_string_pretty(slice)?);
    } else if slice.is_empty() {
        println!("audit spool is empty");
        println!("spool: {}", paths::audit_spool_file().display());
    } else {
        for e in slice {
            println!("{} {} {}", e.occurred_unix_ms, e.kind, e.id);
        }
    }
    Ok(0)
}

pub fn run_flush(opts: FlushOpts) -> Result<u8> {
    let spool = paths::audit_spool_file();
    let mut body = String::new();
    match fs::File::open(&spool) {
        Ok(mut f) => {
            f.read_to_string(&mut body)
                .with_context(|| format!("reading {}", spool.display()))?;
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            println!("audit flush: no spool at {}", spool.display());
            return Ok(0);
        }
        Err(e) => return Err(e).with_context(|| format!("opening {}", spool.display())),
    }

    let body = body.trim();
    if body.is_empty() {
        println!("audit flush: spool is empty");
        return Ok(0);
    }

    let event_count = body.lines().count();
    if opts.dry_run {
        println!(
            "audit flush: would upload {event_count} events from {}",
            spool.display()
        );
        return Ok(0);
    }

    let token_path = paths::cloud_backup_token_file();
    let token = fs::read_to_string(&token_path)
        .with_context(|| format!("reading {}", token_path.display()))?
        .trim()
        .to_string();
    if token.is_empty() {
        return Err(anyhow!(
            "backup/audit token is empty at {}",
            token_path.display()
        ));
    }

    let url = format!(
        "{}/api/v1/audit/batches",
        opts.api_base.trim_end_matches('/')
    );
    let response = ureq::post(&url)
        .set("Authorization", &format!("Bearer {token}"))
        .set("Content-Type", "application/x-ndjson")
        .send_string(body);

    match response {
        Ok(resp) if (200..300).contains(&resp.status()) => {
            fs::write(&spool, "").with_context(|| format!("truncating {}", spool.display()))?;
            println!("audit flush: uploaded {event_count} events");
            Ok(0)
        }
        Ok(resp) => Err(anyhow!("audit flush failed: HTTP {}", resp.status())),
        Err(e) => Err(anyhow!("audit flush failed: {e}")),
    }
}

fn append_event(event: &AuditEvent) -> Result<()> {
    let path = paths::audit_spool_file();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
    }
    let mut f = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .with_context(|| format!("opening {}", path.display()))?;
    writeln!(f, "{}", serde_json::to_string(event)?)
        .with_context(|| format!("writing {}", path.display()))?;
    Ok(())
}

fn read_events() -> Result<Vec<AuditEvent>> {
    let path = paths::audit_spool_file();
    let raw = match fs::read_to_string(&path) {
        Ok(s) => s,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(vec![]),
        Err(e) => return Err(e).with_context(|| format!("reading {}", path.display())),
    };
    raw.lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| serde_json::from_str(l).with_context(|| "parsing audit spool line"))
        .collect()
}

fn validate_kind(kind: &str) -> Result<&str> {
    if kind
        .chars()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_')
        && (3..=64).contains(&kind.len())
    {
        Ok(kind)
    } else {
        Err(anyhow!("audit kind must be 3-64 chars: [a-z0-9_-]"))
    }
}

fn now_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
}

fn new_event_id() -> String {
    format!("evt-{}-{}", now_ms(), std::process::id())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn emit_writes_jsonl_spool() {
        let _guard = crate::env_mutex().lock().expect("env mutex poisoned");
        let dir = tempfile::tempdir().expect("tempdir");
        std::env::set_var("DEPUTYOS_AUDIT_SPOOL", dir.path().join("spool.jsonl"));
        run_emit(EmitOpts {
            kind: "backup_completed".to_string(),
            payload: "{\"ok\":true}".to_string(),
        })
        .expect("emit audit event");
        let events = read_events().expect("read audit events");
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].kind, "backup_completed");
        std::env::remove_var("DEPUTYOS_AUDIT_SPOOL");
    }
}
