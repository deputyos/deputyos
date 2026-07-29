//! Axum router + handlers for the PWA.
//!
//! Auth model: M3-PWA is LAN-trusted. Same trust model the wizard hands off
//! when it finishes — operators on the LAN are assumed allowed. Production
//! bakes can layer Tailscale or Cloudflare Tunnel for stronger auth without
//! changing this code.
//!
//! Subprocess invocations to `deputyctl --json` are wrapped in
//! `tokio::task::spawn_blocking` so the runtime stays responsive.

use std::path::PathBuf;
use std::sync::Arc;

use axum::{
    extract::{Form, Query, State},
    http::{header, StatusCode},
    response::{Html, IntoResponse, Redirect, Response},
    routing::{get, post},
    Json, Router,
};
use serde::Deserialize;

use crate::data;
use crate::push::{self, PushSubscription};
use crate::templates;

#[derive(Clone)]
pub struct AppState {
    /// Where to read/write the VAPID keypair. None means push is disabled
    /// (openssl wasn't available at boot).
    pub vapid_public_b64url: Arc<Option<String>>,
    /// In-memory flash slot for the keys page. Single-shot per request.
    pub flash: Arc<std::sync::Mutex<Option<String>>>,
    /// Subscriptions file path override (tests pin this to a tempdir).
    pub subscriptions_path: Option<PathBuf>,
}

impl AppState {
    pub fn new() -> Self {
        Self {
            vapid_public_b64url: Arc::new(None),
            flash: Arc::new(std::sync::Mutex::new(None)),
            subscriptions_path: None,
        }
    }

    pub fn with_vapid(mut self, vapid: Option<String>) -> Self {
        self.vapid_public_b64url = Arc::new(vapid);
        self
    }

    pub fn with_subscriptions_path(mut self, path: PathBuf) -> Self {
        self.subscriptions_path = Some(path);
        self
    }
}

impl Default for AppState {
    fn default() -> Self {
        Self::new()
    }
}

/// Top-level router.
pub fn router(state: AppState) -> Router {
    let style_css = templates::STYLE_CSS;
    let manifest = templates::MANIFEST_JSON;
    let icon = templates::ICON_SVG;
    let sw = templates::SERVICE_WORKER_JS;

    Router::new()
        .route("/", get(|| async { Redirect::to("/app/dashboard") }))
        .route("/healthz", get(|| async { "ok" }))
        .route("/app/dashboard", get(get_dashboard))
        .route("/app/logs", get(get_logs))
        .route("/app/keys", get(get_keys))
        .route("/app/keys/rotate", post(post_keys_rotate))
        .route("/app/mounts", get(get_mounts))
        .route("/app/mounts/remove", post(post_mounts_remove))
        .route("/app/network", get(get_network))
        .route("/app/tunnel", get(get_tunnel))
        .route("/app/account", get(get_account))
        .route("/app/push/subscribe", post(post_push_subscribe))
        .route("/app/push/vapid-public", get(get_vapid_public))
        .route("/app/cost/raise-cap", post(post_cost_raise_cap))
        .route("/app/reset-cost-trip", post(post_reset_cost_trip))
        .route(
            "/manifest.webmanifest",
            get(move || async move {
                (
                    [(header::CONTENT_TYPE, "application/manifest+json")],
                    manifest,
                )
            }),
        )
        .route(
            "/sw.js",
            get(move || async move { ([(header::CONTENT_TYPE, "text/javascript")], sw) }),
        )
        .route(
            "/static/style.css",
            get(move || async move {
                (
                    [(header::CONTENT_TYPE, "text/css; charset=utf-8")],
                    style_css,
                )
            }),
        )
        .route(
            "/static/icon.svg",
            get(move || async move { ([(header::CONTENT_TYPE, "image/svg+xml")], icon) }),
        )
        .with_state(state)
}

// ---------------------------------------------------------------------------
// Dashboard
// ---------------------------------------------------------------------------

async fn get_dashboard(State(_app): State<AppState>) -> Response {
    let dashboard = tokio::task::spawn_blocking(data::fetch_dashboard)
        .await
        .unwrap_or_else(|_| data::Dashboard {
            status: Default::default(),
            version: Default::default(),
            limits: Default::default(),
            cost: Default::default(),
            doctor: Default::default(),
            network: Default::default(),
            stub: true,
        });
    Html(templates::dashboard(&dashboard)).into_response()
}

// ---------------------------------------------------------------------------
// Logs
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize, Default)]
struct LogQuery {
    #[serde(default)]
    lines: Option<usize>,
}

async fn get_logs(State(_app): State<AppState>, Query(q): Query<LogQuery>) -> Response {
    let lines = q.lines.unwrap_or(100).clamp(10, 1000);
    let dash = tokio::task::spawn_blocking(data::fetch_dashboard)
        .await
        .unwrap_or_else(|_| data::Dashboard {
            status: Default::default(),
            version: Default::default(),
            limits: Default::default(),
            cost: Default::default(),
            doctor: Default::default(),
            network: Default::default(),
            stub: true,
        });
    let unit = if dash.status.unit.is_empty() {
        "openclaw.service".to_string()
    } else {
        dash.status.unit.clone()
    };
    let unit_for_blocking = unit.clone();
    let body =
        tokio::task::spawn_blocking(move || data::fetch_journal_tail(&unit_for_blocking, lines))
            .await
            .unwrap_or_else(|_| "(journal task failed)".into());
    Html(templates::logs_page(&unit, lines, &body, dash.stub)).into_response()
}

// ---------------------------------------------------------------------------
// Provider keys
// ---------------------------------------------------------------------------

async fn get_keys(State(app): State<AppState>) -> Response {
    let providers = tokio::task::spawn_blocking(data::fetch_providers)
        .await
        .unwrap_or_default();
    let stub = crate::paths::dev_stub_enabled() || crate::paths::which_deputyctl().is_none();
    let flash = app.flash.lock().expect("flash lock").take();
    Html(templates::keys_page(&providers, flash.as_deref(), stub)).into_response()
}

#[derive(Debug, Deserialize)]
struct RotateForm {
    provider: String,
    api_key: String,
}

async fn post_keys_rotate(State(app): State<AppState>, Form(f): Form<RotateForm>) -> Response {
    let provider = f.provider.trim().to_string();
    let key = f.api_key;
    if provider.is_empty() || key.trim().is_empty() {
        *app.flash.lock().expect("flash lock") = Some("Provider and key are both required.".into());
        return Redirect::to("/app/keys").into_response();
    }
    // In dev-stub mode (no deputyctl on PATH) we can't actually rotate. Pretend
    // we did so the UI is exercisable.
    if crate::paths::dev_stub_enabled() || crate::paths::which_deputyctl().is_none() {
        *app.flash.lock().expect("flash lock") = Some(format!(
            "(dev-stub) would rotate {provider} via `deputyctl model set --provider {provider} --key-from-stdin --yes`"
        ));
        return Redirect::to("/app/keys").into_response();
    }
    let bin = crate::paths::which_deputyctl().expect("checked above");
    // Run blocking subprocess on the blocking pool so we don't stall the
    // axum executor.
    let result = tokio::task::spawn_blocking(move || rotate_via_deputyctl(&bin, &provider, &key))
        .await
        .unwrap_or_else(|e| Err(format!("rotate task panicked: {e}")));
    let msg = match result {
        Ok(provider_id) => format!("Rotated {provider_id} successfully."),
        Err(e) => format!("Rotation failed: {e}"),
    };
    *app.flash.lock().expect("flash lock") = Some(msg);
    Redirect::to("/app/keys").into_response()
}

// ---------------------------------------------------------------------------
// Mounts (M3.5)
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct MountsRemoveForm {
    id: String,
}

async fn get_mounts(State(app): State<AppState>) -> Response {
    let flash = app.flash.lock().expect("flash lock").take();
    let card = tokio::task::spawn_blocking(data::fetch_mounts)
        .await
        .unwrap_or_default();
    Html(templates::mounts_page(&card.entries, flash.as_deref())).into_response()
}

// ---------------------------------------------------------------------------
// Network (M5.5)
// ---------------------------------------------------------------------------

async fn get_network(State(_app): State<AppState>) -> Response {
    let network = tokio::task::spawn_blocking(|| {
        let bin = crate::paths::which_deputyctl()
            .unwrap_or_else(|| std::path::PathBuf::from("deputyctl"));
        data::run_raw(&bin, &["network", "status", "--json"])
    })
    .await
    .ok()
    .and_then(|r| r.ok())
    .and_then(|raw| serde_json::from_str::<serde_json::Value>(&raw).ok())
    .unwrap_or_default();
    let entries = tokio::task::spawn_blocking(|| deputyctl::mounts::list(None))
        .await
        .unwrap_or_else(|_| Ok(Vec::new()))
        .unwrap_or_default();
    Html(templates::network_page(&network, &entries)).into_response()
}

// ---------------------------------------------------------------------------
// Tunnel (M8) — integrated cloud relay state + copy-able public URL
// ---------------------------------------------------------------------------

async fn get_tunnel(State(_app): State<AppState>) -> Response {
    let card = tokio::task::spawn_blocking(data::fetch_tunnel)
        .await
        .unwrap_or_else(|_| data::TunnelCard {
            kind: "none".into(),
            ..data::TunnelCard::default()
        });
    Html(templates::tunnel_page(&card)).into_response()
}

// ---------------------------------------------------------------------------
// Account (M8) — device identity + token presence (booleans only)
// ---------------------------------------------------------------------------

async fn get_account(State(_app): State<AppState>) -> Response {
    let card = tokio::task::spawn_blocking(data::fetch_account)
        .await
        .unwrap_or_default();
    Html(templates::account_page(&card)).into_response()
}

async fn post_mounts_remove(
    State(app): State<AppState>,
    Form(f): Form<MountsRemoveForm>,
) -> Response {
    let id = f.id.clone();
    let result = tokio::task::spawn_blocking(move || deputyctl::mounts::remove_by_id(None, &f.id))
        .await
        .unwrap_or_else(|e| Err(anyhow::anyhow!("remove task panicked: {e}")));
    let msg = match result {
        Ok(_) => format!("Revoked mount {id:?}."),
        Err(e) => format!("Could not revoke {id:?}: {e}"),
    };
    *app.flash.lock().expect("flash lock") = Some(msg);
    Redirect::to("/app/mounts").into_response()
}

fn rotate_via_deputyctl(
    bin: &std::path::Path,
    provider: &str,
    api_key: &str,
) -> std::result::Result<String, String> {
    use std::io::Write;
    use std::process::{Command, Stdio};
    let mut child = Command::new(bin)
        .args([
            "model",
            "set",
            "--provider",
            provider,
            "--key-from-stdin",
            "--yes",
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("spawn deputyctl: {e}"))?;
    if let Some(mut s) = child.stdin.take() {
        s.write_all(api_key.as_bytes())
            .map_err(|e| format!("write stdin: {e}"))?;
    }
    let out = child
        .wait_with_output()
        .map_err(|e| format!("wait deputyctl: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "deputyctl model set exited {}: {}",
            out.status,
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    Ok(provider.to_string())
}

// ---------------------------------------------------------------------------
// Push
// ---------------------------------------------------------------------------

async fn post_push_subscribe(
    State(app): State<AppState>,
    Json(sub): Json<PushSubscription>,
) -> Response {
    let path = app
        .subscriptions_path
        .clone()
        .unwrap_or_else(crate::paths::push_subscriptions_path);
    let result = tokio::task::spawn_blocking(move || push::append_subscription_to(&path, &sub))
        .await
        .unwrap_or_else(|e| Err(anyhow::anyhow!("subscribe task panicked: {e}")));
    match result {
        Ok(_) => (StatusCode::CREATED, "ok").into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            templates::error_page("Subscription failed", &e.to_string()),
        )
            .into_response(),
    }
}

async fn get_vapid_public(State(app): State<AppState>) -> Response {
    let key = app
        .vapid_public_b64url
        .as_ref()
        .as_deref()
        .unwrap_or("")
        .to_string();
    ([(header::CONTENT_TYPE, "text/plain; charset=utf-8")], key).into_response()
}

// ---- cost guardrails (M5) ----

#[derive(Debug, Deserialize)]
struct RaiseCapForm {
    daily_cap: Option<f64>,
}

async fn post_cost_raise_cap(
    axum::extract::Form(f): axum::extract::Form<RaiseCapForm>,
) -> Response {
    let cap = f.daily_cap.unwrap_or(10.0);
    if cap <= 0.0 {
        return (
            StatusCode::BAD_REQUEST,
            [(header::CONTENT_TYPE, "text/html; charset=utf-8")],
            "daily_cap must be positive. <a href=\"/app/dashboard\">Back</a>",
        )
            .into_response();
    }
    let result = tokio::task::spawn_blocking(move || {
        std::process::Command::new("deputyctl")
            .args(["cost", "set", "--daily-cap", &format!("{cap}")])
            .output()
    })
    .await
    .ok();

    let _raised = matches!(result, Some(Ok(o)) if o.status.success());

    Redirect::to("/app/dashboard").into_response()
}

async fn post_reset_cost_trip() -> Response {
    let _ = tokio::task::spawn_blocking(|| {
        std::process::Command::new("deputyctl")
            .args(["cost", "reset"])
            .output()
    })
    .await;
    Redirect::to("/app/dashboard").into_response()
}
