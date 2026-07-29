//! `deputyctl model list` — enumerate baked-in providers and which are wired up.
//!
//! The provider catalogue is bake-time data: `deputyctl/etc/providers.json` is
//! copied into `/etc/deputyos/providers.json` by the image build, and that is
//! the canonical runtime source. The runtime check for "configured" reads
//! `/etc/deputyos/secrets.env` (mode `0600`, KEY=VALUE shell-style); the env
//! var name to look for is declared per-provider as `key_env_var`.
//!
//! `model set` and `model test` are still M2 stubs; this file owns just the
//! list path so it can ship ahead of network-dependent flows.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use anyhow::{anyhow, bail, Context, Result};
use serde::{Deserialize, Serialize};

use crate::paths;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Provider {
    pub id: String,
    pub display_name: String,
    pub kind: String,
    pub endpoint_default: String,
    pub key_env_var: String,
    pub key_format: String,
    #[serde(default)]
    pub default_model: String,
    #[serde(default)]
    pub supported_models_hint: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProvidersFile {
    pub providers: Vec<Provider>,
}

/// Status of one provider entry, ready to render either as table line or JSON.
#[derive(Debug, Clone, Serialize)]
pub struct ProviderStatus {
    pub id: String,
    pub display_name: String,
    pub kind: String,
    pub configured: bool,
    pub key_env_var: String,
    pub default_model: String,
}

/// Load the bake-time provider catalogue from disk.
pub fn load_providers() -> Result<ProvidersFile> {
    let path = paths::providers_file();
    load_providers_from(&path)
}

/// Load from an explicit path. Used by tests.
pub fn load_providers_from(path: &Path) -> Result<ProvidersFile> {
    let raw = std::fs::read_to_string(path)
        .with_context(|| format!("reading providers from {}", path.display()))?;
    let parsed: ProvidersFile = serde_json::from_str(&raw)
        .with_context(|| format!("parsing providers at {}", path.display()))?;
    Ok(parsed)
}

/// Parse `/etc/deputyos/secrets.env` — a tiny KEY=VALUE shell-style env file.
///
/// Lines without `=`, blank lines, and `#`-comments are ignored. Values are
/// returned verbatim *minus* one optional layer of surrounding `"` or `'`.
/// A missing file yields an empty map (the right behaviour for "no provider
/// configured yet").
pub fn load_secrets() -> BTreeMap<String, String> {
    let path = paths::secrets_file();
    load_secrets_from(&path)
}

/// Load secrets from an explicit path. Used by tests.
pub fn load_secrets_from(path: &Path) -> BTreeMap<String, String> {
    let raw = match std::fs::read_to_string(path) {
        Ok(s) => s,
        Err(_) => return BTreeMap::new(),
    };
    let mut out = BTreeMap::new();
    for line in raw.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let line = line.strip_prefix("export ").unwrap_or(line);
        if let Some((k, v)) = line.split_once('=') {
            let key = k.trim().to_string();
            let mut value = v.trim();
            if (value.starts_with('"') && value.ends_with('"') && value.len() >= 2)
                || (value.starts_with('\'') && value.ends_with('\'') && value.len() >= 2)
            {
                value = &value[1..value.len() - 1];
            }
            if !key.is_empty() {
                out.insert(key, value.to_string());
            }
        }
    }
    out
}

/// Look up one provider entry by id from the loaded catalogue.
pub fn find_provider<'a>(catalogue: &'a ProvidersFile, id: &str) -> Option<&'a Provider> {
    catalogue.providers.iter().find(|p| p.id == id)
}

/// Read the active-provider id from disk. Returns `None` if missing/empty.
pub fn read_active_provider() -> Option<String> {
    let path = paths::active_provider_file();
    let raw = std::fs::read_to_string(&path).ok()?;
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

/// Atomically write the active-provider pointer to disk.
pub fn write_active_provider(id: &str) -> Result<()> {
    let dst = paths::active_provider_file();
    let parent = dst
        .parent()
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
    std::fs::create_dir_all(&parent)
        .with_context(|| format!("creating {} for active-provider pointer", parent.display()))?;
    let tmp = dst.with_extension("tmp");
    std::fs::write(&tmp, format!("{id}\n"))
        .with_context(|| format!("writing {}", tmp.display()))?;
    std::fs::rename(&tmp, &dst)
        .with_context(|| format!("renaming {} -> {}", tmp.display(), dst.display()))?;
    Ok(())
}

/// Atomic update of `secrets.env`: replace `<env_var>` line in place, or
/// append it if not present. Mode is forced to 0600 on Unix.
///
/// Used by `model set` and shared with the wizard via the lib re-export.
pub fn write_secret(env_var: &str, value: &str) -> Result<()> {
    write_secret_to(&paths::secrets_file(), env_var, value)
}

/// Same as [`write_secret`] but at an explicit path. Used by tests and the
/// wizard which may stage to a tempdir before promotion.
pub fn write_secret_to(path: &Path, env_var: &str, value: &str) -> Result<()> {
    if env_var.is_empty() {
        bail!("write_secret: env_var must not be empty");
    }
    let parent = path
        .parent()
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
    std::fs::create_dir_all(&parent)
        .with_context(|| format!("creating {} for secrets file", parent.display()))?;

    let existing = std::fs::read_to_string(path).unwrap_or_default();
    let mut out_lines: Vec<String> = Vec::new();
    let mut replaced = false;
    let key_prefix = format!("{env_var}=");
    let key_export_prefix = format!("export {env_var}=");
    for line in existing.lines() {
        let trimmed = line.trim_start();
        if trimmed.starts_with(&key_prefix) || trimmed.starts_with(&key_export_prefix) {
            out_lines.push(format!("{env_var}={}", shell_quote(value)));
            replaced = true;
        } else {
            out_lines.push(line.to_string());
        }
    }
    if !replaced {
        out_lines.push(format!("{env_var}={}", shell_quote(value)));
    }
    let mut body = out_lines.join("\n");
    if !body.ends_with('\n') {
        body.push('\n');
    }
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, body.as_bytes()).with_context(|| format!("writing {}", tmp.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let perm = std::fs::Permissions::from_mode(0o600);
        std::fs::set_permissions(&tmp, perm)
            .with_context(|| format!("chmod 0600 {}", tmp.display()))?;
    }
    std::fs::rename(&tmp, path)
        .with_context(|| format!("renaming {} -> {}", tmp.display(), path.display()))?;
    Ok(())
}

fn shell_quote(value: &str) -> String {
    // Always single-quote, escape any embedded single-quotes the standard way.
    if value.is_empty() {
        return "''".into();
    }
    if value.contains('\'') {
        // Replace ' with '"'"' (close, escaped, reopen).
        let mut out = String::with_capacity(value.len() + 4);
        out.push('\'');
        for c in value.chars() {
            if c == '\'' {
                out.push_str("'\"'\"'");
            } else {
                out.push(c);
            }
        }
        out.push('\'');
        return out;
    }
    if value
        .chars()
        .any(|c| c.is_whitespace() || matches!(c, '$' | '"' | '\\' | '`' | '#'))
    {
        return format!("'{value}'");
    }
    value.to_string()
}

// ---------------------------------------------------------------------------
// Provider round-trip test (shared with deputywizard via lib path-dep).
// ---------------------------------------------------------------------------

/// Outcome of a single round-trip ping against a provider endpoint.
#[derive(Debug, Clone, Serialize)]
pub struct TestOutcome {
    pub provider_id: String,
    pub endpoint: String,
    pub http_status: Option<u16>,
    pub elapsed_ms: u64,
    pub ok: bool,
    pub error: Option<String>,
}

/// Round-trip a 1-token completion against `provider` using `key`.
///
/// This is the **shared implementation** consumed by:
///   * `deputyctl model test` — reads key from `/etc/deputyos/secrets.env`.
///   * `deputywizard` (Lane W) — reads key from in-memory wizard state.
///
/// Network errors and non-2xx HTTP statuses both return `Ok(TestOutcome)` —
/// only programming bugs return `Err`. The 5-second per-request timeout is
/// enforced internally; AWS Bedrock is reported as `Skip` (multi-field auth).
pub fn test_provider_key(provider: &Provider, key: &str) -> Result<TestOutcome> {
    let start = Instant::now();
    let agent = ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(2))
        .timeout(Duration::from_secs(5))
        .build();

    if provider.kind == "bedrock" {
        return Ok(TestOutcome {
            provider_id: provider.id.clone(),
            endpoint: provider.endpoint_default.clone(),
            http_status: None,
            elapsed_ms: 0,
            ok: false,
            error: Some(
                "AWS Bedrock requires multi-field auth (key id + secret + region); not yet supported by `model test`"
                    .into(),
            ),
        });
    }

    let (url, body, headers): (String, serde_json::Value, Vec<(&str, String)>) =
        match provider.kind.as_str() {
            "anthropic" => (
                "https://api.anthropic.com/v1/messages".to_string(),
                serde_json::json!({
                    "model": if provider.default_model.is_empty() {
                        "claude-haiku-4-5-20251001"
                    } else {
                        &provider.default_model
                    },
                    "max_tokens": 1,
                    "messages": [{"role": "user", "content": "hi"}],
                }),
                vec![
                    ("x-api-key", key.to_string()),
                    ("anthropic-version", "2023-06-01".to_string()),
                    ("content-type", "application/json".to_string()),
                ],
            ),
            "openai-compatible" | "openai" | "huggingface" | "google" => {
                // Local-Ollama uses /api/generate; everything else openai-shape.
                if provider.id == "local-ollama" {
                    let endpoint = endpoint_or_default(provider);
                    let base = endpoint.trim_end_matches("/v1").trim_end_matches('/');
                    (
                        format!("{base}/api/generate"),
                        serde_json::json!({
                            "model": provider.default_model,
                            "prompt": "hi",
                            "stream": false,
                        }),
                        vec![("content-type", "application/json".to_string())],
                    )
                } else {
                    let endpoint = endpoint_or_default(provider);
                    let endpoint = endpoint.trim_end_matches('/');
                    (
                        format!("{endpoint}/chat/completions"),
                        serde_json::json!({
                            "model": provider.default_model,
                            "messages": [{"role": "user", "content": "hi"}],
                            "max_tokens": 1,
                        }),
                        vec![
                            ("authorization", format!("Bearer {key}")),
                            ("content-type", "application/json".to_string()),
                        ],
                    )
                }
            }
            other => {
                return Ok(TestOutcome {
                    provider_id: provider.id.clone(),
                    endpoint: provider.endpoint_default.clone(),
                    http_status: None,
                    elapsed_ms: 0,
                    ok: false,
                    error: Some(format!("unsupported provider kind '{other}'")),
                });
            }
        };

    let mut req = agent.post(&url);
    for (k, v) in &headers {
        req = req.set(k, v);
    }
    let result = req.send_json(body);
    let elapsed = start.elapsed();

    let outcome = match result {
        Ok(resp) => TestOutcome {
            provider_id: provider.id.clone(),
            endpoint: url,
            http_status: Some(resp.status()),
            elapsed_ms: elapsed.as_millis() as u64,
            ok: resp.status() >= 200 && resp.status() < 300,
            error: None,
        },
        Err(ureq::Error::Status(code, resp)) => {
            let body = resp.into_string().unwrap_or_default();
            TestOutcome {
                provider_id: provider.id.clone(),
                endpoint: url,
                http_status: Some(code),
                elapsed_ms: elapsed.as_millis() as u64,
                ok: false,
                error: Some(truncate(&body, 240)),
            }
        }
        Err(ureq::Error::Transport(t)) => TestOutcome {
            provider_id: provider.id.clone(),
            endpoint: url,
            http_status: None,
            elapsed_ms: elapsed.as_millis() as u64,
            ok: false,
            error: Some(format!("transport: {t}")),
        },
    };
    Ok(outcome)
}

fn endpoint_or_default(p: &Provider) -> String {
    if p.endpoint_default.is_empty() {
        // Custom OpenAI-compatible without a baked endpoint cannot be tested.
        // Caller will surface "endpoint required" via the `error` field.
        String::new()
    } else {
        p.endpoint_default.clone()
    }
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let cut: String = s.chars().take(max).collect();
        format!("{cut}…")
    }
}

/// Convenience: load the catalogue, look up `id`, ping it.
///
/// Errors if the provider is unknown or the catalogue can't be loaded.
/// Network/auth failures are still reported via [`TestOutcome::ok`].
pub fn test_provider_id(id: &str, key: &str) -> Result<TestOutcome> {
    let cat = load_providers()?;
    let p = find_provider(&cat, id).ok_or_else(|| anyhow!("unknown provider id: {id}"))?;
    test_provider_key(p, key)
}

/// Resolve the set of providers + their configured state.
/// In airgap mode, prepends baked GGUF models from the airgap catalog.
pub fn list_status() -> Result<Vec<ProviderStatus>> {
    let providers = load_providers()?;
    let secrets = load_secrets();
    let is_airgap = airgap_active();

    // Start with baked airgap models when running in airgap mode.
    let mut all: Vec<ProviderStatus> = if is_airgap {
        load_airgap_models().unwrap_or_default()
    } else {
        Vec::new()
    };

    let cloud_providers: Vec<ProviderStatus> = providers
        .providers
        .into_iter()
        .map(|p| {
            let configured = secrets
                .get(&p.key_env_var)
                .map(|v| !v.is_empty())
                .unwrap_or(false);
            ProviderStatus {
                id: p.id,
                display_name: p.display_name,
                kind: p.kind,
                configured,
                key_env_var: p.key_env_var,
                default_model: p.default_model,
            }
        })
        .collect();

    all.extend(cloud_providers);
    Ok(all)
}

/// True when the airgap flag file exists at `/etc/deputyos/airgap.flag`
/// (env-overridable via `DEPUTYOS_AIRGAP_FLAG` for hermetic tests).
pub fn airgap_active() -> bool {
    crate::paths::airgap_flag_file().is_file()
}

/// Load baked-airgap models from `/opt/deputyos/airgap/models/catalog.json`
/// (env-overridable via `DEPUTYOS_AIRGAP_CATALOG`).
/// Returns synthetic `ProviderStatus` entries with kind `local-llamacpp`.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct AirgapCatalogEntry {
    id: String,
    filename: String,
    #[serde(default)]
    size_mb: u64,
    #[serde(default)]
    port: u16,
    #[serde(default)]
    default: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct AirgapCatalog {
    models: Vec<AirgapCatalogEntry>,
}

pub fn load_airgap_models() -> Result<Vec<ProviderStatus>> {
    Ok(load_airgap_choices()?
        .into_iter()
        .map(|c| ProviderStatus {
            id: c.id,
            display_name: c.display_name,
            kind: "local-llamacpp".into(),
            configured: true, // always available — baked into image
            key_env_var: String::new(),
            default_model: c.model_id,
        })
        .collect())
}

/// One baked/registered airgap model offered to the wizard. Richer than
/// `ProviderStatus` because the wizard needs the `default` flag to pre-select
/// the catalog's default model (the one the profile's `[airgap]
/// default_provider = "local-llamacpp-airgap"` alias resolves to).
#[derive(Debug, Clone)]
pub struct AirgapChoice {
    /// Provider id the wizard persists (`airgap-<model-id>`).
    pub id: String,
    /// Human-readable label, e.g. `LFM2-1.2B (airgap, default)`.
    pub display_name: String,
    /// The baked model id (e.g. `LFM2-1.2B`); stored as the provider's
    /// `default_model`.
    pub model_id: String,
    /// Whether this is the catalog's default (the wizard pre-selects it).
    pub default: bool,
}

/// Read the airgap catalog and return wizard-ready choices (with the default
/// flag preserved). Env-overridable via `DEPUTYOS_AIRGAP_CATALOG` (M4.5).
pub fn load_airgap_choices() -> Result<Vec<AirgapChoice>> {
    let catalog_path = crate::paths::airgap_catalog_file();
    if !catalog_path.is_file() {
        return Ok(Vec::new());
    }
    let raw = std::fs::read_to_string(&catalog_path)
        .with_context(|| format!("reading airgap catalog from {}", catalog_path.display()))?;
    let catalog: AirgapCatalog = serde_json::from_str(&raw)
        .with_context(|| format!("parsing airgap catalog at {}", catalog_path.display()))?;

    Ok(catalog
        .models
        .into_iter()
        .map(|m| {
            let label = if m.default {
                format!("{} (airgap, default)", m.id)
            } else {
                format!("{} (airgap)", m.id)
            };
            AirgapChoice {
                id: format!("airgap-{}", m.id.to_lowercase().replace(' ', "-")),
                display_name: label,
                model_id: m.id,
                default: m.default,
            }
        })
        .collect())
}

/// Register a new GGUF model on a running device.
/// Copies the file, adds it to the catalog, and optionally creates a systemd unit.
pub fn register_gguf(source: &Path, model_id: &str, enable: bool) -> Result<String> {
    let dest_dir = crate::paths::airgap_models_dir();
    std::fs::create_dir_all(&dest_dir)
        .with_context(|| format!("creating {}", dest_dir.display()))?;

    let filename = source
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| format!("{model_id}.gguf"));
    let dest = dest_dir.join(&filename);

    if !dest.exists() {
        std::fs::copy(source, &dest)
            .with_context(|| format!("copying {} -> {}", source.display(), dest.display()))?;
    }

    // Append to or create the catalog.
    let catalog_path = crate::paths::airgap_catalog_file();
    let mut catalog: AirgapCatalog = if catalog_path.is_file() {
        let raw = std::fs::read_to_string(&catalog_path)
            .with_context(|| format!("reading {}", catalog_path.display()))?;
        serde_json::from_str(&raw).unwrap_or(AirgapCatalog { models: Vec::new() })
    } else {
        if let Some(parent) = catalog_path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("creating airgap catalog dir {}", parent.display()))?;
        }
        AirgapCatalog { models: Vec::new() }
    };

    // Don't duplicate.
    let mut wrote_entry = false;
    if !catalog.models.iter().any(|m| m.id == model_id) {
        let size_mb = std::fs::metadata(&dest)
            .map(|m| m.len() / (1024 * 1024))
            .unwrap_or(0);
        // Assign a port: one above the current max (first model gets 8091, the
        // baked default occupies 8090). Mirrors the per-instance env file the
        // llamacpp@ template reads so two enabled models never collide.
        let next_port = catalog.models.iter().map(|m| m.port).max().unwrap_or(8090) + 1;
        catalog.models.push(AirgapCatalogEntry {
            id: model_id.to_string(),
            filename: filename.clone(),
            size_mb,
            port: next_port,
            default: catalog.models.is_empty(),
        });
        wrote_entry = true;
    }

    let body = serde_json::to_string_pretty(&catalog).context("serialising airgap catalog")?;
    let tmp = catalog_path.with_extension("json.tmp");
    std::fs::write(&tmp, body.as_bytes()).with_context(|| format!("writing {}", tmp.display()))?;
    std::fs::rename(&tmp, &catalog_path)
        .with_context(|| format!("renaming -> {}", catalog_path.display()))?;

    // Write the per-instance env file the deputyos-llamacpp@<id>.service
    // template reads (LLAMACPP_PORT + LLAMACPP_MODEL_FILE), so parallel-enabled
    // models bind distinct ports and the exact on-disk filename is honoured
    // (it is not always `<id>.Q4_K_M.gguf` — e.g. Qwen ships as
    // `Qwen2.5-Coder-1.5B-Instruct-Q4_K_M.gguf`). Idempotent: always reflects
    // the catalog entry.
    if let Some(entry) = catalog.models.iter().find(|m| m.id == model_id) {
        let env_dir = crate::paths::airgap_catalog_file()
            .parent()
            .map(|p| p.to_path_buf())
            .unwrap_or_else(|| PathBuf::from("/opt/deputyos/airgap/models"));
        let _ = std::fs::create_dir_all(&env_dir);
        let env_path = env_dir.join(format!("{model_id}.env"));
        let env_body = format!(
            "LLAMACPP_PORT={}\nLLAMACPP_MODEL_FILE={}\n",
            entry.port, entry.filename
        );
        let _ = std::fs::write(&env_path, env_body);
    }

    if enable && wrote_entry {
        // Create and enable a systemd unit instance for this model.
        let unit = format!("deputyos-llamacpp@{model_id}.service");
        let status = std::process::Command::new("systemctl")
            .args(["enable", "--now", &unit])
            .status();
        if let Ok(s) = status {
            if s.success() {
                return Ok(unit);
            }
        }
    }

    Ok(format!("{filename} registered in catalog"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn write_secret_appends_when_missing() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("secrets.env");
        write_secret_to(&path, "OPENROUTER_API_KEY", "sk-or-test-1").expect("write");
        let body = std::fs::read_to_string(&path).expect("read");
        assert!(
            body.contains("OPENROUTER_API_KEY=sk-or-test-1"),
            "got: {body}"
        );
        assert!(body.ends_with('\n'));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&path).expect("md").permissions().mode() & 0o777;
            assert_eq!(mode, 0o600, "secrets.env must be 0600");
        }
    }

    #[test]
    fn write_secret_preserves_existing_keys() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("secrets.env");
        std::fs::write(&path, "ANTHROPIC_API_KEY=sk-ant-old\n# comment\n").expect("seed");
        write_secret_to(&path, "OPENROUTER_API_KEY", "sk-or-new").expect("write");
        let body = std::fs::read_to_string(&path).expect("read");
        assert!(body.contains("ANTHROPIC_API_KEY=sk-ant-old"));
        assert!(body.contains("OPENROUTER_API_KEY=sk-or-new"));
        assert!(body.contains("# comment"));
    }

    #[test]
    fn write_secret_replaces_in_place() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("secrets.env");
        std::fs::write(&path, "OPENROUTER_API_KEY=old\nOTHER=keep\n").expect("seed");
        write_secret_to(&path, "OPENROUTER_API_KEY", "new").expect("write");
        let body = std::fs::read_to_string(&path).expect("read");
        assert!(body.contains("OPENROUTER_API_KEY=new"));
        assert!(!body.contains("OPENROUTER_API_KEY=old"));
        assert!(body.contains("OTHER=keep"));
    }

    /// 200 round-trip against a localhost mock of an openai-compatible provider.
    #[test]
    fn test_provider_key_200_against_mock() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
        let port = listener.local_addr().expect("addr").port();
        let handle = std::thread::spawn(move || {
            use std::io::{Read, Write};
            if let Ok((mut stream, _)) = listener.accept() {
                let mut buf = [0u8; 4096];
                let _ = stream.read(&mut buf);
                let body =
                    b"{\"choices\":[{\"message\":{\"role\":\"assistant\",\"content\":\"ok\"}}]}";
                let resp = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                let _ = stream.write_all(resp.as_bytes());
                let _ = stream.write_all(body);
            }
        });

        let p = Provider {
            id: "mock".into(),
            display_name: "Mock".into(),
            kind: "openai-compatible".into(),
            endpoint_default: format!("http://127.0.0.1:{port}/v1"),
            key_env_var: "MOCK_API_KEY".into(),
            key_format: "test".into(),
            default_model: "test-model".into(),
            supported_models_hint: String::new(),
        };
        let outcome = test_provider_key(&p, "sk-test").expect("test");
        let _ = handle.join();
        assert!(outcome.ok, "expected ok, got {outcome:?}");
        assert_eq!(outcome.http_status, Some(200));
    }

    /// 401 round-trip — outcome.ok=false but a clean status is returned.
    #[test]
    fn test_provider_key_401_against_mock() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
        let port = listener.local_addr().expect("addr").port();
        let handle = std::thread::spawn(move || {
            use std::io::{Read, Write};
            if let Ok((mut stream, _)) = listener.accept() {
                let mut buf = [0u8; 4096];
                let _ = stream.read(&mut buf);
                let body = b"{\"error\":\"unauthorized\"}";
                let resp = format!(
                    "HTTP/1.1 401 Unauthorized\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                let _ = stream.write_all(resp.as_bytes());
                let _ = stream.write_all(body);
            }
        });

        let p = Provider {
            id: "mock".into(),
            display_name: "Mock".into(),
            kind: "openai-compatible".into(),
            endpoint_default: format!("http://127.0.0.1:{port}/v1"),
            key_env_var: "MOCK_API_KEY".into(),
            key_format: "test".into(),
            default_model: "test-model".into(),
            supported_models_hint: String::new(),
        };
        let outcome = test_provider_key(&p, "bad-key").expect("test");
        let _ = handle.join();
        assert!(!outcome.ok, "expected !ok, got {outcome:?}");
        assert_eq!(outcome.http_status, Some(401));
    }

    #[test]
    fn test_provider_key_bedrock_skipped() {
        let p = Provider {
            id: "bedrock".into(),
            display_name: "AWS Bedrock".into(),
            kind: "bedrock".into(),
            endpoint_default: "https://example".into(),
            key_env_var: "AWS_ACCESS_KEY_ID".into(),
            key_format: "AKIA...".into(),
            default_model: "anthropic.claude".into(),
            supported_models_hint: String::new(),
        };
        let outcome = test_provider_key(&p, "AKIA-test").expect("test");
        assert!(!outcome.ok);
        let err = outcome.error.expect("err");
        assert!(err.contains("multi-field auth"), "got: {err}");
    }
}
