//! Axum router + handlers.
//!
//! State lives in [`AppState`], shared via `Arc`. Each route handler reads
//! the current wizard state, optionally mutates it, persists, and renders a
//! response. Auth is a tiny middleware that gates everything except
//! `/healthz` and `/static/*`.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use axum::{
    extract::{Form, State},
    http::{header, HeaderMap, HeaderValue, StatusCode},
    middleware::{self, Next},
    response::{Html, IntoResponse, Redirect, Response},
    routing::{get, post},
    Json, Router,
};
use serde::{Deserialize, Serialize};

use deputyctl::apibase;
use deputyctl::limits::Limits;
use deputyctl::manifest::Manifest;
use deputyctl::model::{Provider, ProvidersFile};

use crate::apply::{self, ApplyMode};
use crate::auth::{
    extract_session_cookie, set_cookie_header, AuthOutcome, AuthState, COOKIE_NAME_DEV,
    COOKIE_NAME_PROD,
};
use crate::chat::{self, AgentReply, ChatTurn};
use crate::provider_check::{self, CheckResult};
use crate::runtime_bridge::RuntimeAgent;
use crate::state::{self, Step, WizardState};
use crate::templates::{self, ProfileChoice, ProviderChoice};

#[derive(Clone)]
pub struct AppState {
    pub auth: AuthState,
    pub state_file: PathBuf,
    pub state: Arc<Mutex<WizardState>>,
    pub providers: Arc<ProvidersFile>,
    pub profiles: Arc<Vec<(String, Manifest)>>,
    pub limits: Arc<Option<Limits>>,
    pub apply_mode: ApplyMode,
    /// In dev mode, root for the mirrored output tree. `None` means "use the
    /// default resolution" (`DEPUTYWIZARD_DEV_OUT` env var or `./dev-out`).
    pub dev_out: Option<PathBuf>,
    pub secure_cookies: bool,
    /// Pending provider key, set at step 3 and cleared by the apply step.
    /// Lives only in memory; never written to the wizard state file.
    pub pending_secret: Arc<Mutex<Option<PendingSecret>>>,
    /// Pending Tailscale auth key. Same lifetime as `pending_secret`.
    pub pending_tailscale: Arc<Mutex<Option<String>>>,
    /// Pending Cloudflare Tunnel credentials JSON (named tunnel only).
    pub pending_cloudflared: Arc<Mutex<Option<String>>>,
    /// Pending backup credentials. Lifetime as above.
    pub pending_backup: Arc<Mutex<Option<PendingBackup>>>,
    /// Pending device-code in the account sign-in flow (M8). Held in memory
    /// only — a device code is a capability that can be exchanged for tokens,
    /// so it must never be written to the wizard state file.
    pub pending_device_code: Arc<Mutex<Option<DeviceCodePending>>>,
    /// Optional override of the agent base URL used by `/chat`. Tests set
    /// this to a local mock; in production the app derives it from the
    /// active profile's `[health].http_check`.
    pub agent_base_override: Option<String>,
    /// Optional override of the chat history file path. Tests pin this to
    /// a tempdir so they can run in parallel without colliding on a shared
    /// env var. In production this is `None` and the path is derived from
    /// the active profile's `data_dir`.
    pub chat_history_override: Option<PathBuf>,
    /// Optional override that forces the wizard into airgap mode with a fixed
    /// set of local-LLM provider choices. Tests set this so they can exercise
    /// the airgap provider flow without a real `/etc/deputyos/airgap.flag` or
    /// catalog (and without racing on process-global env vars). In production
    /// this is `None` and airgap mode is derived from
    /// `deputyctl::model::airgap_active()` + the baked catalog (M4.5).
    pub airgap_providers: Option<Vec<ProviderChoice>>,
    /// Typed client for the resident lifecycle/resource agent. Production uses
    /// its root-owned Unix socket; tests inject a fake implementation.
    pub runtime_agent: Arc<dyn RuntimeAgent>,
}

#[derive(Debug, Clone)]
pub struct PendingSecret {
    pub provider_id: String,
    pub api_key: String,
}

/// Backup credentials held in memory between step 8 and apply.
#[derive(Debug, Clone)]
pub struct PendingBackup {
    /// "b2" | "r2" | "s3"
    pub kind: String,
    /// All credential fields (mix of secrets + non-secrets) in their
    /// rclone-config-friendly key form. Written verbatim to backup.env.
    pub fields: std::collections::BTreeMap<String, String>,
}

/// A device code issued by the account API, held in memory while the user
/// completes authorization on app.deputyos.com. `expires_at` is tracked
/// client-side (15 min) because the API's poll endpoint only ever returns
/// `authorization_pending` — it never signals expiry. A device code is a
/// capability (exchangeable for tokens), so it must never be persisted.
#[derive(Debug, Clone)]
pub struct DeviceCodePending {
    pub device_code: String,
    pub user_code: String,
    pub verification_uri: String,
    pub expires_at: Instant,
}

impl AppState {
    pub fn cookie_name(&self) -> &'static str {
        if self.secure_cookies {
            COOKIE_NAME_PROD
        } else {
            COOKIE_NAME_DEV
        }
    }
}

/// Top-level router.
pub fn router(app: AppState) -> Router {
    let auth_layer = middleware::from_fn_with_state(app.clone(), require_auth);

    let protected = Router::new()
        .route("/", get(get_index))
        .route("/wizard", get(get_index))
        .route("/wizard/system", get(get_system).post(post_system))
        .route("/wizard/profile", get(get_profile).post(post_profile))
        .route("/wizard/provider", get(get_provider).post(post_provider))
        .route("/wizard/channels", get(get_channels).post(post_channels))
        .route("/wizard/egress", get(get_egress).post(post_egress))
        .route("/wizard/ssh", get(get_ssh).post(post_ssh))
        .route("/wizard/tailscale", get(get_tailscale).post(post_tailscale))
        .route("/wizard/account", get(get_account).post(post_account))
        .route(
            "/wizard/cloudflare-tunnel",
            get(get_cloudflare_tunnel).post(post_cloudflare_tunnel),
        )
        .route("/wizard/backup", get(get_backup).post(post_backup))
        .route("/wizard/drives", get(get_drives).post(post_drives))
        .route("/wizard/review", get(get_review))
        .route("/wizard/review/apply", post(post_apply))
        .route("/wizard/done", get(get_done))
        .route("/chat", get(get_chat))
        .route("/chat/message", post(post_chat_message))
        .route("/mounts", get(get_mounts).post(post_mounts_add))
        .route("/mounts/remove", post(post_mounts_remove))
        .route("/mounts/network-add", post(post_mounts_network_add))
        .route("/api/v1/runtime", get(get_runtime_health))
        .route(
            "/api/v1/runtime/capabilities",
            get(get_runtime_capabilities),
        )
        .route("/api/v1/runtime/command", post(post_runtime_command))
        .route(
            "/api/v1/runtime/prepare-pause",
            post(post_runtime_prepare_pause),
        )
        .route("/api/v1/runtime/resume", post(post_runtime_resume))
        .route("/api/v1/runtime/reclaim", post(post_runtime_reclaim))
        .layer(auth_layer);

    let style_css = templates::STYLE_CSS;
    let public = Router::new().route("/healthz", get(healthz)).route(
        "/static/style.css",
        get(move || async move {
            (
                [(header::CONTENT_TYPE, "text/css; charset=utf-8")],
                style_css,
            )
        }),
    );

    public.merge(protected).with_state(app)
}

// ---------------------------------------------------------------------------
// Auth middleware
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct TokenQuery {
    #[serde(default)]
    token: Option<String>,
}

async fn require_auth(
    State(app): State<AppState>,
    headers: HeaderMap,
    req: axum::extract::Request,
    next: Next,
) -> Response {
    // Pull token from query string or Authorization: Bearer.
    let qs = req.uri().query().unwrap_or("");
    let q: TokenQuery = serde_urlencoded::from_str(qs).unwrap_or(TokenQuery { token: None });
    let token = q.token.or_else(|| {
        headers
            .get(header::AUTHORIZATION)
            .and_then(|h| h.to_str().ok())
            .and_then(|h| h.strip_prefix("Bearer ").map(String::from))
    });

    let cookie_header = headers
        .get(header::COOKIE)
        .and_then(|h| h.to_str().ok())
        .map(String::from);
    let cookie_value = extract_session_cookie(cookie_header.as_deref(), app.cookie_name());

    let outcome = app
        .auth
        .check_or_issue(token.as_deref(), cookie_value.as_deref());
    match outcome {
        AuthOutcome::Authorized { issue } => {
            let mut response = next.run(req).await;
            if let Some(session) = issue {
                let cookie = set_cookie_header(app.cookie_name(), &session, app.secure_cookies);
                if let Ok(v) = HeaderValue::from_str(&cookie) {
                    response.headers_mut().append(header::SET_COOKIE, v);
                }
            }
            response
        }
        AuthOutcome::Unauthorized => (
            StatusCode::UNAUTHORIZED,
            [(header::CONTENT_TYPE, "text/html; charset=utf-8")],
            templates::page_unauthorized(),
        )
            .into_response(),
    }
}

// ---------------------------------------------------------------------------
// Healthz / index
// ---------------------------------------------------------------------------

async fn healthz() -> &'static str {
    "ok"
}

// ---------------------------------------------------------------------------
// Resident runtime agent — authenticated tunnel bridge
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct ReclaimRequest {
    #[serde(default)]
    drop_caches: bool,
}

async fn get_runtime_health(State(app): State<AppState>) -> Response {
    runtime_command(app, deputyd::AgentCommand::Health).await
}

async fn get_runtime_capabilities(State(app): State<AppState>) -> Response {
    runtime_command(app, deputyd::AgentCommand::Capabilities).await
}

async fn post_runtime_command(
    State(app): State<AppState>,
    Json(command): Json<deputyd::AgentCommand>,
) -> Response {
    runtime_command(app, command).await
}

async fn post_runtime_prepare_pause(State(app): State<AppState>) -> Response {
    runtime_command(app, deputyd::AgentCommand::PreparePause).await
}

async fn post_runtime_resume(State(app): State<AppState>) -> Response {
    runtime_command(app, deputyd::AgentCommand::Resume).await
}

async fn post_runtime_reclaim(
    State(app): State<AppState>,
    Json(body): Json<ReclaimRequest>,
) -> Response {
    runtime_command(
        app,
        deputyd::AgentCommand::Reclaim {
            drop_caches: body.drop_caches,
        },
    )
    .await
}

async fn runtime_command(app: AppState, command: deputyd::AgentCommand) -> Response {
    // The Unix-socket round trip is blocking but short. Keep it off Axum's
    // async worker so a slow guest operation cannot stall unrelated tunnel
    // traffic.
    let agent = app.runtime_agent.clone();
    match tokio::task::spawn_blocking(move || agent.execute(command)).await {
        Ok(Ok(response)) => {
            let status = if response.ok {
                StatusCode::OK
            } else {
                StatusCode::CONFLICT
            };
            (status, Json(response)).into_response()
        }
        Ok(Err(error)) => {
            let response = deputyd::AgentResponse {
                protocol: deputyd::PROTOCOL_VERSION,
                id: "wizard-transport".to_string(),
                ok: false,
                result: None,
                error: Some(format!("resident agent unavailable: {error:#}")),
            };
            (StatusCode::BAD_GATEWAY, Json(response)).into_response()
        }
        Err(error) => {
            let response = deputyd::AgentResponse {
                protocol: deputyd::PROTOCOL_VERSION,
                id: "wizard-join".to_string(),
                ok: false,
                result: None,
                error: Some(format!("resident agent task failed: {error}")),
            };
            (StatusCode::INTERNAL_SERVER_ERROR, Json(response)).into_response()
        }
    }
}

async fn get_index(State(app): State<AppState>) -> Response {
    let step = app.state.lock().expect("state lock").step;
    Redirect::to(&step_url(step)).into_response()
}

fn step_url(s: Step) -> String {
    match s {
        Step::Done => "/wizard/done".into(),
        _ => format!("/wizard/{}", s.slug()),
    }
}

// ---------------------------------------------------------------------------
// Step 1: system (hostname + timezone)
// ---------------------------------------------------------------------------

async fn get_system(State(app): State<AppState>) -> Response {
    let s = app.state.lock().expect("state lock").clone();
    let html = templates::step_system(&s, app.limits.as_ref().as_ref(), None);
    Html(html).into_response()
}

#[derive(Debug, Deserialize)]
struct SystemForm {
    hostname: String,
    timezone: String,
}

async fn post_system(State(app): State<AppState>, Form(f): Form<SystemForm>) -> Response {
    let hostname = f.hostname.trim().to_string();
    let timezone = f.timezone.trim().to_string();
    let err = validate_hostname(&hostname).err().or_else(|| {
        if timezone.is_empty() || timezone.contains(char::is_whitespace) {
            Some("Timezone must be a non-empty IANA name (e.g. UTC, America/Los_Angeles).".into())
        } else {
            None
        }
    });
    if let Some(e) = err {
        return render_step_with_error(&app, Step::System, &e);
    }
    {
        let mut g = app.state.lock().expect("state lock");
        g.answers.hostname = Some(hostname);
        g.answers.timezone = Some(timezone);
        g.step = Step::Profile;
        let _ = state::save(&app.state_file, &g);
    }
    Redirect::to(&step_url(Step::Profile)).into_response()
}

fn validate_hostname(h: &str) -> Result<(), String> {
    if h.is_empty() {
        return Err("Hostname is required.".into());
    }
    if h.len() > 63 {
        return Err("Hostname must be 63 characters or fewer.".into());
    }
    if !h
        .chars()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
    {
        return Err("Hostname may only contain lowercase letters, digits, and hyphens.".into());
    }
    if h.starts_with('-') || h.ends_with('-') {
        return Err("Hostname must not start or end with a hyphen.".into());
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Step 2: profile
// ---------------------------------------------------------------------------

async fn get_profile(State(app): State<AppState>) -> Response {
    let s = app.state.lock().expect("state lock").clone();
    let choices = profile_choices(&app);
    let html = templates::step_profile(&s, app.limits.as_ref().as_ref(), &choices, None);
    Html(html).into_response()
}

fn profile_choices(app: &AppState) -> Vec<ProfileChoice> {
    app.profiles
        .iter()
        .map(|(_, m)| ProfileChoice {
            id: m.profile.id.clone(),
            display_name: m.profile.display_name.clone(),
            pinned_version: m.profile.pinned_version.clone(),
            min_ram_mb: m.profile.min_ram_mb,
        })
        .collect()
}

#[derive(Debug, Deserialize)]
struct ProfileForm {
    profile: String,
}

async fn post_profile(State(app): State<AppState>, Form(f): Form<ProfileForm>) -> Response {
    let id = f.profile.trim().to_string();
    if !app.profiles.iter().any(|(_, m)| m.profile.id == id) {
        return render_step_with_error(&app, Step::Profile, "Unknown profile id.");
    }
    {
        let mut g = app.state.lock().expect("state lock");
        g.answers.profile = Some(id);
        g.step = Step::Provider;
        let _ = state::save(&app.state_file, &g);
    }
    Redirect::to(&step_url(Step::Provider)).into_response()
}

// ---------------------------------------------------------------------------
// Step 3: provider
// ---------------------------------------------------------------------------

async fn get_provider(State(app): State<AppState>) -> Response {
    let s = app.state.lock().expect("state lock").clone();
    let (choices, airgap) = provider_choices(&app);
    let html = templates::step_provider(&s, app.limits.as_ref().as_ref(), &choices, None, airgap);
    Html(html).into_response()
}

/// Build the provider choices for the current mode + whether the wizard is in
/// airgap (local-LLM-only) mode. The broken pre-M4.5 path filtered the static
/// `providers.json` by `kind == "local-llamacpp" || "local-ollama"`, but no
/// shipped provider has those kinds (local-ollama is `openai-compatible`), so
/// airgap showed zero providers. The fix builds airgap choices from the baked
/// catalog via `deputyctl::model::load_airgap_choices` and resolves the active
/// profile's `[airgap] default_provider` alias so the right model is
/// pre-selected.
///
/// Returns `(choices, airgap)`. On a degenerate airgap build with an empty
/// catalog we fall back to cloud choices so the wizard is still usable.
fn provider_choices(app: &AppState) -> (Vec<ProviderChoice>, bool) {
    // Test override: pin both the mode and the choices without touching env.
    if let Some(ovr) = &app.airgap_providers {
        return (ovr.clone(), true);
    }

    if !deputyctl::model::airgap_active() {
        return (cloud_provider_choices(app), false);
    }

    let models = deputyctl::model::load_airgap_choices().unwrap_or_default();
    if models.is_empty() {
        return (cloud_provider_choices(app), false);
    }

    // Resolve the profile's [airgap] default_provider alias to a concrete
    // choice id so the wizard pre-selects it. `local-llamacpp-airgap` means
    // "the catalog's default-flagged model"; an explicit `airgap-<id>` pins
    // that exact model as the pre-selection.
    let alias = app
        .state
        .lock()
        .expect("state lock")
        .answers
        .profile
        .as_deref()
        .and_then(|id| app.profiles.iter().find(|(_, m)| m.profile.id == id))
        .and_then(|(_, m)| m.airgap.as_ref())
        .map(|a| a.default_provider.clone());
    let pinned_default = match alias.as_deref() {
        Some(a) if a.starts_with("airgap-") => Some(a.to_string()),
        _ => None, // local-llamacpp-airgap → use the catalog default flag
    };

    let choices: Vec<ProviderChoice> = models
        .iter()
        .map(|m| ProviderChoice {
            id: m.id.clone(),
            display_name: m.display_name.clone(),
            key_env_var: String::new(),
            key_format: String::new(),
            default: m.default || pinned_default.as_deref() == Some(&m.id),
        })
        .collect();
    (choices, true)
}

/// Cloud (providers.json) choices. `openrouter` is the pre-selected default.
fn cloud_provider_choices(app: &AppState) -> Vec<ProviderChoice> {
    app.providers
        .providers
        .iter()
        .map(|p| ProviderChoice {
            id: p.id.clone(),
            display_name: p.display_name.clone(),
            key_env_var: p.key_env_var.clone(),
            key_format: p.key_format.clone(),
            default: p.id == "openrouter",
        })
        .collect()
}

#[derive(Debug, Deserialize)]
struct ProviderForm {
    provider: String,
    /// Empty for airgap local-LLM providers (no key). Defaults to empty so the
    /// airgap form — which sends only the provider id — still deserialises.
    #[serde(default)]
    api_key: String,
    #[serde(default)]
    skip_validation: Option<String>,
}

async fn post_provider(State(app): State<AppState>, Form(f): Form<ProviderForm>) -> Response {
    let provider_id = f.provider.trim().to_string();
    let key = f.api_key;
    let skip_validation = f.skip_validation.is_some();

    // Airgap local-LLM providers need no API key and no network round-trip —
    // the model is baked into the image. Resolve them from the same choice set
    // the GET used (override or catalog) so a spoofed `provider=airgap-...`
    // can't bypass the key check on a non-airgap build.
    let (choices, airgap) = provider_choices(&app);
    if airgap && choices.iter().any(|c| c.id == provider_id) {
        {
            let mut g = app.state.lock().expect("state lock");
            g.answers.provider = Some(provider_id.clone());
            g.step = Step::Channels;
            let _ = state::save(&app.state_file, &g);
        }
        // No pending_secret: there is no key to persist.
        *app.pending_secret.lock().expect("pending lock") = None;
        return Redirect::to(&step_url(Step::Channels)).into_response();
    }

    let provider = app
        .providers
        .providers
        .iter()
        .find(|p| p.id == provider_id)
        .cloned();
    let provider = match provider {
        Some(p) => p,
        None => return render_step_with_error(&app, Step::Provider, "Unknown provider id."),
    };
    if key.trim().is_empty() {
        return render_step_with_error(&app, Step::Provider, "API key is required.");
    }

    // Round-trip validation. The check itself blocks on a network call;
    // run it on a tokio blocking thread so the runtime stays responsive.
    if !skip_validation {
        let provider_for_check = provider.clone();
        let key_for_check = key.clone();
        let result = tokio::task::spawn_blocking(move || {
            provider_check::check(&provider_for_check, &key_for_check)
        })
        .await
        .unwrap_or(CheckResult::Network {
            message: "validation task panicked".into(),
        });
        if !result.is_ok() {
            let msg = match result {
                CheckResult::HttpError { status, hint } => {
                    format!("Provider returned HTTP {status}. {hint} (or tick \"Skip validation\")")
                }
                CheckResult::Network { message } => format!(
                    "Could not reach provider: {message}. \
                     Check your network, or tick \"Skip validation\" to bypass."
                ),
                _ => unreachable!("is_ok handled Ok and Skipped above"),
            };
            return render_step_with_error(&app, Step::Provider, &msg);
        }
    }

    {
        let mut g = app.state.lock().expect("state lock");
        g.answers.provider = Some(provider_id.clone());
        g.step = Step::Channels;
        let _ = state::save(&app.state_file, &g);
    }
    *app.pending_secret.lock().expect("pending lock") = Some(PendingSecret {
        provider_id,
        api_key: key,
    });
    Redirect::to(&step_url(Step::Channels)).into_response()
}

// ---------------------------------------------------------------------------
// Step 4: channels
// ---------------------------------------------------------------------------

async fn get_channels(State(app): State<AppState>) -> Response {
    let s = app.state.lock().expect("state lock").clone();
    let (supported, disabled) = channel_lists(&app, &s);
    let html = templates::step_channels(
        &s,
        app.limits.as_ref().as_ref(),
        &supported,
        &disabled,
        None,
    );
    Html(html).into_response()
}

fn channel_lists(app: &AppState, s: &WizardState) -> (Vec<String>, Vec<String>) {
    let supported = s
        .answers
        .profile
        .as_deref()
        .and_then(|id| {
            app.profiles
                .iter()
                .find(|(_, m)| m.profile.id == id)
                .and_then(|(_, m)| m.channels.as_ref().map(|c| c.supported.clone()))
        })
        .unwrap_or_default();
    let disabled = app
        .limits
        .as_ref()
        .as_ref()
        .map(|l| l.capabilities.channels_disabled_by_ram.clone())
        .unwrap_or_default();
    (supported, disabled)
}

async fn post_channels(State(app): State<AppState>, body: String) -> Response {
    // Manual parse: `Form` would only give us the last `channels` value.
    let pairs: Vec<(String, String)> = serde_urlencoded::from_str(&body).unwrap_or_default();
    let chosen: Vec<String> = pairs
        .into_iter()
        .filter(|(k, _)| k == "channels")
        .map(|(_, v)| v)
        .collect();

    let s = app.state.lock().expect("state lock").clone();
    let (supported, disabled) = channel_lists(&app, &s);
    let chosen_set: std::collections::BTreeSet<&String> = chosen.iter().collect();

    // Reject channels not in the profile's supported list.
    if let Some(unknown) = chosen.iter().find(|c| !supported.contains(c)) {
        return render_step_with_error(
            &app,
            Step::Channels,
            &format!("Channel '{unknown}' is not supported by this profile."),
        );
    }

    // Reject channels disabled by limits.
    let blocked: Vec<&String> = disabled.iter().filter(|d| chosen_set.contains(d)).collect();
    if !blocked.is_empty() {
        let names = blocked
            .iter()
            .map(|s| s.as_str())
            .collect::<Vec<_>>()
            .join(", ");
        let msg = format!(
            "{names} disabled by RAM tier on this device. Upgrade to a higher-RAM target to enable.",
        );
        return (
            StatusCode::UNPROCESSABLE_ENTITY,
            Html(templates::step_channels(
                &s,
                app.limits.as_ref().as_ref(),
                &supported,
                &disabled,
                Some(&msg),
            )),
        )
            .into_response();
    }

    {
        let mut g = app.state.lock().expect("state lock");
        g.answers.channels = chosen;
        g.step = Step::Egress;
        let _ = state::save(&app.state_file, &g);
    }
    Redirect::to(&step_url(Step::Egress)).into_response()
}

// ---------------------------------------------------------------------------
// Step 5: egress (M5.5)
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct EgressForm {
    egress_mode: String,
}

async fn get_egress(State(app): State<AppState>) -> Response {
    let s = app.state.lock().expect("state lock").clone();
    let (mode, hosts) = profile_egress_hints(&app, &s);
    let html = templates::step_egress(&s, app.limits.as_ref().as_ref(), &mode, &hosts);
    Html(html).into_response()
}

async fn post_egress(State(app): State<AppState>, Form(f): Form<EgressForm>) -> Response {
    let mode = f.egress_mode.trim().to_string();
    if !matches!(mode.as_str(), "open" | "whitelist" | "airgap") {
        return render_step_with_error(&app, Step::Egress, "Choose open, whitelist, or airgap.");
    }
    {
        let mut g = app.state.lock().expect("state lock");
        g.answers.egress_mode = Some(mode);
        g.step = Step::Ssh;
        let _ = state::save(&app.state_file, &g);
    }
    Redirect::to(&step_url(Step::Ssh)).into_response()
}

// ---------------------------------------------------------------------------
// Step 6: ssh
// ---------------------------------------------------------------------------

async fn get_ssh(State(app): State<AppState>) -> Response {
    let s = app.state.lock().expect("state lock").clone();
    let html = templates::step_ssh(&s, app.limits.as_ref().as_ref(), None);
    Html(html).into_response()
}

#[derive(Debug, Deserialize)]
struct SshForm {
    ssh_keys: String,
}

async fn post_ssh(State(app): State<AppState>, Form(f): Form<SshForm>) -> Response {
    let keys: Vec<String> = f
        .ssh_keys
        .lines()
        .map(|l| l.trim().to_string())
        .filter(|l| !l.is_empty())
        .collect();
    for k in &keys {
        if let Err(e) = validate_ssh_key(k) {
            return render_step_with_error(&app, Step::Ssh, &e);
        }
    }
    if keys.is_empty() {
        return render_step_with_error(&app, Step::Ssh, "At least one SSH key is required.");
    }
    {
        let mut g = app.state.lock().expect("state lock");
        g.answers.ssh_keys = keys;
        g.step = Step::Tailscale;
        let _ = state::save(&app.state_file, &g);
    }
    Redirect::to(&step_url(Step::Tailscale)).into_response()
}

fn validate_ssh_key(k: &str) -> Result<(), String> {
    let prefixes = [
        "ssh-rsa ",
        "ssh-ed25519 ",
        "ssh-ecdsa ",
        "ecdsa-sha2-nistp256 ",
        "ecdsa-sha2-nistp384 ",
        "ecdsa-sha2-nistp521 ",
        "sk-ssh-ed25519@openssh.com ",
        "sk-ecdsa-sha2-nistp256@openssh.com ",
    ];
    if !prefixes.iter().any(|p| k.starts_with(p)) {
        return Err(format!(
            "SSH key must start with a known algorithm prefix (got: {})",
            k.split_whitespace().next().unwrap_or("?")
        ));
    }
    if k.split_whitespace().count() < 2 {
        return Err("SSH key must include a key body after the algorithm prefix.".into());
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Step 6: tailscale
// ---------------------------------------------------------------------------

async fn get_tailscale(State(app): State<AppState>) -> Response {
    let s = app.state.lock().expect("state lock").clone();
    let html = templates::step_tailscale(&s, app.limits.as_ref().as_ref(), None);
    Html(html).into_response()
}

#[derive(Debug, Deserialize)]
struct TailscaleForm {
    #[serde(default)]
    authkey: Option<String>,
    #[serde(default)]
    skip: Option<String>,
}

async fn post_tailscale(State(app): State<AppState>, Form(f): Form<TailscaleForm>) -> Response {
    let key = f.authkey.unwrap_or_default().trim().to_string();
    let enable = f.skip.is_none() && !key.is_empty();
    if enable {
        // Trivial format hint — Tailscale auth keys are `tskey-...` or
        // `tskey-auth-...`. We don't gate on it (formats change), but reject
        // obviously bogus inputs.
        if key.len() < 8 {
            return render_step_with_error(
                &app,
                Step::Tailscale,
                "Auth key looks too short. Generate one at tailscale.com/admin/settings/keys.",
            );
        }
        *app.pending_tailscale.lock().expect("pending lock") = Some(key);
    } else {
        *app.pending_tailscale.lock().expect("pending lock") = None;
    }
    {
        let mut g = app.state.lock().expect("state lock");
        g.answers.tailscale_enabled = enable;
        g.step = Step::Account;
        let _ = state::save(&app.state_file, &g);
    }
    Redirect::to(&step_url(Step::Account)).into_response()
}

// ---------------------------------------------------------------------------
// Step 7: account (M8) — register this device against an deputyOS account
// ---------------------------------------------------------------------------

/// The account step is optional (M8 hard rule: every flow works without an
/// account). "Skip" advances with `account_registered=false` and no tokens;
/// the Cloudflare Tunnel step then falls back to the cloudflared quick/named
/// paths as before. When the user signs in, the wizard mints a device code,
/// the user authorizes it on app.deputyos.com/device, and the wizard
/// exchanges it for an account access token, registers this device, and
/// writes the tunnel/backup tokens to /etc/deputyos/{tunnel,backup}-token
/// (0600). Tokens never touch the wizard state file — only the non-secret
/// `account_email` / `account_registered` flags do.
async fn get_account(State(app): State<AppState>) -> Response {
    let s = app.state.lock().expect("state lock").clone();
    let pending = app
        .pending_device_code
        .lock()
        .expect("pending lock")
        .clone();
    let view = templates::AccountView {
        registered: s.answers.account_registered,
        user_code: pending.as_ref().map(|d| d.user_code.as_str()),
        verification_uri: pending.as_ref().map(|d| d.verification_uri.as_str()),
        email: s.answers.account_email.as_deref(),
        api_base: s.answers.account_api_base.as_deref(),
        note: None,
        note_is_error: false,
    };
    let html = templates::step_account(&s, app.limits.as_ref().as_ref(), &view);
    Html(html).into_response()
}

#[derive(Debug, Deserialize)]
struct AccountForm {
    /// "begin" | "poll" | "skip" | "continue" | "cancel".
    #[serde(default)]
    action: String,
    /// Optional self-reported account email (non-secret local label).
    #[serde(default)]
    email: Option<String>,
    /// Optional custom/self-hosted backend API base URL. Empty/None means use
    /// the production backend (or `DEPUTYOS_API_BASE` if set for local E2E).
    #[serde(default)]
    api_base: Option<String>,
}

/// Trim + canonicalize a user-entered api base. Returns None for blank, and
/// strips a trailing slash to match [`deputyctl::apibase::resolve`].
fn normalize_api_base(raw: &str) -> Option<String> {
    let s = raw.trim();
    if s.is_empty() {
        return None;
    }
    Some(s.trim_end_matches('/').to_string())
}

/// Persist a (non-default) custom backend choice into the wizard answers +
/// state file. `None` clears it. Called from the Account step handler so the
/// begin→poll cycle uses a consistent backend.
fn remember_api_base(app: &AppState, base: Option<String>) {
    let mut g = app.state.lock().expect("state lock");
    g.answers.account_api_base = base;
    let _ = state::save(&app.state_file, &g);
}

async fn post_account(State(app): State<AppState>, Form(f): Form<AccountForm>) -> Response {
    let action = f.action.trim().to_string();
    let email = f
        .email
        .as_deref()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());
    // A custom backend entered with any action is remembered immediately so
    // the begin→poll cycle (and re-renders) stays consistent. A blank
    // submission leaves a prior choice intact (skip/back doesn't wipe it);
    // the only way to revert to the default is to re-register, which the
    // "Use a different account" path handles by re-showing the field.
    let submitted_base = f.api_base.as_deref().and_then(normalize_api_base);
    if let Some(b) = submitted_base {
        remember_api_base(&app, Some(b));
    }
    let api_base = account_api_base(&app);

    match action.as_str() {
        "skip" | "continue" => {
            *app.pending_device_code.lock().expect("pending lock") = None;
            {
                let mut g = app.state.lock().expect("state lock");
                g.step = Step::CloudflareTunnel;
                let _ = state::save(&app.state_file, &g);
            }
            Redirect::to(&step_url(Step::CloudflareTunnel)).into_response()
        }
        "cancel" => {
            *app.pending_device_code.lock().expect("pending lock") = None;
            render_account_view(&app, false, None, None, email.as_deref(), None, false)
        }
        "begin" => {
            *app.pending_device_code.lock().expect("pending lock") = None;
            let client_name = current_hostname(&app);
            let api_base2 = api_base.clone();
            let outcome =
                tokio::task::spawn_blocking(move || request_device_code(&api_base2, &client_name))
                    .await;
            match outcome {
                Ok(Ok(dc)) => {
                    let view_code = Some(dc.user_code.clone());
                    let view_uri = Some(dc.verification_uri.clone());
                    *app.pending_device_code.lock().expect("pending lock") = Some(dc);
                    render_account_view(
                        &app,
                        false,
                        view_code,
                        view_uri,
                        email.as_deref(),
                        None,
                        false,
                    )
                }
                Ok(Err(e)) => {
                    render_account_view(&app, false, None, None, email.as_deref(), Some(e), true)
                }
                Err(e) => render_account_view(
                    &app,
                    false,
                    None,
                    None,
                    email.as_deref(),
                    Some(format!("device-code task failed: {e}")),
                    true,
                ),
            }
        }
        "poll" => {
            let pending = app
                .pending_device_code
                .lock()
                .expect("pending lock")
                .clone();
            let Some(dc) = pending else {
                return render_account_view(
                    &app,
                    false,
                    None,
                    None,
                    email.as_deref(),
                    Some("no device code in flight — click Sign in to begin.".into()),
                    true,
                );
            };
            if Instant::now() > dc.expires_at {
                *app.pending_device_code.lock().expect("pending lock") = None;
                return render_account_view(
                    &app,
                    false,
                    None,
                    None,
                    email.as_deref(),
                    Some("device code expired — click Sign in to begin again.".into()),
                    true,
                );
            }
            let device_name = current_hostname(&app);
            let root = apply::root(app.apply_mode, app.dev_out.as_deref());
            let device_code = dc.device_code.clone();
            let api_base2 = api_base.clone();
            let email2 = email.clone();
            let start_tunnel = app.apply_mode == ApplyMode::Production;
            let res = tokio::task::spawn_blocking(move || {
                poll_and_register(
                    &api_base2,
                    &device_code,
                    &device_name,
                    &root,
                    email2.as_deref(),
                    start_tunnel,
                )
            })
            .await;
            match res {
                Ok(PollResult::Registered) => {
                    *app.pending_device_code.lock().expect("pending lock") = None;
                    {
                        let mut g = app.state.lock().expect("state lock");
                        g.answers.account_registered = true;
                        g.answers.account_email = email.clone().or(g.answers.account_email.take());
                        g.step = Step::CloudflareTunnel;
                        let _ = state::save(&app.state_file, &g);
                    }
                    Redirect::to(&step_url(Step::CloudflareTunnel)).into_response()
                }
                Ok(PollResult::Pending) => render_account_view(
                    &app,
                    false,
                    Some(dc.user_code.clone()),
                    Some(dc.verification_uri.clone()),
                    email.as_deref(),
                    Some(
                        "Still waiting — open the link, enter the code, sign in, then click Continue."
                            .into(),
                    ),
                    false,
                ),
                Ok(PollResult::Other(msg)) => render_account_view(
                    &app,
                    false,
                    Some(dc.user_code.clone()),
                    Some(dc.verification_uri.clone()),
                    email.as_deref(),
                    Some(msg),
                    true,
                ),
                Err(e) => render_account_view(
                    &app,
                    false,
                    None,
                    None,
                    email.as_deref(),
                    Some(format!("poll task failed: {e}")),
                    true,
                ),
            }
        }
        _ => render_account_view(
            &app,
            false,
            None,
            None,
            email.as_deref(),
            Some("unknown action.".into()),
            true,
        ),
    }
}

/// Build the Account step HTML from owned view fields (avoids borrowing across
/// the AppState mutex boundary).
#[allow(clippy::too_many_arguments)]
fn render_account_view(
    app: &AppState,
    registered: bool,
    user_code: Option<String>,
    verification_uri: Option<String>,
    email: Option<&str>,
    note: Option<String>,
    note_is_error: bool,
) -> Response {
    let s = app.state.lock().expect("state lock").clone();
    let view = templates::AccountView {
        registered,
        user_code: user_code.as_deref(),
        verification_uri: verification_uri.as_deref(),
        email,
        // The custom backend the user entered this session (None = default).
        // Pulled from the same persisted answers the GET handler reads, so the
        // field stays populated across the begin→poll re-renders.
        api_base: s.answers.account_api_base.as_deref(),
        note: note.as_deref(),
        note_is_error,
    };
    let html = templates::step_account(&s, app.limits.as_ref().as_ref(), &view);
    Html(html).into_response()
}

/// Base URL for the account/auth API. Precedence: a custom backend the user
/// entered at this step (persisted in `answers.account_api_base`) > the
/// `DEPUTYOS_API_BASE` env override (local E2E) > the production default. Mirrors
/// [`deputyctl::apibase::resolve`] (the wizard doesn't read the `/etc/deputyos/
/// api-base` file itself — its own answers persist the choice across boots).
fn account_api_base(app: &AppState) -> String {
    let s = app.state.lock().expect("state lock");
    if let Some(b) = s.answers.account_api_base.as_deref() {
        let b = b.trim();
        if !b.is_empty() {
            return b.trim_end_matches('/').to_string();
        }
    }
    drop(s);
    std::env::var("DEPUTYOS_API_BASE")
        .ok()
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| apibase::DEFAULT_API_BASE.to_string())
        .trim_end_matches('/')
        .to_string()
}

/// Device name to register under: the wizard's chosen hostname, else the OS
/// hostname, else a generic fallback.
fn current_hostname(app: &AppState) -> String {
    let chosen = app
        .state
        .lock()
        .expect("state lock")
        .answers
        .hostname
        .clone()
        .filter(|h| !h.is_empty());
    if let Some(h) = chosen {
        return h;
    }
    std::fs::read_to_string("/etc/hostname")
        .map(|s| s.trim().to_string())
        .ok()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "deputyos-device".into())
}

// ---- device-code request/response shapes (mirror the API contract) ----

#[derive(Serialize)]
struct DeviceTokenReq<'a> {
    device_code: &'a str,
}

#[derive(Serialize)]
struct RegisterReq<'a> {
    device_name: &'a str,
}

#[derive(Serialize)]
struct AccountFile<'a> {
    registered: bool,
    device_id: &'a str,
    device_name: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    email: Option<&'a str>,
    /// The owner account id (the JWT `sub`). Written so the wizard's
    /// AccountOwner auth mode (remote management via tunnel) can match a
    /// presented JWT against this device's owner without re-decoding at launch.
    #[serde(skip_serializing_if = "Option::is_none")]
    account_id: Option<&'a str>,
}

#[derive(Deserialize)]
struct DeviceCodeResp {
    device_code: String,
    user_code: String,
    verification_uri: String,
}

#[derive(Deserialize)]
struct TokenResp {
    access_token: String,
}

#[derive(Deserialize)]
struct RegisterResp {
    device_id: String,
    tunnel_token: String,
    backup_token: String,
}

/// Outcome of a poll-and-register attempt.
enum PollResult {
    /// Authorized + registered; tokens written to disk.
    Registered,
    /// User hasn't completed authorization yet.
    Pending,
    /// Transport / unexpected error. The pending code is kept so the user can
    /// retry without re-initiating.
    Other(String),
}

/// Decode the `sub` claim from a JWT's payload, without signature verification
/// (display/keying only — the signature was verified server-side at issue).
/// Returns `None` on any malformation. Mirrors `deputyos-console::api_client`
/// and the sibling API's `jwt_subject`.
fn jwt_subject(token: &str) -> Option<String> {
    use base64::engine::general_purpose::URL_SAFE_NO_PAD;
    use base64::Engine;
    let mut parts = token.split('.');
    let _header = parts.next()?;
    let payload = parts.next()?;
    let bytes = URL_SAFE_NO_PAD.decode(payload).ok()?;
    let value: serde_json::Value = serde_json::from_slice(&bytes).ok()?;
    value.get("sub")?.as_str().map(|s| s.to_string())
}

/// Request a device code from the API. Runs in `spawn_blocking`.
fn request_device_code(api_base: &str, client_name: &str) -> Result<DeviceCodePending, String> {
    let url = format!("{api_base}/api/v1/auth/device-code");
    let resp = ureq::post(&url).query("client_name", client_name).call();
    let txt = match resp {
        Ok(r) => r.into_string().unwrap_or_default(),
        Err(ureq::Error::Status(c, r)) => {
            return Err(format!(
                "device-code {c}: {}",
                r.into_string().unwrap_or_default()
            ))
        }
        Err(ureq::Error::Transport(t)) => return Err(format!("device-code transport: {t}")),
    };
    let parsed: DeviceCodeResp =
        serde_json::from_str(&txt).map_err(|e| format!("device-code bad response: {e}"))?;
    Ok(DeviceCodePending {
        device_code: parsed.device_code,
        user_code: parsed.user_code,
        verification_uri: parsed.verification_uri,
        expires_at: Instant::now() + Duration::from_secs(15 * 60),
    })
}

/// Poll the device-token endpoint and, on success, register the device and
/// write the tunnel/backup tokens (0600) + an account.json label. The whole
/// sequence runs in one `spawn_blocking` so the access token never crosses an
/// await boundary or leaves memory.
fn poll_and_register(
    api_base: &str,
    device_code: &str,
    device_name: &str,
    root: &Path,
    email: Option<&str>,
    start_tunnel: bool,
) -> PollResult {
    // 1. Exchange the device code for an account access token.
    let token_url = format!("{api_base}/api/v1/auth/device-token");
    let body = match serde_json::to_string(&DeviceTokenReq { device_code }) {
        Ok(b) => b,
        Err(e) => return PollResult::Other(format!("encoding device-token request: {e}")),
    };
    let resp = ureq::post(&token_url)
        .set("Content-Type", "application/json")
        .send_string(&body);
    let access_token = match resp {
        Ok(r) => {
            let txt = r.into_string().unwrap_or_default();
            match serde_json::from_str::<TokenResp>(&txt) {
                Ok(t) => t.access_token,
                Err(e) => return PollResult::Other(format!("device-token bad response: {e}")),
            }
        }
        Err(ureq::Error::Status(400, r)) => {
            let b = r.into_string().unwrap_or_default();
            if b.contains("authorization_pending") {
                return PollResult::Pending;
            }
            return PollResult::Other(format!("device-token 400: {b}"));
        }
        Err(ureq::Error::Status(c, r)) => {
            return PollResult::Other(format!(
                "device-token {c}: {}",
                r.into_string().unwrap_or_default()
            ))
        }
        Err(ureq::Error::Transport(t)) => {
            return PollResult::Other(format!("device-token transport: {t}"))
        }
    };

    // 2. Register this device under the account → tunnel/backup tokens.
    let reg_url = format!("{api_base}/api/v1/accounts/devices/register");
    let body = match serde_json::to_string(&RegisterReq { device_name }) {
        Ok(b) => b,
        Err(e) => return PollResult::Other(format!("encoding register request: {e}")),
    };
    let resp = ureq::post(&reg_url)
        .set("Authorization", &format!("Bearer {access_token}"))
        .set("Content-Type", "application/json")
        .send_string(&body);
    let reg: RegisterResp = match resp {
        Ok(r) => {
            let txt = r.into_string().unwrap_or_default();
            match serde_json::from_str(&txt) {
                Ok(v) => v,
                Err(e) => return PollResult::Other(format!("register bad response: {e}")),
            }
        }
        Err(ureq::Error::Status(c, r)) => {
            return PollResult::Other(format!(
                "register {c}: {}",
                r.into_string().unwrap_or_default()
            ))
        }
        Err(ureq::Error::Transport(t)) => {
            return PollResult::Other(format!("register transport: {t}"))
        }
    };

    // 3. Write tokens (0600) + a non-secret account.json label for the PWA.
    let tunnel_path = root.join("etc/deputyos/tunnel-token");
    let backup_path = root.join("etc/deputyos/backup-token");
    if let Err(e) = apply::write_atomic(&tunnel_path, &format!("{}\n", reg.tunnel_token), 0o600) {
        return PollResult::Other(format!("writing tunnel-token: {e}"));
    }
    if let Err(e) = apply::write_atomic(&backup_path, &format!("{}\n", reg.backup_token), 0o600) {
        return PollResult::Other(format!("writing backup-token: {e}"));
    }
    // Decode the account id from the access JWT's `sub` claim (non-trusting —
    // the signature was verified server-side when issuing the token) so the
    // wizard's AccountOwner auth mode can later match a presented JWT to this
    // device's owner. Failure → `None` (remote management stays disabled).
    let account_id = jwt_subject(&access_token);
    let account_json = match serde_json::to_string(&AccountFile {
        registered: true,
        device_id: &reg.device_id,
        device_name,
        email,
        account_id: account_id.as_deref(),
    }) {
        Ok(s) => s,
        Err(e) => return PollResult::Other(format!("encoding account.json: {e}")),
    };
    if let Err(e) = apply::write_atomic(
        &root.join("etc/deputyos/account.json"),
        &account_json,
        0o600,
    ) {
        return PollResult::Other(format!("writing account.json: {e}"));
    }

    // 4. Persist a custom backend choice to /etc/deputyos/api-base (0644 — a
    // public hostname, non-secret) so the integrated tunnel + command poller
    // reach the same self-hosted backend this device registered against (see
    // deputyctl::apibase). Skipped for the default backend so production
    // appliances stay free of a redundant file.
    if api_base != apibase::DEFAULT_API_BASE {
        if let Err(e) = apply::write_atomic(
            &root.join("etc/deputyos/api-base"),
            &format!("{api_base}\n"),
            0o644,
        ) {
            return PollResult::Other(format!("writing api-base: {e}"));
        }
    }

    // Registration completes the condition for the already-enabled tunnel
    // unit. Start it immediately so remote access is available without
    // waiting for the reconciliation timer or another boot.
    if start_tunnel {
        let status = std::process::Command::new("systemctl")
            .args(["start", "deputyos-tunnel.service"])
            .status();
        if !matches!(status, Ok(status) if status.success()) {
            eprintln!(
                "wizard: registered successfully; tunnel start deferred to reconciliation timer"
            );
        }
    }

    PollResult::Registered
}

// ---------------------------------------------------------------------------
// Step 7: cloudflare-tunnel
// ---------------------------------------------------------------------------

async fn get_cloudflare_tunnel(State(app): State<AppState>) -> Response {
    let s = app.state.lock().expect("state lock").clone();
    let html = templates::step_cloudflare_tunnel(
        &s,
        app.limits.as_ref().as_ref(),
        s.answers.account_registered,
        None,
    );
    Html(html).into_response()
}

#[derive(Debug, Deserialize)]
struct CloudflareTunnelForm {
    choice: String,
    #[serde(default)]
    credentials: Option<String>,
}

async fn post_cloudflare_tunnel(
    State(app): State<AppState>,
    Form(f): Form<CloudflareTunnelForm>,
) -> Response {
    let choice = f.choice.trim();
    let mut tunnel_name: Option<String> = None;
    match choice {
        "skip" | "quick" | "integrated" => {
            *app.pending_cloudflared.lock().expect("pending lock") = None;
        }
        "named" => {
            let raw = f.credentials.unwrap_or_default();
            let raw = raw.trim();
            if raw.is_empty() {
                return render_step_with_error(
                    &app,
                    Step::CloudflareTunnel,
                    "Named tunnel requires the credentials JSON.",
                );
            }
            let parsed: serde_json::Value = match serde_json::from_str(raw) {
                Ok(v) => v,
                Err(e) => {
                    return render_step_with_error(
                        &app,
                        Step::CloudflareTunnel,
                        &format!("credentials JSON is invalid: {e}"),
                    );
                }
            };
            tunnel_name = parsed
                .get("TunnelName")
                .and_then(|v| v.as_str())
                .map(String::from)
                .or_else(|| {
                    parsed
                        .get("TunnelID")
                        .and_then(|v| v.as_str())
                        .map(String::from)
                });
            if tunnel_name.is_none() {
                return render_step_with_error(
                    &app,
                    Step::CloudflareTunnel,
                    "credentials JSON must include either TunnelName or TunnelID.",
                );
            }
            *app.pending_cloudflared.lock().expect("pending lock") = Some(raw.to_string());
        }
        _ => {
            return render_step_with_error(
                &app,
                Step::CloudflareTunnel,
                "Unknown tunnel choice — pick integrated, skip, quick, or named.",
            );
        }
    }
    {
        let mut g = app.state.lock().expect("state lock");
        g.answers.cloudflare_tunnel_choice = Some(choice.to_string());
        g.answers.cloudflare_tunnel_name = tunnel_name;
        g.step = Step::Backup;
        let _ = state::save(&app.state_file, &g);
    }
    Redirect::to(&step_url(Step::Backup)).into_response()
}

// ---------------------------------------------------------------------------
// Step 8: backup
// ---------------------------------------------------------------------------

async fn get_backup(State(app): State<AppState>) -> Response {
    let s = app.state.lock().expect("state lock").clone();
    let html = templates::step_backup(&s, app.limits.as_ref().as_ref(), None);
    Html(html).into_response()
}

#[derive(Debug, Deserialize)]
struct BackupForm {
    kind: String,
    #[serde(default)]
    b2_account_id: Option<String>,
    #[serde(default)]
    b2_application_key: Option<String>,
    #[serde(default)]
    b2_bucket: Option<String>,
    #[serde(default)]
    r2_account_id: Option<String>,
    #[serde(default)]
    r2_access_key: Option<String>,
    #[serde(default)]
    r2_secret_key: Option<String>,
    #[serde(default)]
    r2_bucket: Option<String>,
    #[serde(default)]
    s3_endpoint: Option<String>,
    #[serde(default)]
    s3_access_key: Option<String>,
    #[serde(default)]
    s3_secret_key: Option<String>,
    #[serde(default)]
    s3_bucket: Option<String>,
}

async fn post_backup(State(app): State<AppState>, Form(f): Form<BackupForm>) -> Response {
    use std::collections::BTreeMap;
    let kind = f.kind.trim().to_string();
    let mut meta: BTreeMap<String, String> = BTreeMap::new();
    let mut fields: BTreeMap<String, String> = BTreeMap::new();
    fn req(name: &str, v: Option<String>) -> Result<String, String> {
        let v = v.unwrap_or_default().trim().to_string();
        if v.is_empty() {
            return Err(format!("{name} is required for this backup kind."));
        }
        Ok(v)
    }
    match kind.as_str() {
        "skip" => {
            *app.pending_backup.lock().expect("pending lock") = None;
        }
        "managed" => {
            *app.pending_backup.lock().expect("pending lock") = Some(PendingBackup {
                kind: "managed".into(),
                fields,
            });
        }
        "b2" => {
            let account_id = match req("Account ID", f.b2_account_id) {
                Ok(v) => v,
                Err(e) => return render_step_with_error(&app, Step::Backup, &e),
            };
            let app_key = match req("Application key", f.b2_application_key) {
                Ok(v) => v,
                Err(e) => return render_step_with_error(&app, Step::Backup, &e),
            };
            let bucket = match req("Bucket", f.b2_bucket) {
                Ok(v) => v,
                Err(e) => return render_step_with_error(&app, Step::Backup, &e),
            };
            meta.insert("bucket".into(), bucket.clone());
            meta.insert("account_id".into(), account_id.clone());
            fields.insert("account".into(), account_id);
            fields.insert("key".into(), app_key);
            fields.insert("bucket".into(), bucket);
            *app.pending_backup.lock().expect("pending lock") = Some(PendingBackup {
                kind: "b2".into(),
                fields,
            });
        }
        "r2" => {
            let account_id = match req("Account ID", f.r2_account_id) {
                Ok(v) => v,
                Err(e) => return render_step_with_error(&app, Step::Backup, &e),
            };
            let access = match req("Access key", f.r2_access_key) {
                Ok(v) => v,
                Err(e) => return render_step_with_error(&app, Step::Backup, &e),
            };
            let secret = match req("Secret key", f.r2_secret_key) {
                Ok(v) => v,
                Err(e) => return render_step_with_error(&app, Step::Backup, &e),
            };
            let bucket = match req("Bucket", f.r2_bucket) {
                Ok(v) => v,
                Err(e) => return render_step_with_error(&app, Step::Backup, &e),
            };
            meta.insert("bucket".into(), bucket.clone());
            meta.insert("account_id".into(), account_id.clone());
            fields.insert("account".into(), account_id.clone());
            fields.insert("access_key_id".into(), access);
            fields.insert("secret_access_key".into(), secret);
            fields.insert("bucket".into(), bucket);
            fields.insert(
                "endpoint".into(),
                format!("https://{account_id}.r2.cloudflarestorage.com"),
            );
            *app.pending_backup.lock().expect("pending lock") = Some(PendingBackup {
                kind: "r2".into(),
                fields,
            });
        }
        "s3" => {
            let endpoint = match req("Endpoint URL", f.s3_endpoint) {
                Ok(v) => v,
                Err(e) => return render_step_with_error(&app, Step::Backup, &e),
            };
            let access = match req("Access key", f.s3_access_key) {
                Ok(v) => v,
                Err(e) => return render_step_with_error(&app, Step::Backup, &e),
            };
            let secret = match req("Secret key", f.s3_secret_key) {
                Ok(v) => v,
                Err(e) => return render_step_with_error(&app, Step::Backup, &e),
            };
            let bucket = match req("Bucket", f.s3_bucket) {
                Ok(v) => v,
                Err(e) => return render_step_with_error(&app, Step::Backup, &e),
            };
            meta.insert("bucket".into(), bucket.clone());
            meta.insert("endpoint".into(), endpoint.clone());
            fields.insert("endpoint".into(), endpoint);
            fields.insert("access_key_id".into(), access);
            fields.insert("secret_access_key".into(), secret);
            fields.insert("bucket".into(), bucket);
            *app.pending_backup.lock().expect("pending lock") = Some(PendingBackup {
                kind: "s3".into(),
                fields,
            });
        }
        _ => {
            return render_step_with_error(
                &app,
                Step::Backup,
                "Unknown backup kind — pick managed, skip, b2, r2, or s3.",
            );
        }
    }
    {
        let mut g = app.state.lock().expect("state lock");
        g.answers.backup_kind = Some(kind);
        g.answers.backup_meta = meta;
        g.step = Step::Drives;
        let _ = state::save(&app.state_file, &g);
    }
    Redirect::to(&step_url(Step::Drives)).into_response()
}

// ---------------------------------------------------------------------------
// Step 9: drives (M3.5, hybrid). Surfaces configured mounts + the active
// profile's suggested paths and links to the standalone /mounts page for
// add/revoke. The step itself only flips `drives_acknowledged`; it does not
// mutate the mounts policy (mounts are live-mutable, not a one-shot choice).
// ---------------------------------------------------------------------------

async fn get_drives(State(app): State<AppState>) -> Response {
    let s = app.state.lock().expect("state lock").clone();
    let entries = tokio::task::spawn_blocking(|| deputyctl::mounts::list(None))
        .await
        .unwrap_or_else(|_| Ok(Vec::new()))
        .unwrap_or_default();
    let (suggested, default_mode) = profile_mounts_hints(&app, &s);
    let html = templates::step_drives(
        &s,
        app.limits.as_ref().as_ref(),
        &entries,
        &suggested,
        &default_mode,
        None,
    );
    Html(html).into_response()
}

#[derive(Debug, Deserialize)]
struct DrivesForm {}

async fn post_drives(State(app): State<AppState>, Form(_f): Form<DrivesForm>) -> Response {
    {
        let mut g = app.state.lock().expect("state lock");
        g.answers.drives_acknowledged = true;
        g.step = Step::Review;
        let _ = state::save(&app.state_file, &g);
    }
    Redirect::to(&step_url(Step::Review)).into_response()
}

/// Pull the active profile's `[mounts]` hints (suggested paths + default mode)
/// for the Drives step. Returns `(Vec::new(), "ro")` when the profile has no
/// `[mounts]` section or no profile is chosen yet — the step renders a muted
/// "no suggestions" line in that case.
fn profile_mounts_hints(app: &AppState, s: &state::WizardState) -> (Vec<String>, String) {
    let id = match s.answers.profile.as_deref() {
        Some(id) => id,
        None => return (Vec::new(), "ro".into()),
    };
    let manifest = match app.profiles.iter().find(|(_, m)| m.profile.id == id) {
        Some((_, m)) => m,
        None => return (Vec::new(), "ro".into()),
    };
    match &manifest.mounts {
        Some(sec) => (
            sec.suggested_paths.clone(),
            if sec.default_mode.is_empty() {
                "ro".into()
            } else {
                sec.default_mode.clone()
            },
        ),
        None => (Vec::new(), "ro".into()),
    }
}

/// Resolve the active profile's `[default_egress]` hints for the Egress step
/// (M5.5 Lane F): a recommended mode to pre-select and a starter allow-list
/// to show as hints. Returns `("open", [])` when no profile is chosen or the
/// profile declares no `[default_egress]` section — the wizard's open default.
fn profile_egress_hints(app: &AppState, s: &state::WizardState) -> (String, Vec<String>) {
    let id = match s.answers.profile.as_deref() {
        Some(id) => id,
        None => return ("open".into(), Vec::new()),
    };
    let manifest = match app.profiles.iter().find(|(_, m)| m.profile.id == id) {
        Some((_, m)) => m,
        None => return ("open".into(), Vec::new()),
    };
    match &manifest.default_egress {
        Some(sec) => (
            if sec.mode.is_empty() {
                "open".into()
            } else {
                sec.mode.clone()
            },
            sec.allow_hosts.clone(),
        ),
        None => ("open".into(), Vec::new()),
    }
}

// ---------------------------------------------------------------------------
// Final step: review + apply
// ---------------------------------------------------------------------------

async fn get_review(State(app): State<AppState>) -> Response {
    let s = app.state.lock().expect("state lock").clone();
    let html = templates::step_review(&s, app.limits.as_ref().as_ref());
    Html(html).into_response()
}

async fn post_apply(State(app): State<AppState>) -> Response {
    let s = app.state.lock().expect("state lock").clone();
    let pending = app.pending_secret.lock().expect("pending lock").take();
    let pending_ts = app.pending_tailscale.lock().expect("pending lock").take();
    let pending_cf = app.pending_cloudflared.lock().expect("pending lock").take();
    let pending_bu = app.pending_backup.lock().expect("pending lock").take();

    let provider: Option<Provider> = s
        .answers
        .provider
        .as_deref()
        .and_then(|id| app.providers.providers.iter().find(|p| p.id == id))
        .cloned();
    let provider_secret = match (provider.as_ref(), pending.as_ref()) {
        (Some(p), Some(ps)) if ps.provider_id == p.id => Some((p, ps.api_key.as_str())),
        _ => None,
    };

    let active_manifest = s
        .answers
        .profile
        .as_deref()
        .and_then(|id| app.profiles.iter().find(|(_, m)| m.profile.id == id));
    let ports: Vec<u16> = active_manifest
        .map(|(_, m)| m.service.ports.clone())
        .unwrap_or_default();
    let unit = active_manifest.map(|(_, m)| m.service.unit.as_str());

    let extras = apply::ApplyExtras {
        tailscale_authkey: pending_ts.as_deref(),
        cloudflared_credentials: pending_cf.as_deref(),
        backup: pending_bu.as_ref().map(|pb| apply::BackupRef {
            kind: pb.kind.as_str(),
            fields: &pb.fields,
        }),
    };

    match apply::apply(
        app.apply_mode,
        app.dev_out.as_deref(),
        &s.answers,
        provider_secret,
        &ports,
        unit,
        &extras,
    ) {
        Ok(_report) => {
            let mut g = app.state.lock().expect("state lock");
            g.step = Step::Done;
            g.completed_at = Some(now_rfc3339());
            let _ = state::save(&app.state_file, &g);
            Redirect::to("/wizard/done").into_response()
        }
        Err(e) => render_step_with_error(&app, Step::Review, &format!("Apply failed: {e}")),
    }
}

// ---------------------------------------------------------------------------
// /chat — built-in private web chat
// ---------------------------------------------------------------------------

async fn get_chat(State(app): State<AppState>) -> Response {
    let s = app.state.lock().expect("state lock").clone();
    let history = chat::load_history(&chat_history_path(&app, &s));
    let html = templates::page_chat(&s, app.limits.as_ref().as_ref(), &history);
    Html(html).into_response()
}

#[derive(Debug, Deserialize)]
struct ChatMessageForm {
    message: String,
}

async fn post_chat_message(
    State(app): State<AppState>,
    Form(f): Form<ChatMessageForm>,
) -> Response {
    let msg = f.message.trim().to_string();
    if msg.is_empty() {
        return Html("<div class=\"error\">message is empty</div>").into_response();
    }
    let s = app.state.lock().expect("state lock").clone();
    let endpoint = agent_base(&app, &s);
    let now = now_rfc3339();
    let user_turn = ChatTurn {
        role: "user".into(),
        content: msg.clone(),
        at: now.clone(),
    };
    let path = chat_history_path(&app, &s);
    let _ = chat::append_turn(&path, &user_turn);

    let endpoint_for_call = endpoint.clone();
    let msg_for_call = msg.clone();
    let reply =
        tokio::task::spawn_blocking(move || chat::ask_agent(&endpoint_for_call, &msg_for_call))
            .await
            .unwrap_or(AgentReply::Unavailable("chat task panicked".into()));
    let assistant_text = match reply {
        AgentReply::Ok(t) => t,
        AgentReply::Unavailable(t) => t,
    };
    let assistant_turn = ChatTurn {
        role: "assistant".into(),
        content: assistant_text,
        at: now_rfc3339(),
    };
    let _ = chat::append_turn(&path, &assistant_turn);

    let history = chat::load_history(&path);
    let body = templates::render_chat_messages(&history);
    Html(body).into_response()
}

fn chat_history_path(app: &AppState, s: &WizardState) -> PathBuf {
    if let Some(p) = app.chat_history_override.as_ref() {
        return p.clone();
    }
    if let Ok(p) = std::env::var("DEPUTYWIZARD_CHAT_HISTORY") {
        return PathBuf::from(p);
    }
    // Honour the active profile's data_dir if known. In dev mode, root the
    // path under dev-out so we don't write into a real ~/.openclaw.
    let data_dir = s
        .answers
        .profile
        .as_deref()
        .and_then(|id| app.profiles.iter().find(|(_, m)| m.profile.id == id))
        .map(|(_, m)| PathBuf::from(&m.paths.data_dir));
    if app.apply_mode == ApplyMode::Dev {
        let root = app
            .dev_out
            .clone()
            .unwrap_or_else(|| PathBuf::from("dev-out"));
        if let Some(d) = data_dir {
            let stripped: PathBuf = d
                .strip_prefix("/")
                .map(|p| p.to_path_buf())
                .unwrap_or(d.clone());
            return root.join(stripped).join("chat-history.jsonl");
        }
        return root.join("chat-history.jsonl");
    }
    chat::history_path(data_dir.as_deref())
}

fn agent_base(app: &AppState, s: &WizardState) -> String {
    if let Some(o) = app.agent_base_override.as_deref() {
        return o.to_string();
    }
    s.answers
        .profile
        .as_deref()
        .and_then(|id| app.profiles.iter().find(|(_, m)| m.profile.id == id))
        .map(|(_, m)| chat::agent_base_from_health_check(&m.health.http_check))
        .unwrap_or_else(|| "http://127.0.0.1:8080".into())
}

async fn get_done(State(app): State<AppState>) -> Response {
    let s = app.state.lock().expect("state lock").clone();
    let mode = match app.apply_mode {
        ApplyMode::Production => "production",
        ApplyMode::Dev => "dev",
    };
    let html = templates::page_done(&s, app.limits.as_ref().as_ref(), mode);
    Html(html).into_response()
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn render_step_with_error(app: &AppState, step: Step, error: &str) -> Response {
    let s = app.state.lock().expect("state lock").clone();
    let html = match step {
        Step::System => templates::step_system(&s, app.limits.as_ref().as_ref(), Some(error)),
        Step::Profile => {
            let choices = profile_choices(app);
            templates::step_profile(&s, app.limits.as_ref().as_ref(), &choices, Some(error))
        }
        Step::Provider => {
            let (choices, airgap) = provider_choices(app);
            templates::step_provider(
                &s,
                app.limits.as_ref().as_ref(),
                &choices,
                Some(error),
                airgap,
            )
        }
        Step::Channels => {
            let (supported, disabled) = channel_lists(app, &s);
            templates::step_channels(
                &s,
                app.limits.as_ref().as_ref(),
                &supported,
                &disabled,
                Some(error),
            )
        }
        Step::Egress => {
            let (mode, hosts) = profile_egress_hints(app, &s);
            templates::step_egress(&s, app.limits.as_ref().as_ref(), &mode, &hosts)
        }
        Step::Ssh => templates::step_ssh(&s, app.limits.as_ref().as_ref(), Some(error)),
        Step::Tailscale => templates::step_tailscale(&s, app.limits.as_ref().as_ref(), Some(error)),
        Step::Account => {
            let pending = app
                .pending_device_code
                .lock()
                .expect("pending lock")
                .clone();
            let view = templates::AccountView {
                registered: s.answers.account_registered,
                user_code: pending.as_ref().map(|d| d.user_code.as_str()),
                verification_uri: pending.as_ref().map(|d| d.verification_uri.as_str()),
                email: s.answers.account_email.as_deref(),
                api_base: s.answers.account_api_base.as_deref(),
                note: Some(error),
                note_is_error: true,
            };
            templates::step_account(&s, app.limits.as_ref().as_ref(), &view)
        }
        Step::CloudflareTunnel => templates::step_cloudflare_tunnel(
            &s,
            app.limits.as_ref().as_ref(),
            s.answers.account_registered,
            Some(error),
        ),
        Step::Backup => templates::step_backup(&s, app.limits.as_ref().as_ref(), Some(error)),
        Step::Drives => {
            let entries = deputyctl::mounts::list(None).unwrap_or_default();
            let (suggested, default_mode) = profile_mounts_hints(app, &s);
            templates::step_drives(
                &s,
                app.limits.as_ref().as_ref(),
                &entries,
                &suggested,
                &default_mode,
                Some(error),
            )
        }
        Step::Review => templates::step_review(&s, app.limits.as_ref().as_ref()),
        Step::Done => templates::page_done(&s, app.limits.as_ref().as_ref(), "dev"),
    };
    (
        StatusCode::OK,
        [(header::CONTENT_TYPE, "text/html; charset=utf-8")],
        html,
    )
        .into_response()
}

fn now_rfc3339() -> String {
    use time::format_description::well_known::Rfc3339;
    time::OffsetDateTime::now_utc()
        .format(&Rfc3339)
        .unwrap_or_else(|_| "1970-01-01T00:00:00Z".into())
}

// ---------------------------------------------------------------------------
// Mounts (M3.5) — standalone page outside the linear wizard step machine.
// Reachable at /mounts after the wizard completes (or at any time during it
// once the user knows what they want to share). Backed by deputyctl::mounts.
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct MountsAddForm {
    id: String,
    host_path: String,
    guest_path: String,
    mode: String,
}

#[derive(Debug, Deserialize)]
struct MountsRemoveForm {
    id: String,
}

async fn get_mounts(State(app): State<AppState>) -> Response {
    let entries = tokio::task::spawn_blocking(|| deputyctl::mounts::list(None))
        .await
        .unwrap_or_else(|_| Ok(Vec::new()))
        .unwrap_or_default();
    let html = templates::page_mounts(app.limits.as_ref().as_ref(), &entries, None);
    Html(html).into_response()
}

async fn post_mounts_add(State(app): State<AppState>, Form(f): Form<MountsAddForm>) -> Response {
    use std::str::FromStr;
    let mode = match deputyctl::mounts::Mode::from_str(&f.mode) {
        Ok(m) => m,
        Err(e) => {
            let entries = deputyctl::mounts::list(None).unwrap_or_default();
            return Html(templates::page_mounts(
                app.limits.as_ref().as_ref(),
                &entries,
                Some(&format!("Bad mode: {e}")),
            ))
            .into_response();
        }
    };
    let id = f.id.clone();
    let res = tokio::task::spawn_blocking(move || {
        deputyctl::mounts::add_host_fs(None, &f.id, &f.host_path, &f.guest_path, mode)
    })
    .await
    .unwrap_or_else(|e| Err(anyhow::anyhow!("add task panicked: {e}")));
    let flash = match res {
        Ok(_) => format!("Added mount {id:?}."),
        Err(e) => format!("Could not add {id:?}: {e}"),
    };
    let entries = deputyctl::mounts::list(None).unwrap_or_default();
    Html(templates::page_mounts(
        app.limits.as_ref().as_ref(),
        &entries,
        Some(&flash),
    ))
    .into_response()
}

async fn post_mounts_remove(
    State(app): State<AppState>,
    Form(f): Form<MountsRemoveForm>,
) -> Response {
    let id = f.id.clone();
    let res = tokio::task::spawn_blocking(move || deputyctl::mounts::remove_by_id(None, &f.id))
        .await
        .unwrap_or_else(|e| Err(anyhow::anyhow!("remove task panicked: {e}")));
    let flash = match res {
        Ok(_) => format!("Revoked mount {id:?}."),
        Err(e) => format!("Could not revoke {id:?}: {e}"),
    };
    let entries = deputyctl::mounts::list(None).unwrap_or_default();
    Html(templates::page_mounts(
        app.limits.as_ref().as_ref(),
        &entries,
        Some(&flash),
    ))
    .into_response()
}

#[derive(Debug, Deserialize)]
struct MountsNetworkAddForm {
    id: String,
    kind: String,
    source: String,
    guest_path: String,
    mode: String,
    #[serde(default)]
    credentials_env: Option<String>,
}

async fn post_mounts_network_add(
    State(app): State<AppState>,
    Form(f): Form<MountsNetworkAddForm>,
) -> Response {
    use std::str::FromStr;
    let mode = match deputyctl::mounts::Mode::from_str(&f.mode) {
        Ok(m) => m,
        Err(e) => {
            let entries = deputyctl::mounts::list(None).unwrap_or_default();
            return Html(templates::page_mounts(
                app.limits.as_ref().as_ref(),
                &entries,
                Some(&format!("Bad mode: {e}")),
            ))
            .into_response();
        }
    };
    let creds = f
        .credentials_env
        .as_deref()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());
    let id = f.id.clone();
    let res = tokio::task::spawn_blocking(move || {
        deputyctl::mounts::add_network_mount(
            None,
            &f.id,
            &f.kind,
            &f.source,
            &f.guest_path,
            mode,
            creds.as_deref(),
        )
    })
    .await
    .unwrap_or_else(|e| Err(anyhow::anyhow!("network-add task panicked: {e}")));
    let flash = match res {
        Ok(_) => format!("Added network share {id:?}."),
        Err(e) => format!("Could not add {id:?}: {e}"),
    };
    let entries = deputyctl::mounts::list(None).unwrap_or_default();
    Html(templates::page_mounts(
        app.limits.as_ref().as_ref(),
        &entries,
        Some(&flash),
    ))
    .into_response()
}
