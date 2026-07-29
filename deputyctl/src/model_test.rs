//! `deputyctl model test` — 1-token round-trip against the configured provider.
//!
//! Reads the active provider id from `/etc/deputyos/active-provider` (or
//! `--provider <id>`), looks up the key in `/etc/deputyos/secrets.env`, and
//! delegates to [`crate::model::test_provider_key`]. The shared function is
//! also called by Lane W's wizard so both surfaces report identical
//! round-trip semantics.

use anyhow::{anyhow, Context, Result};

use crate::model;

#[derive(Debug, Clone, Default)]
pub struct TestOpts {
    pub provider: Option<String>,
    pub json: bool,
}

pub fn run(opts: TestOpts) -> Result<u8> {
    let cat = model::load_providers().context("loading providers catalogue")?;

    // 1. Resolve which provider to test.
    let provider_id = match &opts.provider {
        Some(id) => id.clone(),
        None => match model::read_active_provider() {
            Some(id) => id,
            None => {
                emit_error(opts.json, "no active provider — run `deputyctl model set`");
                return Ok(1);
            }
        },
    };
    let provider = match model::find_provider(&cat, &provider_id) {
        Some(p) => p.clone(),
        None => {
            emit_error(
                opts.json,
                &format!("unknown provider '{provider_id}' (run `deputyctl model list`)"),
            );
            return Ok(1);
        }
    };

    // 2. Resolve the key.
    let secrets = model::load_secrets();
    let key = match secrets.get(&provider.key_env_var) {
        Some(v) if !v.is_empty() => v.clone(),
        _ => {
            emit_error(
                opts.json,
                &format!(
                    "no key configured for {} ({}); run `deputyctl model set --provider {}`",
                    provider.id, provider.key_env_var, provider.id
                ),
            );
            return Ok(1);
        }
    };

    // 3. Round-trip.
    let outcome = model::test_provider_key(&provider, &key)
        .map_err(|e| anyhow!("test_provider_key: {e:#}"))?;

    if opts.json {
        let payload = serde_json::json!({
            "provider": outcome.provider_id,
            "endpoint": outcome.endpoint,
            "http_status": outcome.http_status,
            "elapsed_ms": outcome.elapsed_ms,
            "ok": outcome.ok,
            "error": outcome.error,
        });
        println!("{}", serde_json::to_string_pretty(&payload)?);
    } else {
        println!("provider:   {}", outcome.provider_id);
        println!("endpoint:   {}", outcome.endpoint);
        match outcome.http_status {
            Some(code) => println!("http:       {code}"),
            None => println!("http:       (no response)"),
        }
        println!("elapsed:    {} ms", outcome.elapsed_ms);
        println!("result:     {}", if outcome.ok { "ok" } else { "FAIL" });
        if let Some(err) = &outcome.error {
            println!("error:      {err}");
        }
    }
    Ok(if outcome.ok { 0 } else { 1 })
}

fn emit_error(json: bool, msg: &str) {
    if json {
        let payload = serde_json::json!({
            "ok": false,
            "error": msg,
        });
        println!(
            "{}",
            serde_json::to_string_pretty(&payload).unwrap_or_else(|_| "{}".to_string())
        );
    } else {
        eprintln!("model test: {msg}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write as _;

    fn stage_env(dir: &tempfile::TempDir) -> (std::path::PathBuf, std::path::PathBuf) {
        let secrets = dir.path().join("secrets.env");
        let active = dir.path().join("active-provider");
        std::env::set_var("DEPUTYOS_SECRETS_FILE", &secrets);
        std::env::set_var("DEPUTYOS_ACTIVE_PROVIDER_FILE", &active);
        let default_providers =
            std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("etc/providers.json");
        std::env::set_var("DEPUTYOS_PROVIDERS_FILE", default_providers);
        (secrets, active)
    }

    fn cleanup() {
        std::env::remove_var("DEPUTYOS_SECRETS_FILE");
        std::env::remove_var("DEPUTYOS_ACTIVE_PROVIDER_FILE");
        std::env::remove_var("DEPUTYOS_PROVIDERS_FILE");
    }

    fn write_mock_providers(dir: &tempfile::TempDir, port: u16) -> std::path::PathBuf {
        let p = dir.path().join("providers.json");
        let body = serde_json::json!({
            "providers": [{
                "id": "mock",
                "display_name": "Mock",
                "kind": "openai-compatible",
                "endpoint_default": format!("http://127.0.0.1:{port}/v1"),
                "key_env_var": "MOCK_API_KEY",
                "key_format": "test",
                "default_model": "test-model",
                "supported_models_hint": "",
            }]
        });
        std::fs::write(&p, serde_json::to_string(&body).expect("ser")).expect("write");
        std::env::set_var("DEPUTYOS_PROVIDERS_FILE", &p);
        p
    }

    fn spawn_mock(status: u16) -> u16 {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
        let port = listener.local_addr().expect("addr").port();
        std::thread::spawn(move || {
            use std::io::{Read, Write};
            if let Ok((mut stream, _)) = listener.accept() {
                let mut buf = [0u8; 4096];
                let _ = stream.read(&mut buf);
                let body = b"{\"ok\":true}";
                let phrase = if status == 200 { "OK" } else { "Unauthorized" };
                let resp = format!(
                    "HTTP/1.1 {status} {phrase}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                let _ = stream.write_all(resp.as_bytes());
                let _ = stream.write_all(body);
            }
        });
        port
    }

    #[test]
    fn json_shape_on_200() {
        let _g = crate::env_mutex().lock().unwrap_or_else(|e| e.into_inner());
        let dir = tempfile::tempdir().expect("tempdir");
        let (secrets, active) = stage_env(&dir);
        let port = spawn_mock(200);
        write_mock_providers(&dir, port);
        let mut f = std::fs::File::create(&secrets).expect("seed secrets");
        writeln!(f, "MOCK_API_KEY=sk-test").expect("write");
        let mut a = std::fs::File::create(&active).expect("active");
        writeln!(a, "mock").expect("write");

        let code = run(TestOpts {
            provider: None,
            json: true,
        })
        .expect("run");
        assert_eq!(code, 0);
        cleanup();
    }

    #[test]
    fn no_active_provider_clean_error() {
        let _g = crate::env_mutex().lock().unwrap_or_else(|e| e.into_inner());
        let dir = tempfile::tempdir().expect("tempdir");
        let (_secrets, _active) = stage_env(&dir);
        // active-provider file does not exist → expect "no active provider" error.
        let code = run(TestOpts {
            provider: None,
            json: false,
        })
        .expect("run");
        assert_eq!(code, 1);
        cleanup();
    }
}
