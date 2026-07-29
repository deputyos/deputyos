//! The deputyOS API client — device-code login, token lifecycle, fleet.
//!
//! Mirrors the contract proven in `deputywizard/src/routes.rs`
//! (`request_device_code` / `poll_and_register`) and the API route shapes in
//! `api-deputyos-com/api/src/{auth,accounts}.rs`. Pure HTTP — no token
//! persistence here (that's [`crate::store`]); callers pass tokens as
//! strings, so this module is fully unit-testable with `httpmock`.
//!
//! ### Contract (the endpoints we hit)
//!
//! - `POST /api/v1/auth/device-code?client_name=` →
//!   `{ device_code, user_code, verification_uri, expires_in, interval }`
//! - `POST /api/v1/auth/device-token` body `{ device_code }` →
//!   `{ access_token, refresh_token, expires_in }` (200), or
//!   `400 authorization_pending` while the user hasn't confirmed.
//! - `POST /api/v1/auth/refresh` body `{ refresh_token }` → token pair.
//! - `POST /api/v1/auth/revoke` body `{ refresh_token, access_token? }` → 200.
//! - `GET  /api/v1/accounts/devices` (Bearer) → `[{ id, name, created_at,
//!   revoked_at }]`.
//! - `POST /api/v1/accounts/devices/register` (Bearer) body `{ device_name }`
//!   → `{ device_id, tunnel_token, backup_token }`.
//! - `POST /api/v1/accounts/devices/revoke` (Bearer) body `{ device_id }`.
//!
//! The token response does not carry `account_id`; we decode it from the
//! JWT `sub` claim ([`jwt_subject`]) for display — non-trusting, since the
//! signature is verified server-side on every call.

use anyhow::{anyhow, bail, Context, Result};
use serde::{Deserialize, Serialize};

/// Default API base. Override per-client (tests point at an httpmock origin).
pub const DEFAULT_API_BASE: &str = "https://api.deputyos.com";

/// A thin deputyOS API client. Stateless beyond the base URL + agent config.
#[derive(Debug, Clone)]
pub struct ApiClient {
    base: String,
    http: ureq::Agent,
}

impl ApiClient {
    /// Construct a client for `base` (e.g. [`DEFAULT_API_BASE`] or an
    /// httpmock URL in tests).
    pub fn new(base: &str) -> Self {
        Self {
            base: base.trim_end_matches('/').to_string(),
            http: ureq::AgentBuilder::new()
                .timeout(std::time::Duration::from_secs(30))
                .build(),
        }
    }

    pub fn base(&self) -> &str {
        &self.base
    }

    /// `POST /auth/device-code?client_name=` — start a device-code login.
    pub fn device_code_start(&self, client_name: &str) -> Result<DeviceCodeStart> {
        let url = format!("{}/api/v1/auth/device-code", self.base);
        let resp = self
            .http
            .post(&url)
            .query("client_name", client_name)
            .call();
        let txt = read_body(resp).context("device-code request")?;
        let parsed: DeviceCodeStartResp = serde_json::from_str(&txt)
            .with_context(|| format!("device-code bad response: {txt}"))?;
        Ok(DeviceCodeStart {
            device_code: parsed.device_code,
            user_code: parsed.user_code,
            verification_uri: parsed.verification_uri,
            expires_in: parsed.expires_in,
            interval: parsed.interval,
        })
    }

    /// `POST /auth/device-token` — poll once. Returns [`PollOutcome::Pending`]
    /// while the user hasn't confirmed, or [`PollOutcome::Authorized`] with
    /// the token pair + the account id (decoded from the JWT `sub`).
    pub fn device_code_poll(&self, device_code: &str) -> Result<PollOutcome> {
        let url = format!("{}/api/v1/auth/device-token", self.base);
        let body = serde_json::to_string(&DeviceTokenReq { device_code })
            .context("encoding device-token request")?;
        let resp = self
            .http
            .post(&url)
            .set("Content-Type", "application/json")
            .send_string(&body);
        match resp {
            Ok(r) => {
                let txt = r.into_string().unwrap_or_default();
                let t: TokenResp = serde_json::from_str(&txt)
                    .with_context(|| format!("device-token bad response: {txt}"))?;
                Ok(PollOutcome::Authorized(token_pair_from_resp(&t)))
            }
            Err(ureq::Error::Status(400, r)) => {
                let b = r.into_string().unwrap_or_default();
                if b.contains("authorization_pending") {
                    Ok(PollOutcome::Pending)
                } else {
                    Err(anyhow!("device-token 400: {b}"))
                }
            }
            Err(e) => Err(read_err(e).context("device-token request")),
        }
    }

    /// `POST /auth/refresh` — exchange a refresh token for a fresh pair.
    pub fn refresh(&self, refresh_token: &str) -> Result<TokenPair> {
        let url = format!("{}/api/v1/auth/refresh", self.base);
        let body = serde_json::to_string(&RefreshReq { refresh_token })
            .context("encoding refresh request")?;
        let resp = self
            .http
            .post(&url)
            .set("Content-Type", "application/json")
            .send_string(&body);
        let txt = read_body(resp).context("refresh request")?;
        let t: TokenResp =
            serde_json::from_str(&txt).with_context(|| format!("refresh bad response: {txt}"))?;
        Ok(token_pair_from_resp(&t))
    }

    /// `POST /auth/revoke` — revoke a refresh token (and optionally its
    /// access token). Best-effort; errors if the API refuses.
    pub fn revoke(&self, refresh_token: &str, access_token: Option<&str>) -> Result<()> {
        let url = format!("{}/api/v1/auth/revoke", self.base);
        let body = serde_json::to_string(&RevokeReq {
            refresh_token,
            access_token,
        })
        .context("encoding revoke request")?;
        let resp = self
            .http
            .post(&url)
            .set("Content-Type", "application/json")
            .send_string(&body);
        match resp {
            Ok(_) => Ok(()),
            Err(ureq::Error::Status(c, r)) => {
                // Revoking an already-revoked/unknown token is a no-op success
                // server-side; surface other statuses as errors.
                if (200..300).contains(&c) {
                    Ok(())
                } else {
                    Err(anyhow!(
                        "revoke {c}: {}",
                        r.into_string().unwrap_or_default()
                    ))
                }
            }
            Err(e) => Err(read_err(e).context("revoke request")),
        }
    }

    /// `GET /accounts/devices` (Bearer) — list the account's devices (fleet).
    pub fn list_devices(&self, access_token: &str) -> Result<Vec<DeviceEntry>> {
        let url = format!("{}/api/v1/accounts/devices", self.base);
        let resp = self
            .http
            .get(&url)
            .set("Authorization", &format!("Bearer {access_token}"))
            .call();
        let txt = read_body(resp).context("list_devices request")?;
        let devs: Vec<DeviceEntry> = serde_json::from_str(&txt)
            .with_context(|| format!("list_devices bad response: {txt}"))?;
        Ok(devs)
    }

    /// `POST /accounts/devices/register` (Bearer) — register a new device
    /// under the account, returning the tunnel/backup tokens to install.
    pub fn register_device(&self, access_token: &str, device_name: &str) -> Result<DeviceRegister> {
        let url = format!("{}/api/v1/accounts/devices/register", self.base);
        let body = serde_json::to_string(&RegisterDeviceReq { device_name })
            .context("encoding register request")?;
        let resp = self
            .http
            .post(&url)
            .set("Authorization", &format!("Bearer {access_token}"))
            .set("Content-Type", "application/json")
            .send_string(&body);
        let txt = read_body(resp).context("register_device request")?;
        let r: DeviceRegisterResp = serde_json::from_str(&txt)
            .with_context(|| format!("register_device bad response: {txt}"))?;
        Ok(DeviceRegister {
            device_id: r.device_id,
            tunnel_token: r.tunnel_token,
            backup_token: r.backup_token,
        })
    }

    /// `POST /accounts/devices/revoke` (Bearer) — revoke a device by id.
    pub fn revoke_device(&self, access_token: &str, device_id: &str) -> Result<()> {
        let url = format!("{}/api/v1/accounts/devices/revoke", self.base);
        let body = serde_json::to_string(&RevokeDeviceReq { device_id })
            .context("encoding revoke-device request")?;
        let resp = self
            .http
            .post(&url)
            .set("Authorization", &format!("Bearer {access_token}"))
            .set("Content-Type", "application/json")
            .send_string(&body);
        match resp {
            Ok(_) => Ok(()),
            Err(e) => Err(read_err(e).context("revoke_device request")),
        }
    }

    /// Queue one typed resident-agent operation. Delivery is asynchronous so
    /// this also works while a deputy is offline.
    pub fn enqueue_command(
        &self,
        access_token: &str,
        device_id: &str,
        command: &str,
        params: serde_json::Value,
        idempotency_key: &str,
    ) -> Result<RemoteCommand> {
        let url = format!("{}/api/v1/devices/{device_id}/commands", self.base);
        let body = serde_json::json!({
            "command": command,
            "params": params,
            "idempotency_key": idempotency_key,
            "expires_in_secs": 900
        });
        let response = self
            .http
            .post(&url)
            .set("Authorization", &format!("Bearer {access_token}"))
            .set("Content-Type", "application/json")
            .send_string(&body.to_string());
        let text = read_body(response).context("enqueue_command request")?;
        serde_json::from_str(&text).with_context(|| format!("enqueue_command bad response: {text}"))
    }

    /// Build the tunnel proxy URL for a remote device's path. Per Phase C the
    /// proxy path is keyed by `device_id` (not account name), so a remote
    /// wizard opens at `…/tunnel/proxy/<device_id>/<path>`. `path` is the
    /// in-wizard path (e.g. `""` for the wizard root, `"chat"` for chat).
    pub fn tunnel_proxy_url(&self, device_id: &str, path: &str) -> String {
        let p = path.trim_start_matches('/');
        format!("{}/api/v1/tunnel/proxy/{}/{}", self.base, device_id, p)
    }

    /// Origin-correct production URL for a native tunnel surface. Dedicated
    /// wildcard origins allow root-relative assets and WebSockets to work
    /// unchanged. Non-production API bases retain the path proxy for local
    /// development and tests.
    pub fn tunnel_surface_url(&self, device_id: &str, surface: &str) -> Result<String> {
        if !device_id
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == '-')
        {
            anyhow::bail!("device id cannot be represented in a tunnel hostname");
        }
        let prefix = match surface {
            "webui" => "",
            "control" => "control-",
            "terminal" => "terminal-",
            _ => anyhow::bail!("unknown remote surface"),
        };
        if self.base == DEFAULT_API_BASE {
            Ok(format!("https://{prefix}{device_id}.tunnel.deputyos.com/"))
        } else {
            let path = match surface {
                "webui" => "",
                "control" => "_deputyos/control/",
                "terminal" => "_deputyos/terminal/",
                _ => unreachable!(),
            };
            Ok(self.tunnel_proxy_url(device_id, path))
        }
    }
}

/// A device-code login start result.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeviceCodeStart {
    pub device_code: String,
    pub user_code: String,
    pub verification_uri: String,
    pub expires_in: u64,
    pub interval: u64,
}

/// One poll outcome from [`ApiClient::device_code_poll`].
#[derive(Debug, Clone)]
pub enum PollOutcome {
    /// User hasn't confirmed the code yet — keep polling at `interval`.
    Pending,
    /// Authorized — tokens issued.
    Authorized(TokenPair),
}

/// An access/refresh token pair with the issuing account id (decoded from the
/// JWT `sub`) and the access token's remaining lifetime (seconds).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TokenPair {
    pub access_token: String,
    pub refresh_token: String,
    pub expires_in: u64,
    /// The account id this token represents (the JWT `sub`). `None` if the
    /// JWT payload couldn't be decoded (the token still works — the API
    /// verifies the signature).
    #[serde(default)]
    pub account_id: Option<String>,
}

/// A fleet device entry (subset of the API's `DeviceEntry`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeviceEntry {
    pub id: String,
    pub name: String,
    pub created_at: String,
    #[serde(default)]
    pub revoked_at: Option<String>,
    #[serde(default)]
    pub tunnel_online: bool,
}

/// A freshly-registered device's credentials.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeviceRegister {
    pub device_id: String,
    pub tunnel_token: String,
    pub backup_token: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RemoteCommand {
    pub id: String,
    pub command: String,
    pub status: String,
    #[serde(default)]
    pub created_at: String,
    #[serde(default)]
    pub expires_at: Option<String>,
}

// ---- request/response wire shapes (private) ----

#[derive(Serialize)]
struct DeviceTokenReq<'a> {
    device_code: &'a str,
}

#[derive(Deserialize)]
struct DeviceCodeStartResp {
    device_code: String,
    user_code: String,
    verification_uri: String,
    #[serde(default = "default_expires_in")]
    expires_in: u64,
    #[serde(default = "default_interval")]
    interval: u64,
}

fn default_expires_in() -> u64 {
    900
}
fn default_interval() -> u64 {
    5
}

#[derive(Deserialize)]
struct TokenResp {
    access_token: String,
    #[serde(default)]
    refresh_token: String,
    #[serde(default = "default_expires_in")]
    expires_in: u64,
}

#[derive(Serialize)]
struct RefreshReq<'a> {
    refresh_token: &'a str,
}

#[derive(Serialize)]
struct RevokeReq<'a> {
    refresh_token: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    access_token: Option<&'a str>,
}

#[derive(Serialize)]
struct RegisterDeviceReq<'a> {
    device_name: &'a str,
}

#[derive(Serialize)]
struct RevokeDeviceReq<'a> {
    device_id: &'a str,
}

#[derive(Deserialize)]
struct DeviceRegisterResp {
    device_id: String,
    tunnel_token: String,
    backup_token: String,
}

fn token_pair_from_resp(t: &TokenResp) -> TokenPair {
    TokenPair {
        access_token: t.access_token.clone(),
        refresh_token: t.refresh_token.clone(),
        expires_in: t.expires_in,
        account_id: jwt_subject(&t.access_token),
    }
}

/// Decode the `sub` claim from a JWT's payload, without signature verification.
/// Pure display keying — the API verifies the signature on each call, so we
/// only need to read the account id locally. Returns `None` on any malformation.
pub fn jwt_subject(token: &str) -> Option<String> {
    use base64::engine::general_purpose::URL_SAFE_NO_PAD;
    use base64::Engine;
    let mut parts = token.split('.');
    let _header = parts.next()?;
    let payload = parts.next()?;
    let bytes = URL_SAFE_NO_PAD.decode(payload).ok()?;
    let value: serde_json::Value = serde_json::from_slice(&bytes).ok()?;
    value.get("sub")?.as_str().map(|s| s.to_string())
}

/// Read the response body on success, or convert a ureq error into an anyhow
/// error with the status + body.
fn read_body(resp: Result<ureq::Response, ureq::Error>) -> Result<String> {
    match resp {
        Ok(r) => Ok(r.into_string().unwrap_or_default()),
        Err(ureq::Error::Status(c, r)) => {
            bail!("HTTP {c}: {}", r.into_string().unwrap_or_default())
        }
        Err(e) => Err(read_err(e)),
    }
}

/// Transport errors carry no body; surface them directly.
fn read_err(e: ureq::Error) -> anyhow::Error {
    match e {
        ureq::Error::Status(c, r) => anyhow!("HTTP {c}: {}", r.into_string().unwrap_or_default()),
        ureq::Error::Transport(t) => anyhow!("transport: {t}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use httpmock::MockServer;

    fn mock_jwt(sub: &str) -> String {
        // header.payload.sig — only payload content matters for jwt_subject.
        let payload = serde_json::json!({ "sub": sub }).to_string();
        use base64::engine::general_purpose::URL_SAFE_NO_PAD;
        use base64::Engine;
        format!("header.{}.sig", URL_SAFE_NO_PAD.encode(payload))
    }

    #[test]
    fn device_code_start_parses_response() {
        let s = MockServer::start();
        s.mock(|w, t| {
            w.method(httpmock::Method::POST)
                .path("/api/v1/auth/device-code");
            t.status(200).json_body(serde_json::json!({
                "device_code": "dc-123",
                "user_code": "ABCD-WXYZ",
                "verification_uri": "https://app.deputyos.com/device",
                "expires_in": 900,
                "interval": 5,
            }));
        });
        let c = ApiClient::new(&s.base_url());
        let r = c.device_code_start("deputyos-console").expect("ok");
        assert_eq!(r.device_code, "dc-123");
        assert_eq!(r.user_code, "ABCD-WXYZ");
        assert_eq!(r.expires_in, 900);
        assert_eq!(r.interval, 5);
    }

    #[test]
    fn device_code_poll_pending_then_authorized() {
        let s = MockServer::start();
        s.mock(|w, t| {
            w.method(httpmock::Method::POST)
                .path("/api/v1/auth/device-token");
            t.status(400).body("authorization_pending");
        });
        let c = ApiClient::new(&s.base_url());
        match c.device_code_poll("dc").expect("pending ok") {
            PollOutcome::Pending => {}
            other => panic!("expected Pending, got {other:?}"),
        }

        // Second server with an authorized response.
        let s2 = MockServer::start();
        let jwt = mock_jwt("acct-42");
        s2.mock(|w, t| {
            w.method(httpmock::Method::POST)
                .path("/api/v1/auth/device-token");
            t.status(200).json_body(serde_json::json!({
                "access_token": jwt,
                "refresh_token": "rt-abc",
                "expires_in": 900,
            }));
        });
        let c2 = ApiClient::new(&s2.base_url());
        match c2.device_code_poll("dc").expect("auth ok") {
            PollOutcome::Authorized(tp) => {
                assert_eq!(tp.refresh_token, "rt-abc");
                assert_eq!(tp.expires_in, 900);
                assert_eq!(tp.account_id.as_deref(), Some("acct-42"));
            }
            PollOutcome::Pending => panic!("expected Authorized"),
        }
    }

    #[test]
    fn refresh_returns_new_pair() {
        let s = MockServer::start();
        let jwt = mock_jwt("acct-7");
        s.mock(|w, t| {
            w.method(httpmock::Method::POST)
                .path("/api/v1/auth/refresh");
            t.status(200).json_body(serde_json::json!({
                "access_token": jwt,
                "refresh_token": "rt-new",
                "expires_in": 900,
            }));
        });
        let c = ApiClient::new(&s.base_url());
        let tp = c.refresh("rt-old").expect("ok");
        assert_eq!(tp.refresh_token, "rt-new");
        assert_eq!(tp.account_id.as_deref(), Some("acct-7"));
    }

    #[test]
    fn list_devices_parses_entries() {
        let s = MockServer::start();
        s.mock(|w, t| {
            w.method(httpmock::Method::GET).path("/api/v1/accounts/devices");
            t.status(200).json_body(serde_json::json!([
                { "id": "dev-1", "name": "laptop", "created_at": "2026-06-22T00:00:00Z", "revoked_at": null },
                { "id": "dev-2", "name": "vm", "created_at": "2026-06-23T00:00:00Z", "revoked_at": "2026-06-23T12:00:00Z" },
            ]));
        });
        let c = ApiClient::new(&s.base_url());
        let devs = c.list_devices("tok").expect("ok");
        assert_eq!(devs.len(), 2);
        assert_eq!(devs[0].id, "dev-1");
        assert!(devs[0].revoked_at.is_none());
        assert_eq!(devs[1].revoked_at.as_deref(), Some("2026-06-23T12:00:00Z"));
    }

    #[test]
    fn register_device_returns_tokens() {
        let s = MockServer::start();
        s.mock(|w, t| {
            w.method(httpmock::Method::POST)
                .path("/api/v1/accounts/devices/register");
            t.status(200).json_body(serde_json::json!({
                "device_id": "dev-9",
                "tunnel_token": "tt",
                "backup_token": "bt",
            }));
        });
        let c = ApiClient::new(&s.base_url());
        let r = c.register_device("tok", "my-laptop").expect("ok");
        assert_eq!(r.device_id, "dev-9");
        assert_eq!(r.tunnel_token, "tt");
        assert_eq!(r.backup_token, "bt");
    }

    #[test]
    fn enqueue_command_uses_typed_idempotent_contract() {
        let server = MockServer::start();
        let mock = server.mock(|when, then| {
            when.method(httpmock::Method::POST)
                .path("/api/v1/devices/dev-1/commands")
                .header("Authorization", "Bearer token")
                .json_body(serde_json::json!({
                    "command": "repair.run",
                    "params": {},
                    "idempotency_key": "console:repair:1",
                    "expires_in_secs": 900
                }));
            then.status(200).json_body(serde_json::json!({
                "id": "cmd-1",
                "command": "repair.run",
                "status": "pending",
                "created_at": "1",
                "expires_at": "901"
            }));
        });
        let client = ApiClient::new(&server.base_url());
        let command = client
            .enqueue_command(
                "token",
                "dev-1",
                "repair.run",
                serde_json::json!({}),
                "console:repair:1",
            )
            .expect("enqueue");
        assert_eq!(command.id, "cmd-1");
        assert_eq!(command.status, "pending");
        mock.assert_hits(1);
    }

    #[test]
    fn revoke_device_is_ok_on_200() {
        let s = MockServer::start();
        s.mock(|w, t| {
            w.method(httpmock::Method::POST)
                .path("/api/v1/accounts/devices/revoke");
            t.status(200);
        });
        let c = ApiClient::new(&s.base_url());
        c.revoke_device("tok", "dev-9").expect("ok");
    }

    #[test]
    fn tunnel_proxy_url_is_device_id_keyed() {
        let c = ApiClient::new("https://api.deputyos.com");
        assert_eq!(
            c.tunnel_proxy_url("dev-1", ""),
            "https://api.deputyos.com/api/v1/tunnel/proxy/dev-1/"
        );
        assert_eq!(
            c.tunnel_proxy_url("dev-1", "chat"),
            "https://api.deputyos.com/api/v1/tunnel/proxy/dev-1/chat"
        );
        assert_eq!(
            c.tunnel_proxy_url("dev-1", "/wizard"),
            "https://api.deputyos.com/api/v1/tunnel/proxy/dev-1/wizard"
        );
    }

    #[test]
    fn production_tunnel_surfaces_have_dedicated_origins() {
        let c = ApiClient::new(DEFAULT_API_BASE);
        assert_eq!(
            c.tunnel_surface_url("dev-1", "webui").expect("webui"),
            "https://dev-1.tunnel.deputyos.com/"
        );
        assert_eq!(
            c.tunnel_surface_url("dev-1", "terminal").expect("terminal"),
            "https://terminal-dev-1.tunnel.deputyos.com/"
        );
        assert_eq!(
            c.tunnel_surface_url("dev-1", "control").expect("control"),
            "https://control-dev-1.tunnel.deputyos.com/"
        );
    }

    #[test]
    fn jwt_subject_decodes_sub() {
        assert_eq!(jwt_subject(&mock_jwt("acct-1")).as_deref(), Some("acct-1"));
        // Malformed tokens yield None, not a panic.
        assert_eq!(jwt_subject("not-a-jwt"), None);
        assert_eq!(jwt_subject("a.b"), None);
    }

    #[test]
    fn api_base_is_trimmed_of_trailing_slash() {
        let c = ApiClient::new("https://api.test/");
        assert_eq!(c.base(), "https://api.test");
    }
}
