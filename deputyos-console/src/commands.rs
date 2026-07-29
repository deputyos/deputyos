//! Tauri command surface — `#[tauri::command]` wrappers around the core
//! ([`crate::api_client`], [`crate::instance_ops`], [`crate::store`]).
//!
//! Only compiled with the `gui` feature (it pulls `tauri`). Each command
//! returns `Result<T, String>` so Tauri surfaces errors to the frontend as a
//! rejected invoke promise the `app.js` can render.
//!
//! The login flow is two-step (`login_start` → poll `login_poll` at the
//! `interval` the API returns) so the frontend can show the user code +
//! verification URI and update when the user confirms. On success the tokens
//! are persisted via the [`TokenStore`] (keyring in the GUI build).

use std::sync::Mutex;

use anyhow::Result as AnyResult;
use serde::Serialize;
use tauri::State;

use crate::api_client::{ApiClient, DeviceEntry, PollOutcome, RemoteCommand};
use crate::instance_ops::InstanceOps;
use crate::store::{FileTokenStore, KeyringStore, StoredTokens, TokenStore};

/// Shared GUI state: the API client (pointing at the API base), the instance
/// ops, and the token store. Held in Tauri's managed state behind a Mutex
/// (the API client is cheaply cloneable; the store is cheap to call).
pub struct GuiState {
    pub api: ApiClient,
    pub ops: InstanceOps,
    pub store: Mutex<Box<dyn TokenStore + Send + Sync>>,
    /// The in-flight device code from the last `login_start` (so `login_poll`
    /// knows what to poll without the frontend echoing it back).
    pub pending_device_code: Mutex<Option<String>>,
}

impl GuiState {
    pub fn new(api_base: &str) -> Self {
        // Prefer the OS keychain; fall back to the file store if the keyring
        // entry can't even be opened (no Secret Service on a headless host,
        // locked keychain, etc.). `entry()` is the cheapest probe we have.
        let store: Box<dyn TokenStore + Send + Sync> = match KeyringStore::default_store().entry() {
            Ok(_) => Box::new(KeyringStore::default_store()),
            Err(_) => Box::new(FileTokenStore::default_store()),
        };
        Self {
            api: ApiClient::new(api_base),
            ops: InstanceOps::new(),
            store: Mutex::new(store),
            pending_device_code: Mutex::new(None),
        }
    }
}

// ---- login ----

#[derive(Serialize)]
pub struct LoginStart {
    pub user_code: String,
    pub verification_uri: String,
    pub expires_in: u64,
    pub interval: u64,
}

#[tauri::command]
pub fn login_start(
    state: State<'_, GuiState>,
    client_name: String,
) -> AnyResult<LoginStart, String> {
    let r = state.api.device_code_start(&client_name).map_err(to_err)?;
    *state
        .pending_device_code
        .lock()
        .map_err(|e| e.to_string())? = Some(r.device_code.clone());
    Ok(LoginStart {
        user_code: r.user_code,
        verification_uri: r.verification_uri,
        expires_in: r.expires_in,
        interval: r.interval,
    })
}

#[derive(Serialize)]
pub struct LoginStatus {
    pub status: String,
    pub account_id: Option<String>,
}

#[tauri::command]
pub fn login_poll(state: State<'_, GuiState>) -> AnyResult<LoginStatus, String> {
    let device_code = state
        .pending_device_code
        .lock()
        .map_err(|e| e.to_string())?
        .clone()
        .ok_or_else(|| "no login in progress; call login_start first".to_string())?;
    match state.api.device_code_poll(&device_code).map_err(to_err)? {
        PollOutcome::Pending => Ok(LoginStatus {
            status: "pending".into(),
            account_id: None,
        }),
        PollOutcome::Authorized(pair) => {
            let account_id = pair.account_id.clone();
            let stored = StoredTokens::from_pair(pair);
            state
                .store
                .lock()
                .map_err(|e| e.to_string())?
                .save(&stored)
                .map_err(to_err)?;
            // Clear the pending code — login is complete.
            *state
                .pending_device_code
                .lock()
                .map_err(|e| e.to_string())? = None;
            Ok(LoginStatus {
                status: "authorized".into(),
                account_id,
            })
        }
    }
}

#[tauri::command]
pub fn login_status(state: State<'_, GuiState>) -> AnyResult<LoginStatus, String> {
    let tokens = state
        .store
        .lock()
        .map_err(|e| e.to_string())?
        .load()
        .map_err(to_err)?;
    Ok(match tokens {
        Some(t) => LoginStatus {
            status: "authorized".into(),
            account_id: t.account_id,
        },
        None => LoginStatus {
            status: "logged_out".into(),
            account_id: None,
        },
    })
}

#[tauri::command]
pub fn logout(state: State<'_, GuiState>) -> AnyResult<(), String> {
    // Best-effort revoke the refresh token, then clear local storage.
    if let Some(t) = state
        .store
        .lock()
        .map_err(|e| e.to_string())?
        .load()
        .map_err(to_err)?
    {
        let _ = state.api.revoke(&t.refresh_token, Some(&t.access_token));
    }
    state
        .store
        .lock()
        .map_err(|e| e.to_string())?
        .clear()
        .map_err(to_err)?;
    Ok(())
}

// ---- local instances ----

#[tauri::command]
pub fn list_instances(state: State<'_, GuiState>) -> AnyResult<Vec<crate::Instance>, String> {
    state.ops.list().map_err(to_err)
}

#[tauri::command]
pub fn create_instance(
    state: State<'_, GuiState>,
    name: String,
    profile: Option<String>,
    manifest_url: Option<String>,
    channel: Option<String>,
) -> AnyResult<crate::Instance, String> {
    state
        .ops
        .create(&name, profile, manifest_url, channel)
        .map_err(to_err)
}

#[tauri::command]
pub fn delete_instance(state: State<'_, GuiState>, id: String) -> AnyResult<(), String> {
    state.ops.delete(&id).map_err(to_err)
}

#[tauri::command]
pub fn start_instance(state: State<'_, GuiState>, id: String) -> AnyResult<String, String> {
    state.ops.start(&id).map_err(to_err)
}

#[tauri::command]
pub fn stop_instance(state: State<'_, GuiState>, id: String) -> AnyResult<(), String> {
    state.ops.stop(&id).map_err(to_err)
}

#[tauri::command]
pub fn pause_instance(state: State<'_, GuiState>, id: String) -> AnyResult<(), String> {
    state.ops.pause(&id).map_err(to_err)
}

#[tauri::command]
pub fn resume_instance(state: State<'_, GuiState>, id: String) -> AnyResult<String, String> {
    state.ops.resume(&id).map_err(to_err)
}

#[tauri::command]
pub fn set_instance_memory(
    state: State<'_, GuiState>,
    id: String,
    target_mib: u64,
) -> AnyResult<(), String> {
    state.ops.set_memory(&id, target_mib).map_err(to_err)
}

#[tauri::command]
pub fn configure_instance_resources(
    state: State<'_, GuiState>,
    id: String,
    resources: deputyos_desktop::ResourceSpec,
) -> AnyResult<crate::Instance, String> {
    state
        .ops
        .configure_resources(&id, resources)
        .map_err(to_err)
}

#[tauri::command]
pub fn instance_agent_health(
    state: State<'_, GuiState>,
    id: String,
) -> AnyResult<deputyd::AgentResult, String> {
    state.ops.agent_health(&id).map_err(to_err)
}

#[tauri::command]
pub fn status_instance(
    state: State<'_, GuiState>,
    id: String,
) -> AnyResult<crate::VmStatus, String> {
    state.ops.status(&id).map_err(to_err)
}

#[tauri::command]
pub fn install_instance(state: State<'_, GuiState>, id: String) -> AnyResult<(), String> {
    state.ops.install(&id).map_err(to_err)
}

#[tauri::command]
pub fn open_wizard(state: State<'_, GuiState>, id: String) -> AnyResult<String, String> {
    state.ops.wizard_url(&id).map_err(to_err)
}

/// Open a URL in the user's default browser. The wizard (and the remote
/// tunnel wizard) are normal HTTP UIs; the frontend calls this instead of a
/// webview navigation so the user gets a real browser tab. Reuses the
/// launcher's `browser::open_url` (the `webbrowser` crate), which respects
/// `DEPUTYOS_DESKTOP_NO_BROWSER=1` for headless/CI.
#[tauri::command]
pub fn open_url(url: String) -> AnyResult<(), String> {
    deputyos_desktop::browser::open_url(&url).map_err(to_err)
}

/// Host virtualization readiness for **local** agents. `ok=true` means the
/// host driver's prerequisite (qemu+KVM on Linux, WSL2 on Windows, UTM on
/// macOS) is present; on `false`, `message` carries the driver's user-facing
/// install hint and `target` names the image family this host uses. The
/// frontend renders this as a banner so a missing prereq (or the
/// "local agents require Linux in v1" unsupported case) is visible instead of
/// every start/install silently failing.
#[derive(Serialize)]
pub struct HostPrereq {
    pub ok: bool,
    pub target: String,
    pub message: Option<String>,
    pub capabilities: deputyos_desktop::DriverCapabilities,
}

#[tauri::command]
pub fn host_prereq(state: State<'_, GuiState>) -> AnyResult<HostPrereq, String> {
    let driver = state.ops.driver();
    let target = driver.target_for_host().to_string();
    match driver.check_prereq() {
        Ok(()) => Ok(HostPrereq {
            ok: true,
            target,
            message: None,
            capabilities: driver.capabilities(),
        }),
        Err(e) => Ok(HostPrereq {
            ok: false,
            target,
            message: Some(format!("{e:#}")),
            capabilities: driver.capabilities(),
        }),
    }
}

// ---- fleet (remote management, Phase C) ----

#[tauri::command]
pub fn list_fleet(state: State<'_, GuiState>) -> AnyResult<Vec<DeviceEntry>, String> {
    let tokens = current_tokens(&state)?;
    state.api.list_devices(&tokens.access_token).map_err(to_err)
}

#[tauri::command]
pub fn open_remote_wizard(
    state: State<'_, GuiState>,
    device_id: String,
) -> AnyResult<String, String> {
    let tokens = current_tokens(&state)?;
    // The remote wizard opens through the tunnel proxy with the account JWT
    // as a query token (Phase C wires the proxy + wizard AccountOwner auth).
    // Chat is the wizard's /chat over the same tunnel.
    Ok(format!(
        "{}?token={}",
        state.api.tunnel_proxy_url(&device_id, ""),
        urlencoding(&tokens.access_token)
    ))
}

/// Build a browser URL for one of the tunnel's compiled-in surfaces.
/// The relay and guest independently enforce the same mapping.
#[tauri::command]
pub fn open_remote_surface(
    state: State<'_, GuiState>,
    device_id: String,
    surface: String,
) -> AnyResult<String, String> {
    let tokens = current_tokens(&state)?;
    let base = state
        .api
        .tunnel_surface_url(&device_id, &surface)
        .map_err(to_err)?;
    Ok(format!(
        "{}?token={}",
        base,
        urlencoding(&tokens.access_token)
    ))
}

/// Queue a safe resident-agent operation. The API and guest independently
/// validate the same allowlist; this local check also narrows the webview
/// command boundary.
#[tauri::command]
pub fn queue_remote_command(
    state: State<'_, GuiState>,
    device_id: String,
    command: String,
) -> AnyResult<RemoteCommand, String> {
    if !matches!(
        command.as_str(),
        "agent.health"
            | "workload.restart"
            | "repair.run"
            | "update.run"
            | "memory.reclaim"
            | "backup.run"
    ) {
        return Err("unsupported console operation".into());
    }
    let tokens = current_tokens(&state)?;
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis())
        .unwrap_or(0);
    let params = if command == "memory.reclaim" {
        serde_json::json!({"drop_caches": false})
    } else {
        serde_json::json!({})
    };
    state
        .api
        .enqueue_command(
            &tokens.access_token,
            &device_id,
            &command,
            params,
            &format!("console:{device_id}:{command}:{now}"),
        )
        .map_err(to_err)
}

// ---- helpers ----

fn current_tokens(state: &State<'_, GuiState>) -> AnyResult<StoredTokens, String> {
    state
        .store
        .lock()
        .map_err(|e| e.to_string())?
        .load()
        .map_err(to_err)?
        .ok_or_else(|| "not logged in".to_string())
}

/// Minimal URL-encoding of the JWT for a query param (encode everything
/// that's not URL-safe in a JWT — JWTs are base64url which is already safe, so
/// this is a no-op pass-through; kept explicit for clarity + future-proofing).
fn urlencoding(s: &str) -> String {
    s.to_string()
}

/// Flatten anyhow errors into a String for the Tauri boundary.
fn to_err(e: anyhow::Error) -> String {
    format!("{e:#}")
}
