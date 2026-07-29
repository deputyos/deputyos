//! Round-trip provider key validation.
//!
//! When the user types an API key at wizard step 3 we issue a single, cheap
//! HTTP request to the provider's "list models" endpoint to confirm the key
//! works **before** persisting it to `secrets.env`. This is the classic
//! 80/20 fix that catches mistyped keys without dragging in an SDK per
//! provider.
//!
//! Per `docs/05-model-providers.md`, the providers fall into a handful of
//! shapes by `kind`:
//!
//! - `openai-compatible`, `openai`: `GET <endpoint>/models` with
//!   `Authorization: Bearer <key>`. (For OpenAI-compatible providers whose
//!   endpoint already ends in `/v1`, the joined URL is `<endpoint>/models`.)
//! - `anthropic`: `GET https://api.anthropic.com/v1/models` with the
//!   `x-api-key` and `anthropic-version: 2023-06-01` headers.
//! - `local-ollama`: `GET <endpoint>/api/tags` (with the Ollama path massaged
//!   if the endpoint has `/v1` baked on). No auth.
//! - `bedrock`, `huggingface`, `google`: skipped — they're either
//!   multi-field-auth (Bedrock) or have non-uniform list endpoints. We
//!   persist the key without round-trip and the wizard surfaces a "skipped
//!   validation for this provider kind" notice. Better than a fake 200.
//!
//! Network egress is permitted (it's the user pasting their own key against
//! their own provider). All requests carry a 5-second timeout so a
//! restricted network never wedges the wizard. The user can also tick
//! "Skip validation" to bypass.

use std::time::Duration;

use deputyctl::model::Provider;

/// Outcome of a round-trip validation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CheckResult {
    /// The provider returned a 2xx response — the key is good.
    Ok,
    /// The provider responded but with an HTTP error status (4xx/5xx).
    /// The wizard renders this verbatim alongside the provider's
    /// `key_format` hint.
    HttpError { status: u16, hint: String },
    /// The request didn't complete (timeout, DNS, refused). The wizard
    /// renders a generic "couldn't reach provider" message and offers a
    /// "skip validation" checkbox.
    Network { message: String },
    /// We don't know how to round-trip this provider kind. Persist the key
    /// without validation and warn the user.
    Skipped { reason: &'static str },
}

impl CheckResult {
    pub fn is_ok(&self) -> bool {
        matches!(self, CheckResult::Ok | CheckResult::Skipped { .. })
    }
}

const DEFAULT_TIMEOUT: Duration = Duration::from_secs(5);

/// Public entry point. The `endpoint_override` lets callers (notably tests)
/// substitute a localhost mock; in production it's just `provider.endpoint_default`.
pub fn check(provider: &Provider, api_key: &str) -> CheckResult {
    check_with(
        provider,
        api_key,
        &provider.endpoint_default,
        DEFAULT_TIMEOUT,
    )
}

pub fn check_with(
    provider: &Provider,
    api_key: &str,
    endpoint: &str,
    timeout: Duration,
) -> CheckResult {
    let agent = ureq::AgentBuilder::new()
        .timeout_connect(timeout)
        .timeout_read(timeout)
        .timeout_write(timeout)
        .build();

    match provider.kind.as_str() {
        "anthropic" => {
            let url = format!("{}/v1/models", endpoint.trim_end_matches('/'));
            let req = agent
                .get(&url)
                .set("x-api-key", api_key)
                .set("anthropic-version", "2023-06-01");
            execute(req, &provider.key_format)
        }
        "openai" | "openai-compatible" => {
            // For Local Ollama specifically, the no-auth `/api/tags` endpoint
            // is the canonical health check; the rest of the openai-compatible
            // bucket uses `<endpoint>/models` with a Bearer token.
            if provider.id == "local-ollama" {
                let base = endpoint
                    .trim_end_matches('/')
                    .trim_end_matches("/v1")
                    .trim_end_matches('/');
                let url = format!("{}/api/tags", base);
                return execute(agent.get(&url), &provider.key_format);
            }
            let url = format!("{}/models", endpoint.trim_end_matches('/'));
            let req = agent
                .get(&url)
                .set("Authorization", &format!("Bearer {api_key}"));
            execute(req, &provider.key_format)
        }
        "bedrock" => CheckResult::Skipped {
            reason: "AWS Bedrock uses multi-field SigV4 auth; key persisted without round-trip.",
        },
        "huggingface" => CheckResult::Skipped {
            reason:
                "Hugging Face requires a model id to validate; key persisted without round-trip.",
        },
        "google" => CheckResult::Skipped {
            reason:
                "Google AI Studio uses a query-param key shape; key persisted without round-trip.",
        },
        _ => CheckResult::Skipped {
            reason: "Provider kind doesn't have a known round-trip; key persisted as-is.",
        },
    }
}

fn execute(req: ureq::Request, key_format_hint: &str) -> CheckResult {
    match req.call() {
        Ok(resp) => {
            let code = resp.status();
            if (200..300).contains(&code) {
                CheckResult::Ok
            } else {
                CheckResult::HttpError {
                    status: code,
                    hint: format!("expected 2xx; check the key format: `{key_format_hint}`"),
                }
            }
        }
        Err(ureq::Error::Status(code, _)) => CheckResult::HttpError {
            status: code,
            hint: format!("check the key format: `{key_format_hint}`"),
        },
        Err(ureq::Error::Transport(t)) => CheckResult::Network {
            message: format!("{t}"),
        },
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use std::net::TcpListener;

    fn provider_oai(endpoint: &str) -> Provider {
        Provider {
            id: "openrouter".into(),
            display_name: "OpenRouter".into(),
            kind: "openai-compatible".into(),
            endpoint_default: endpoint.into(),
            key_env_var: "OPENROUTER_API_KEY".into(),
            key_format: "sk-or-v1-...".into(),
            default_model: String::new(),
            supported_models_hint: String::new(),
        }
    }

    /// Spin up a one-shot tokio axum server that returns the supplied status
    /// code at `/models`. Returns the bound address and a JoinHandle.
    fn spawn_mock(
        status: u16,
    ) -> (
        std::net::SocketAddr,
        tokio::runtime::Runtime,
        tokio::task::JoinHandle<()>,
    ) {
        let rt = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .unwrap();
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        listener.set_nonblocking(true).unwrap();
        let tokio_listener =
            rt.block_on(async { tokio::net::TcpListener::from_std(listener).unwrap() });
        let app = axum::Router::new().route(
            "/models",
            axum::routing::get(move || async move {
                let body = if (200..300).contains(&status) {
                    r#"{"data":[]}"#
                } else {
                    r#"{"error":"nope"}"#
                };
                (
                    axum::http::StatusCode::from_u16(status).unwrap(),
                    [(axum::http::header::CONTENT_TYPE, "application/json")],
                    body,
                )
            }),
        );
        let handle = rt.spawn(async move {
            let _ = axum::serve(tokio_listener, app).await;
        });
        (addr, rt, handle)
    }

    #[test]
    fn happy_path_200() {
        let (addr, rt, handle) = spawn_mock(200);
        let endpoint = format!("http://{addr}");
        let p = provider_oai(&endpoint);
        let r = check_with(&p, "sk-or-v1-test", &endpoint, Duration::from_secs(2));
        assert!(matches!(r, CheckResult::Ok), "got {r:?}");
        handle.abort();
        drop(rt);
    }

    #[test]
    fn sad_path_401() {
        let (addr, rt, handle) = spawn_mock(401);
        let endpoint = format!("http://{addr}");
        let p = provider_oai(&endpoint);
        let r = check_with(&p, "bogus", &endpoint, Duration::from_secs(2));
        match r {
            CheckResult::HttpError { status, .. } => assert_eq!(status, 401),
            other => panic!("expected HttpError(401), got {other:?}"),
        }
        handle.abort();
        drop(rt);
    }

    #[test]
    fn timeout_or_refused() {
        // Bind a port, drop the listener, and use that address — the OS will
        // give us back a port nothing is listening on. Connection refused
        // surfaces as Network, which is what we want.
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        drop(listener);
        let endpoint = format!("http://{addr}");
        let p = provider_oai(&endpoint);
        let r = check_with(&p, "k", &endpoint, Duration::from_millis(500));
        assert!(
            matches!(r, CheckResult::Network { .. }),
            "expected Network, got {r:?}"
        );
    }

    #[test]
    fn bedrock_is_skipped() {
        let mut p = provider_oai("https://bedrock-runtime.us-east-1.amazonaws.com");
        p.kind = "bedrock".into();
        p.id = "bedrock".into();
        let r = check(&p, "AKIAFAKE");
        assert!(matches!(r, CheckResult::Skipped { .. }));
        assert!(r.is_ok());
    }
}
