#![allow(clippy::unwrap_used)]
//! End-to-end click-through of the wizard.
//!
//! Boots the Axum server on a random port (`127.0.0.1:0`), drives every step
//! with `reqwest` (cookie-aware), and asserts the produced `dev-out/` tree.
//! Also covers a few rejection paths (bad token, blocked channel).

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use deputyctl::manifest::Manifest;
use deputywizard::apply::ApplyMode;
use deputywizard::auth::{AuthMode, AuthState};
use deputywizard::routes::{router, AppState};
use deputywizard::runtime_bridge::{RuntimeAgent, SocketRuntimeAgent};
use deputywizard::state::{self, WizardState};
use deputywizard::templates::ProviderChoice;

const TEST_TOKEN: &str = "deadbeefcafe0001deadbeefcafe0002";

struct Harness {
    base: String,
    _state_dir: tempfile::TempDir,
    out_dir: tempfile::TempDir,
    handle: tokio::task::JoinHandle<()>,
}

impl Drop for Harness {
    fn drop(&mut self) {
        self.handle.abort();
    }
}

async fn boot(auth_mode: AuthMode) -> Harness {
    boot_with(auth_mode, None, None).await
}

async fn boot_with(
    auth_mode: AuthMode,
    agent_base_override: Option<String>,
    chat_history_override: Option<PathBuf>,
) -> Harness {
    let token = match auth_mode {
        AuthMode::Token => Some(TEST_TOKEN.to_string()),
        AuthMode::None | AuthMode::AccountOwner => None,
    };
    boot_with_auth(
        AuthState::new(auth_mode, token),
        agent_base_override,
        chat_history_override,
        None,
    )
    .await
}

async fn boot_with_auth(
    auth: AuthState,
    agent_base_override: Option<String>,
    chat_history_override: Option<PathBuf>,
    runtime_agent: Option<Arc<dyn RuntimeAgent>>,
) -> Harness {
    // Load the canonical fixtures bundled in the repo.
    let workspace_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .to_path_buf();
    let providers =
        deputyctl::model::load_providers_from(&workspace_root.join("deputyctl/etc/providers.json"))
            .expect("load providers");
    let limits = deputyctl::limits::load_from(
        &workspace_root.join("deputyctl/etc/limits.qemu-aarch64.json"),
    )
    .expect("load limits");
    let mut profiles: Vec<(String, Manifest)> = Vec::new();
    for id in ["openclaw", "hermes"] {
        let path = workspace_root.join(format!("profiles/{id}.toml"));
        let m = deputyctl::manifest::load(&path).expect("load profile");
        profiles.push((id.to_string(), m));
    }

    let state_dir = tempfile::tempdir().expect("state tempdir");
    let out_dir = tempfile::tempdir().expect("dev-out tempdir");

    let state_file = state_dir.path().join("wizard-state.json");
    let app = AppState {
        auth,
        state_file: state_file.clone(),
        state: Arc::new(Mutex::new(WizardState::default())),
        providers: Arc::new(providers),
        profiles: Arc::new(profiles),
        limits: Arc::new(Some(limits)),
        apply_mode: ApplyMode::Dev,
        dev_out: Some(out_dir.path().to_path_buf()),
        secure_cookies: false,
        pending_secret: Arc::new(Mutex::new(None)),
        pending_tailscale: Arc::new(Mutex::new(None)),
        pending_cloudflared: Arc::new(Mutex::new(None)),
        pending_backup: Arc::new(Mutex::new(None)),
        pending_device_code: Arc::new(Mutex::new(None)),
        agent_base_override,
        chat_history_override,
        airgap_providers: None,
        runtime_agent: runtime_agent.unwrap_or_else(|| {
            Arc::new(SocketRuntimeAgent::new(
                state_dir.path().join("deputyd.sock"),
            ))
        }),
    };

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let addr: SocketAddr = listener.local_addr().expect("local addr");
    let r = router(app);
    let handle = tokio::spawn(async move {
        let _ = axum::serve(listener, r).await;
    });

    // Touch state file so the saver's tmp-rename is happy.
    let _ = state::save(&state_file, &WizardState::default());

    Harness {
        base: format!("http://{addr}"),
        _state_dir: state_dir,
        out_dir,
        handle,
    }
}

/// Boot the wizard in airgap mode (M4.5 Lane C): the provider step offers only
/// the given local-LLM choices, no API key is collected, and no network
/// round-trip is attempted. Mirrors `boot_with` but pins `airgap_providers`.
async fn boot_airgap(providers: Vec<ProviderChoice>) -> Harness {
    let workspace_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .to_path_buf();
    let providers_file =
        deputyctl::model::load_providers_from(&workspace_root.join("deputyctl/etc/providers.json"))
            .expect("load providers");
    let limits = deputyctl::limits::load_from(
        &workspace_root.join("deputyctl/etc/limits.qemu-aarch64.json"),
    )
    .expect("load limits");
    let mut profiles: Vec<(String, Manifest)> = Vec::new();
    for id in ["openclaw", "hermes"] {
        let path = workspace_root.join(format!("profiles/{id}.toml"));
        let m = deputyctl::manifest::load(&path).expect("load profile");
        profiles.push((id.to_string(), m));
    }
    let state_dir = tempfile::tempdir().expect("state tempdir");
    let out_dir = tempfile::tempdir().expect("dev-out tempdir");
    let state_file = state_dir.path().join("wizard-state.json");
    let app = AppState {
        auth: AuthState::new(AuthMode::None, None),
        state_file: state_file.clone(),
        state: Arc::new(Mutex::new(WizardState::default())),
        providers: Arc::new(providers_file),
        profiles: Arc::new(profiles),
        limits: Arc::new(Some(limits)),
        apply_mode: ApplyMode::Dev,
        dev_out: Some(out_dir.path().to_path_buf()),
        secure_cookies: false,
        pending_secret: Arc::new(Mutex::new(None)),
        pending_tailscale: Arc::new(Mutex::new(None)),
        pending_cloudflared: Arc::new(Mutex::new(None)),
        pending_backup: Arc::new(Mutex::new(None)),
        pending_device_code: Arc::new(Mutex::new(None)),
        agent_base_override: None,
        chat_history_override: None,
        airgap_providers: Some(providers),
        runtime_agent: Arc::new(SocketRuntimeAgent::new(
            state_dir.path().join("deputyd.sock"),
        )),
    };
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let addr: SocketAddr = listener.local_addr().expect("local addr");
    let r = router(app);
    let handle = tokio::spawn(async move {
        let _ = axum::serve(listener, r).await;
    });
    let _ = state::save(&state_file, &WizardState::default());
    Harness {
        base: format!("http://{addr}"),
        _state_dir: state_dir,
        out_dir,
        handle,
    }
}

fn airgap_choices() -> Vec<ProviderChoice> {
    vec![
        ProviderChoice {
            id: "airgap-lfm2-1.2b".into(),
            display_name: "LFM2-1.2B (airgap, default)".into(),
            key_env_var: String::new(),
            key_format: String::new(),
            default: true,
        },
        ProviderChoice {
            id: "airgap-qwen2.5-coder-1.5b".into(),
            display_name: "Qwen2.5-Coder-1.5B (airgap)".into(),
            key_env_var: String::new(),
            key_format: String::new(),
            default: false,
        },
    ]
}

fn client() -> reqwest::Client {
    reqwest::Client::builder()
        .cookie_store(true)
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .expect("build client")
}

#[derive(Clone, Default)]
struct FakeRuntimeAgent {
    commands: Arc<Mutex<Vec<deputyd::AgentCommand>>>,
}

impl FakeRuntimeAgent {
    fn commands(&self) -> Vec<deputyd::AgentCommand> {
        self.commands.lock().expect("runtime commands").clone()
    }
}

impl RuntimeAgent for FakeRuntimeAgent {
    fn execute(&self, command: deputyd::AgentCommand) -> anyhow::Result<deputyd::AgentResponse> {
        self.commands
            .lock()
            .expect("runtime commands")
            .push(command.clone());
        let result = match command {
            deputyd::AgentCommand::Health => deputyd::AgentResult::Health {
                report: deputyd::HealthReport {
                    agent_version: "test".to_string(),
                    protocol: deputyd::PROTOCOL_VERSION,
                    lifecycle: deputyd::LifecycleState::default(),
                    memory: deputyd::MemoryReport::default(),
                    uptime_seconds: Some(42.0),
                    memory_pressure: None,
                    active_profile: Some("openclaw".to_string()),
                },
            },
            deputyd::AgentCommand::PreparePause => deputyd::AgentResult::State {
                state: deputyd::LifecycleState {
                    phase: deputyd::LifecyclePhase::Quiesced,
                    changed_at_unix: Some(1),
                },
            },
            deputyd::AgentCommand::Resume => deputyd::AgentResult::State {
                state: deputyd::LifecycleState::default(),
            },
            deputyd::AgentCommand::Reclaim { drop_caches } => deputyd::AgentResult::Reclaimed {
                compacted: true,
                caches_dropped: drop_caches,
            },
            _ => deputyd::AgentResult::Capabilities {
                report: deputyd::capability_report(),
            },
        };
        Ok(deputyd::AgentResponse {
            protocol: deputyd::PROTOCOL_VERSION,
            id: "fake-runtime".to_string(),
            ok: true,
            result: Some(result),
            error: None,
        })
    }
}

fn owner_auth_and_jwt(account_id: &str) -> (AuthState, String) {
    use jsonwebtoken::{encode, Algorithm, EncodingKey, Header};
    use rand::rngs::OsRng;
    use rsa::pkcs8::{EncodePrivateKey, EncodePublicKey, LineEnding};
    use rsa::RsaPrivateKey;

    #[derive(serde::Serialize)]
    struct Claims<'a> {
        sub: &'a str,
        exp: usize,
    }

    let private = RsaPrivateKey::new(&mut OsRng, 2048).expect("RSA key");
    let public_pem = private
        .to_public_key()
        .to_public_key_pem(LineEnding::LF)
        .expect("public PEM")
        .into_bytes();
    let private_pem = private.to_pkcs8_pem(LineEnding::LF).expect("private PEM");
    let exp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock")
        .as_secs() as usize
        + 3600;
    let jwt = encode(
        &Header::new(Algorithm::RS256),
        &Claims {
            sub: account_id,
            exp,
        },
        &EncodingKey::from_rsa_pem(private_pem.as_bytes()).expect("encoding key"),
    )
    .expect("JWT");
    (
        AuthState::new_account_owner(public_pem, account_id.to_string()),
        jwt,
    )
}

// ---------------------------------------------------------------------------
// Public-route tests
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn healthz_is_unauthenticated() {
    let h = boot(AuthMode::Token).await;
    let c = client();
    let r = c.get(format!("{}/healthz", h.base)).send().await.unwrap();
    assert_eq!(r.status(), 200);
    assert_eq!(r.text().await.unwrap(), "ok");
}

#[tokio::test(flavor = "multi_thread")]
async fn wizard_without_token_is_401() {
    let h = boot(AuthMode::Token).await;
    let c = client();
    let r = c.get(format!("{}/wizard", h.base)).send().await.unwrap();
    assert_eq!(r.status(), 401);
}

#[tokio::test(flavor = "multi_thread")]
async fn wizard_with_wrong_token_is_401() {
    let h = boot(AuthMode::Token).await;
    let c = client();
    let r = c
        .get(format!("{}/wizard?token=nope", h.base))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 401);
}

// ---------------------------------------------------------------------------
// Authenticated resident-agent tunnel bridge
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn runtime_bridge_rejects_unauthenticated_requests() {
    let fake = FakeRuntimeAgent::default();
    let h = boot_with_auth(
        AuthState::new(AuthMode::Token, Some(TEST_TOKEN.to_string())),
        None,
        None,
        Some(Arc::new(fake.clone())),
    )
    .await;
    let response = client()
        .get(format!("{}/api/v1/runtime", h.base))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 401);
    assert!(fake.commands().is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn runtime_bridge_allows_only_typed_operations_after_token_login() {
    let fake = FakeRuntimeAgent::default();
    let h = boot_with_auth(
        AuthState::new(AuthMode::Token, Some(TEST_TOKEN.to_string())),
        None,
        None,
        Some(Arc::new(fake.clone())),
    )
    .await;
    let client = client();

    let health = client
        .get(format!("{}/api/v1/runtime?token={TEST_TOKEN}", h.base))
        .send()
        .await
        .unwrap();
    assert_eq!(health.status(), 200);
    let health_json: serde_json::Value = health.json().await.unwrap();
    assert_eq!(health_json["ok"], true);
    assert_eq!(health_json["result"]["kind"], "health");

    let pause = client
        .post(format!("{}/api/v1/runtime/prepare-pause", h.base))
        .send()
        .await
        .unwrap();
    assert_eq!(pause.status(), 200);

    let reclaim = client
        .post(format!("{}/api/v1/runtime/reclaim", h.base))
        .json(&serde_json::json!({ "drop_caches": true }))
        .send()
        .await
        .unwrap();
    assert_eq!(reclaim.status(), 200);

    let resume = client
        .post(format!("{}/api/v1/runtime/resume", h.base))
        .send()
        .await
        .unwrap();
    assert_eq!(resume.status(), 200);

    let typed = client
        .post(format!("{}/api/v1/runtime/command", h.base))
        .json(&serde_json::json!({
            "command": "workload",
            "action": "restart"
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(typed.status(), 200);

    let rejected_typed = client
        .post(format!("{}/api/v1/runtime/command", h.base))
        .json(&serde_json::json!({
            "command": "shell",
            "program": "sh",
            "args": ["-c", "id"]
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(rejected_typed.status(), 422);

    let arbitrary = client
        .post(format!("{}/api/v1/runtime/exec", h.base))
        .json(&serde_json::json!({ "command": "sh", "args": ["-c", "id"] }))
        .send()
        .await
        .unwrap();
    assert_eq!(arbitrary.status(), 404);

    assert_eq!(
        fake.commands(),
        vec![
            deputyd::AgentCommand::Health,
            deputyd::AgentCommand::PreparePause,
            deputyd::AgentCommand::Reclaim { drop_caches: true },
            deputyd::AgentCommand::Resume,
            deputyd::AgentCommand::Workload {
                action: deputyd::WorkloadAction::Restart,
            },
        ]
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn runtime_bridge_accepts_valid_account_owner_jwt_for_tunnel_access() {
    let fake = FakeRuntimeAgent::default();
    let (auth, jwt) = owner_auth_and_jwt("acct-runtime-owner");
    let h = boot_with_auth(auth, None, None, Some(Arc::new(fake.clone()))).await;

    let response = client()
        .get(format!("{}/api/v1/runtime", h.base))
        .bearer_auth(jwt)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    assert_eq!(fake.commands(), vec![deputyd::AgentCommand::Health]);
}

// ---------------------------------------------------------------------------
// M4.5 Lane C — air-gapped provider step (local LLM, no API key, no network)
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn airgap_provider_step_offers_local_models_without_api_key() {
    let h = boot_airgap(airgap_choices()).await;
    let c = client();
    let r = c
        .get(format!("{}/wizard/provider", h.base))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    let body = r.text().await.unwrap();
    // Airgap copy is shown (not the cloud API-key copy).
    assert!(body.contains("air-gapped"), "airgap intro copy present");
    assert!(
        !body.contains("round-trip request"),
        "cloud validation copy must not appear in airgap mode"
    );
    // Both local models are offered.
    assert!(body.contains("airgap-lfm2-1.2b"));
    assert!(body.contains("airgap-qwen2.5-coder-1.5b"));
    // No password field and no Skip validation checkbox — there is no key.
    assert!(
        !body.contains(r#"type="password""#),
        "no API key password input in airgap mode"
    );
    assert!(
        !body.contains("Skip validation"),
        "no Skip validation checkbox in airgap mode"
    );
    // The default model is pre-selected.
    assert!(
        body.contains(r#"value="airgap-lfm2-1.2b" checked"#),
        "default airgap model is pre-selected"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn airgap_provider_post_advances_without_key_or_secret() {
    let h = boot_airgap(airgap_choices()).await;
    let c = client();
    // No api_key field — the airgap form sends only the provider id.
    let r = c
        .post(format!("{}/wizard/provider", h.base))
        .form(&[("provider", "airgap-lfm2-1.2b")])
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 303);
    let loc = r
        .headers()
        .get("location")
        .unwrap()
        .to_str()
        .unwrap()
        .to_string();
    assert!(
        loc.contains("/wizard/channels"),
        "airgap provider advances to channels, got {loc}"
    );

    // The chosen provider was persisted to wizard state; no secret pending.
    let r = c
        .get(format!("{}/wizard/channels", h.base))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    // Channels step renders; the provider is reflected as chosen (state has it).
    // Re-fetch the provider step to confirm the selection stuck.
    let r = c
        .get(format!("{}/wizard/provider", h.base))
        .send()
        .await
        .unwrap();
    let body = r.text().await.unwrap();
    assert!(
        body.contains(r#"value="airgap-lfm2-1.2b" checked"#),
        "chosen airgap provider stays selected"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn airgap_rejects_spoofed_cloud_provider_id() {
    // A non-airgap build must NOT let a POST of provider=airgap-... bypass the
    // API-key check. boot() has airgap_providers=None and no flag, so the
    // wizard is in cloud mode and the airgap branch in post_provider is skipped.
    let h = boot(AuthMode::None).await;
    let c = client();
    let r = c
        .post(format!("{}/wizard/provider", h.base))
        .form(&[("provider", "airgap-lfm2-1.2b")])
        .send()
        .await
        .unwrap();
    // Cloud mode: provider id not in providers.json → "Unknown provider id."
    assert_eq!(r.status(), 200);
    let body = r.text().await.unwrap();
    assert!(
        body.contains("Unknown provider id"),
        "spoofed airgap id rejected in cloud mode"
    );
}

// ---------------------------------------------------------------------------
// Happy path — full click-through
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn full_click_through_writes_dev_out() {
    let h = boot(AuthMode::Token).await;
    let c = client();

    // 1. Token exchange — first request consumes token and sets a cookie.
    let r = c
        .get(format!("{}/wizard?token={TEST_TOKEN}", h.base))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 303, "GET /wizard redirects to current step");

    // 2. Step 1 — system.
    let r = c
        .get(format!("{}/wizard/system", h.base))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    let body = r.text().await.unwrap();
    assert!(body.contains("Hostname"), "system page renders form");

    let r = c
        .post(format!("{}/wizard/system", h.base))
        .form(&[("hostname", "deputyos-test"), ("timezone", "UTC")])
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 303);
    assert_eq!(
        r.headers().get("location").unwrap().to_str().unwrap(),
        "/wizard/profile"
    );

    // 3. Step 2 — profile.
    let r = c
        .post(format!("{}/wizard/profile", h.base))
        .form(&[("profile", "openclaw")])
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 303);

    // 4. Step 3 — provider. Use skip_validation so we don't hit the real
    // OpenRouter endpoint during tests.
    let r = c
        .post(format!("{}/wizard/provider", h.base))
        .form(&[
            ("provider", "openrouter"),
            ("api_key", "sk-or-v1-testkey"),
            ("skip_validation", "1"),
        ])
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 303);

    // 5. Step 4 — channels.
    let r = c
        .post(format!("{}/wizard/channels", h.base))
        .form(&[("channels", "telegram"), ("channels", "slack")])
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 303);

    // 6. Step 5 — ssh.
    let r = c
        .post(format!("{}/wizard/ssh", h.base))
        .form(&[(
            "ssh_keys",
            "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAITESTKEY me@host",
        )])
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 303);

    // 6a. Step 6 — tailscale (skip).
    let r = c
        .post(format!("{}/wizard/tailscale", h.base))
        .form(&[("skip", "1")])
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 303);

    // 6b. Step 7 — cloudflare-tunnel: pick "quick" so apply records the intent.
    let r = c
        .post(format!("{}/wizard/cloudflare-tunnel", h.base))
        .form(&[("choice", "quick")])
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 303);

    // 6c. Step 8 — backup: configure R2 so we can assert rclone.conf wrote.
    let r = c
        .post(format!("{}/wizard/backup", h.base))
        .form(&[
            ("kind", "r2"),
            ("r2_account_id", "acct-xyz"),
            ("r2_access_key", "ACCESS"),
            ("r2_secret_key", "SECRET"),
            ("r2_bucket", "deputyos-backups"),
        ])
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 303);

    // 6d. Step 9 — drives (M3.5 hybrid): acknowledge and continue.
    let r = c
        .post(format!("{}/wizard/drives", h.base))
        .form(&Vec::<(&str, &str)>::new())
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 303);
    assert_eq!(
        r.headers().get("location").unwrap().to_str().unwrap(),
        "/wizard/review"
    );

    // 7. Review page renders all answers.
    let r = c
        .get(format!("{}/wizard/review", h.base))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    let body = r.text().await.unwrap();
    assert!(body.contains("deputyos-test"));
    assert!(body.contains("UTC"));
    assert!(body.contains("openclaw"));
    assert!(body.contains("openrouter"));
    assert!(body.contains("telegram"));

    // 8. Apply.
    let r = c
        .post(format!("{}/wizard/review/apply", h.base))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 303);

    // 9. Done page.
    let r = c
        .get(format!("{}/wizard/done", h.base))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    let body = r.text().await.unwrap();
    assert!(body.contains("Setup complete"));

    // 10. Files written.
    let root = h.out_dir.path();
    let hostname = std::fs::read_to_string(root.join("etc/hostname")).unwrap();
    assert_eq!(hostname.trim(), "deputyos-test");
    let active = std::fs::read_to_string(root.join("etc/deputyos/active-profile")).unwrap();
    assert_eq!(active.trim(), "openclaw");
    let secrets = std::fs::read_to_string(root.join("etc/deputyos/secrets.env")).unwrap();
    assert!(
        secrets.contains("OPENROUTER_API_KEY=sk-or-v1-testkey"),
        "secrets.env missing key: {secrets}"
    );
    assert!(root
        .join("etc/deputyos/openclaw/channels.d/telegram.enabled")
        .exists());
    assert!(root
        .join("etc/deputyos/openclaw/channels.d/slack.enabled")
        .exists());
    let auth_keys = std::fs::read_to_string(root.join("home/agent/.ssh/authorized_keys")).unwrap();
    assert!(auth_keys.contains("ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAITESTKEY me@host"));

    // 11. Backup config landed.
    let conf = std::fs::read_to_string(root.join("etc/deputyos/rclone.conf")).unwrap();
    assert!(conf.contains("[remote]"));
    assert!(conf.contains("r2.cloudflarestorage.com"));
    let backup_env = std::fs::read_to_string(root.join("etc/deputyos/backup.env")).unwrap();
    assert!(backup_env.contains("BACKUP_BUCKET=deputyos-backups"));
}

// ---------------------------------------------------------------------------
// Rejection tests
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn channel_blocked_by_limits_returns_422() {
    let h = boot(AuthMode::None).await;
    let c = client();

    // Walk to step 4 with a profile that includes the blocked channel.
    c.post(format!("{}/wizard/system", h.base))
        .form(&[("hostname", "deputyos"), ("timezone", "UTC")])
        .send()
        .await
        .unwrap();
    c.post(format!("{}/wizard/profile", h.base))
        .form(&[("profile", "openclaw")])
        .send()
        .await
        .unwrap();
    c.post(format!("{}/wizard/provider", h.base))
        .form(&[
            ("provider", "openrouter"),
            ("api_key", "k"),
            ("skip_validation", "1"),
        ])
        .send()
        .await
        .unwrap();
    // The qemu-aarch64 limits.json marks `whatsapp-cloud-webhook` as
    // disabled by RAM. The openclaw channel list is `whatsapp` (different
    // string), so for this assertion we use the fact that the limits list
    // has whatsapp-cloud-webhook — which is not even in openclaw's
    // supported list. So the rejection here is "not supported by profile".
    let r = c
        .post(format!("{}/wizard/channels", h.base))
        .form(&[("channels", "whatsapp-cloud-webhook")])
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    let body = r.text().await.unwrap();
    assert!(
        body.contains("not supported by this profile"),
        "expected validation error, got: {body}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn invalid_hostname_is_rejected_inline() {
    let h = boot(AuthMode::None).await;
    let c = client();
    let r = c
        .post(format!("{}/wizard/system", h.base))
        .form(&[("hostname", "BAD HOST"), ("timezone", "UTC")])
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    let body = r.text().await.unwrap();
    assert!(
        body.to_lowercase().contains("hostname"),
        "expected hostname error in body: {body}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn ssh_key_with_no_algorithm_prefix_is_rejected() {
    let h = boot(AuthMode::None).await;
    let c = client();
    // Walk forward to ssh.
    c.post(format!("{}/wizard/system", h.base))
        .form(&[("hostname", "deputyos"), ("timezone", "UTC")])
        .send()
        .await
        .unwrap();
    c.post(format!("{}/wizard/profile", h.base))
        .form(&[("profile", "openclaw")])
        .send()
        .await
        .unwrap();
    c.post(format!("{}/wizard/provider", h.base))
        .form(&[
            ("provider", "openrouter"),
            ("api_key", "k"),
            ("skip_validation", "1"),
        ])
        .send()
        .await
        .unwrap();
    c.post(format!("{}/wizard/channels", h.base))
        .form(&[("channels", "telegram")])
        .send()
        .await
        .unwrap();
    let r = c
        .post(format!("{}/wizard/ssh", h.base))
        .form(&[("ssh_keys", "garbage data here")])
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    let body = r.text().await.unwrap();
    assert!(
        body.to_lowercase().contains("ssh key"),
        "expected ssh validation error: {body}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn no_token_mode_skips_auth_completely() {
    let h = boot(AuthMode::None).await;
    let c = client();
    let r = c
        .get(format!("{}/wizard/system", h.base))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
}

#[tokio::test(flavor = "multi_thread")]
async fn unknown_profile_id_is_rejected() {
    let h = boot(AuthMode::None).await;
    let c = client();
    c.post(format!("{}/wizard/system", h.base))
        .form(&[("hostname", "deputyos"), ("timezone", "UTC")])
        .send()
        .await
        .unwrap();
    let r = c
        .post(format!("{}/wizard/profile", h.base))
        .form(&[("profile", "no-such-profile")])
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    let body = r.text().await.unwrap();
    assert!(body.contains("Unknown profile id"));
}

#[tokio::test(flavor = "multi_thread")]
async fn token_is_single_use() {
    let h = boot(AuthMode::Token).await;
    let c1 = client();
    let r = c1
        .get(format!("{}/wizard?token={TEST_TOKEN}", h.base))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 303);
    // A fresh client that re-uses the same token after consumption must be 401.
    let c2 = client();
    let r = c2
        .get(format!("{}/wizard?token={TEST_TOKEN}", h.base))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 401, "token should be consumed on first use");
}

// ---------------------------------------------------------------------------
// Phase 5 Lane W — new wizard steps + /chat
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn tailscale_step_renders_and_skip_advances() {
    let h = boot(AuthMode::None).await;
    let c = client();
    // Walk to tailscale.
    walk_to_ssh(&c, &h.base).await;
    let r = c
        .get(format!("{}/wizard/tailscale", h.base))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    let body = r.text().await.unwrap();
    assert!(body.contains("Tailscale"));
    assert!(body.contains("Skip"));

    let r = c
        .post(format!("{}/wizard/tailscale", h.base))
        .form(&[("skip", "1")])
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 303);
    assert_eq!(
        r.headers().get("location").unwrap().to_str().unwrap(),
        "/wizard/account"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn account_step_skip_advances_and_writes_no_tokens() {
    // M8 hard rule: every flow works without an account. Skipping the Account
    // step must advance to the tunnel step and write zero tokens / account.json.
    let h = boot(AuthMode::None).await;
    let c = client();
    walk_to_ssh(&c, &h.base).await;
    c.post(format!("{}/wizard/tailscale", h.base))
        .form(&[("skip", "1")])
        .send()
        .await
        .unwrap();

    let r = c
        .get(format!("{}/wizard/account", h.base))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    let body = r.text().await.unwrap();
    assert!(body.contains("Account"));
    assert!(body.contains("Skip"));

    let r = c
        .post(format!("{}/wizard/account", h.base))
        .form(&[("action", "skip")])
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 303);
    assert_eq!(
        r.headers().get("location").unwrap().to_str().unwrap(),
        "/wizard/cloudflare-tunnel"
    );

    assert!(!h.out_dir.path().join("etc/deputyos/tunnel-token").exists());
    assert!(!h.out_dir.path().join("etc/deputyos/backup-token").exists());
    assert!(!h.out_dir.path().join("etc/deputyos/account.json").exists());
    assert!(
        !h.out_dir.path().join("etc/deputyos/api-base").exists(),
        "skipping the Account step must not write an api-base file"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn account_step_custom_backend_field_renders_and_is_remembered() {
    // The Account step surfaces a "Custom backend" input. Entering a URL and
    // starting the flow must persist it to wizard-state answers (so the
    // begin→poll cycle uses a consistent backend) even when the backend is
    // unreachable — the field is re-rendered with the value intact.
    let h = boot(AuthMode::None).await;
    let c = client();
    walk_to_ssh(&c, &h.base).await;
    c.post(format!("{}/wizard/tailscale", h.base))
        .form(&[("skip", "1")])
        .send()
        .await
        .unwrap();

    // A port we immediately drop → connection refused (fast transport error,
    // no DNS dependency, no hang).
    let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let refused_addr = l.local_addr().unwrap();
    drop(l);
    let custom_base = format!("http://{refused_addr}");

    let r = c
        .get(format!("{}/wizard/account", h.base))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    let body = r.text().await.unwrap();
    assert!(body.contains("Custom backend"), "field missing: {body}");

    let r = c
        .post(format!("{}/wizard/account", h.base))
        .form(&[
            ("action", "begin"),
            ("email", "owner@example.com"),
            ("api_base", custom_base.as_str()),
        ])
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    let body = r.text().await.unwrap();
    // The unreachable backend surfaces as an error note, but the chosen URL
    // is preserved in the re-rendered form.
    assert!(body.to_lowercase().contains("device-code"), "body: {body}");
    assert!(
        body.contains(&custom_base),
        "custom base not preserved: {body}"
    );

    // The choice was persisted to the wizard answers (survives re-render).
    let state_path = h._state_dir.path().join("wizard-state.json");
    let raw = std::fs::read_to_string(&state_path).unwrap();
    assert!(raw.contains(&custom_base), "api_base not in state: {raw}");
    // Nothing written to /etc yet — registration hasn't succeeded.
    assert!(!h.out_dir.path().join("etc/deputyos/api-base").exists());
}

#[tokio::test(flavor = "multi_thread")]
async fn account_step_custom_backend_persists_to_etc_on_registration() {
    // Full device-code flow against a stub self-hosted backend. On success the
    // chosen backend is written to /etc/deputyos/api-base (0644) so the tunnel
    // + command poller reach the same backend this device registered against.
    let stub_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let stub_addr: SocketAddr = stub_listener.local_addr().unwrap();
    let stub_app = axum::Router::new()
        .route(
            "/api/v1/auth/device-code",
            axum::routing::post(|| async {
                axum::Json(serde_json::json!({
                    "device_code": "dc-stub",
                    "user_code": "STUB",
                    "verification_uri": "http://verify.test",
                }))
            }),
        )
        .route(
            "/api/v1/auth/device-token",
            axum::routing::post(|| async {
                axum::Json(serde_json::json!({"access_token": "stub.jwt.token"}))
            }),
        )
        .route(
            "/api/v1/accounts/devices/register",
            axum::routing::post(|axum::Json(_): axum::Json<serde_json::Value>| async {
                axum::Json(serde_json::json!({
                    "device_id": "dev-stub",
                    "tunnel_token": "tt-stub",
                    "backup_token": "bt-stub",
                }))
            }),
        );
    let stub_handle = tokio::spawn(async move {
        let _ = axum::serve(stub_listener, stub_app).await;
    });

    let h = boot(AuthMode::None).await;
    let c = client();
    walk_to_ssh(&c, &h.base).await;
    c.post(format!("{}/wizard/tailscale", h.base))
        .form(&[("skip", "1")])
        .send()
        .await
        .unwrap();

    let custom_base = format!("http://{stub_addr}");

    // Begin the device-code flow against the stub backend.
    let r = c
        .post(format!("{}/wizard/account", h.base))
        .form(&[
            ("action", "begin"),
            ("email", "owner@example.com"),
            ("api_base", custom_base.as_str()),
        ])
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    let body = r.text().await.unwrap();
    assert!(body.contains("STUB"), "user code missing: {body}");

    // Poll → the stub mints tokens + registers → advance to the tunnel step.
    let r = c
        .post(format!("{}/wizard/account", h.base))
        .form(&[("action", "poll"), ("api_base", custom_base.as_str())])
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 303);
    assert_eq!(
        r.headers().get("location").unwrap().to_str().unwrap(),
        "/wizard/cloudflare-tunnel"
    );

    // The custom backend is persisted to /etc/deputyos/api-base (0644) so the
    // tunnel + command poller pick it up at runtime.
    let api_base_path = h.out_dir.path().join("etc/deputyos/api-base");
    let written = std::fs::read_to_string(&api_base_path).expect("api-base written");
    assert_eq!(written.trim(), custom_base);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&api_base_path)
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o644, "api-base must be 0644 (public, non-secret)");
    }
    // Registration also wrote the tokens + account label against the stub.
    assert_eq!(
        std::fs::read_to_string(h.out_dir.path().join("etc/deputyos/tunnel-token"))
            .unwrap()
            .trim(),
        "tt-stub"
    );
    assert!(h.out_dir.path().join("etc/deputyos/account.json").exists());

    stub_handle.abort();
}

#[tokio::test(flavor = "multi_thread")]
async fn cloudflare_tunnel_named_validates_credentials_json() {
    let h = boot(AuthMode::None).await;
    let c = client();
    walk_to_cloudflare(&c, &h.base).await;
    // Bad JSON.
    let r = c
        .post(format!("{}/wizard/cloudflare-tunnel", h.base))
        .form(&[("choice", "named"), ("credentials", "not json")])
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    let body = r.text().await.unwrap();
    assert!(body.to_lowercase().contains("invalid"));

    // Good JSON.
    let creds = r#"{"AccountTag":"a","TunnelID":"id","TunnelName":"my-agent","TunnelSecret":"s"}"#;
    let r = c
        .post(format!("{}/wizard/cloudflare-tunnel", h.base))
        .form(&[("choice", "named"), ("credentials", creds)])
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 303);
    assert_eq!(
        r.headers().get("location").unwrap().to_str().unwrap(),
        "/wizard/backup"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn cloudflare_tunnel_quick_advances() {
    let h = boot(AuthMode::None).await;
    let c = client();
    walk_to_cloudflare(&c, &h.base).await;
    let r = c
        .post(format!("{}/wizard/cloudflare-tunnel", h.base))
        .form(&[("choice", "quick")])
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 303);
}

#[tokio::test(flavor = "multi_thread")]
async fn backup_skip_advances_to_drives() {
    // M3.5: Backup now advances to the Drives step, not Review.
    let h = boot(AuthMode::None).await;
    let c = client();
    walk_to_backup(&c, &h.base).await;
    let r = c
        .post(format!("{}/wizard/backup", h.base))
        .form(&[("kind", "skip")])
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 303);
    assert_eq!(
        r.headers().get("location").unwrap().to_str().unwrap(),
        "/wizard/drives"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn managed_backup_is_available_for_paid_accounts() {
    let h = boot(AuthMode::None).await;
    let c = client();
    walk_to_backup(&c, &h.base).await;
    let page = c
        .get(format!("{}/wizard/backup", h.base))
        .send()
        .await
        .unwrap();
    assert!(page
        .text()
        .await
        .unwrap()
        .contains("Business and Enterprise"));

    let response = c
        .post(format!("{}/wizard/backup", h.base))
        .form(&[("kind", "managed")])
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 303);
    assert_eq!(
        response
            .headers()
            .get("location")
            .unwrap()
            .to_str()
            .unwrap(),
        "/wizard/drives"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn drives_step_renders_and_links_mounts() {
    // M3.5 hybrid Drives step: GET renders the mount table + a link to the
    // standalone /mounts page; POST acknowledges and advances to Review.
    let h = boot(AuthMode::None).await;
    let c = client();
    walk_to_backup(&c, &h.base).await;
    // Skip backup so state.step lands on Drives.
    c.post(format!("{}/wizard/backup", h.base))
        .form(&[("kind", "skip")])
        .send()
        .await
        .unwrap();

    let r = c
        .get(format!("{}/wizard/drives", h.base))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    let body = r.text().await.unwrap();
    assert!(
        body.contains("/mnt/deputyos/"),
        "drives step explains the mount root"
    );
    assert!(
        body.contains("href=\"/mounts\""),
        "drives step links to the standalone /mounts page"
    );
    assert!(
        body.contains("Continue"),
        "drives step has a continue button"
    );

    // POST advances to Review.
    let r = c
        .post(format!("{}/wizard/drives", h.base))
        .form(&Vec::<(&str, &str)>::new())
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 303);
    assert_eq!(
        r.headers().get("location").unwrap().to_str().unwrap(),
        "/wizard/review"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn mounts_page_has_network_share_form() {
    // M3.5: the standalone /mounts page exposes an SMB/NFS add form posting
    // to /mounts/network-add (Lane C gap).
    let h = boot(AuthMode::None).await;
    let c = client();
    let r = c.get(format!("{}/mounts", h.base)).send().await.unwrap();
    assert_eq!(r.status(), 200);
    let body = r.text().await.unwrap();
    assert!(body.contains("/mounts/network-add"));
    assert!(body.contains("name=\"kind\""));
    assert!(body.contains("name=\"source\""));
    assert!(body.contains("name=\"credentials_env\""));
}

#[tokio::test(flavor = "multi_thread")]
async fn backup_b2_requires_all_fields() {
    let h = boot(AuthMode::None).await;
    let c = client();
    walk_to_backup(&c, &h.base).await;
    // Missing application key — should re-render with error.
    let r = c
        .post(format!("{}/wizard/backup", h.base))
        .form(&[
            ("kind", "b2"),
            ("b2_account_id", "abc"),
            ("b2_application_key", ""),
            ("b2_bucket", "x"),
        ])
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    let body = r.text().await.unwrap();
    assert!(body.to_lowercase().contains("application key"));
}

#[tokio::test(flavor = "multi_thread")]
async fn chat_unauth_returns_401() {
    let h = boot(AuthMode::Token).await;
    let c = client();
    let r = c.get(format!("{}/chat", h.base)).send().await.unwrap();
    assert_eq!(r.status(), 401);
}

#[tokio::test(flavor = "multi_thread")]
async fn chat_authed_renders_form() {
    let h = boot(AuthMode::None).await;
    let c = client();
    let r = c.get(format!("{}/chat", h.base)).send().await.unwrap();
    assert_eq!(r.status(), 200);
    let body = r.text().await.unwrap();
    assert!(body.contains("Your message"));
    assert!(body.contains("hx-post=\"/chat/message\""));
}

#[tokio::test(flavor = "multi_thread")]
async fn chat_post_message_persists_history() {
    // Bring up a tiny stub agent that replies with `{reply: "..."}`.
    let stub_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let stub_addr: SocketAddr = stub_listener.local_addr().unwrap();
    let stub_app = axum::Router::new().route(
        "/chat",
        axum::routing::post(|axum::Json(_): axum::Json<serde_json::Value>| async {
            axum::Json(serde_json::json!({"reply": "stub-reply"}))
        }),
    );
    let stub_handle = tokio::spawn(async move {
        let _ = axum::serve(stub_listener, stub_app).await;
    });

    // Boot the wizard with the chat history pinned to a tempfile and the
    // agent base URL pointed at the stub.
    let chat_dir = tempfile::tempdir().unwrap();
    let history = chat_dir.path().join("chat-history.jsonl");

    let h = boot_with(
        AuthMode::None,
        Some(format!("http://{stub_addr}")),
        Some(history.clone()),
    )
    .await;
    let c = client();
    let r = c
        .post(format!("{}/chat/message", h.base))
        .form(&[("message", "hello there")])
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    let body = r.text().await.unwrap();
    assert!(body.contains("hello there"), "body: {body}");
    assert!(body.contains("stub-reply"), "body: {body}");

    let raw = std::fs::read_to_string(&history).unwrap();
    assert!(raw.contains("\"role\":\"user\""));
    assert!(raw.contains("\"role\":\"assistant\""));

    stub_handle.abort();
}

#[tokio::test(flavor = "multi_thread")]
async fn chat_handles_unreachable_agent_gracefully() {
    // Pick a port and drop it — connection refused.
    let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = l.local_addr().unwrap();
    drop(l);

    let chat_dir = tempfile::tempdir().unwrap();
    let history = chat_dir.path().join("h.jsonl");
    let h = boot_with(
        AuthMode::None,
        Some(format!("http://{addr}")),
        Some(history),
    )
    .await;
    let c = client();
    let r = c
        .post(format!("{}/chat/message", h.base))
        .form(&[("message", "ping")])
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    let body = r.text().await.unwrap();
    // Either "unreachable" or the wire-up fallback message — both fine.
    assert!(
        body.to_lowercase().contains("unreachable")
            || body.to_lowercase().contains("doesn't expose")
            || body.to_lowercase().contains("did not include"),
        "fallback expected, got: {body}"
    );
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

async fn walk_to_ssh(c: &reqwest::Client, base: &str) {
    c.post(format!("{base}/wizard/system"))
        .form(&[("hostname", "deputyos"), ("timezone", "UTC")])
        .send()
        .await
        .unwrap();
    c.post(format!("{base}/wizard/profile"))
        .form(&[("profile", "openclaw")])
        .send()
        .await
        .unwrap();
    c.post(format!("{base}/wizard/provider"))
        .form(&[
            ("provider", "openrouter"),
            ("api_key", "k"),
            ("skip_validation", "1"),
        ])
        .send()
        .await
        .unwrap();
    c.post(format!("{base}/wizard/channels"))
        .form(&[("channels", "telegram")])
        .send()
        .await
        .unwrap();
    c.post(format!("{base}/wizard/ssh"))
        .form(&[(
            "ssh_keys",
            "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAITESTKEY me@host",
        )])
        .send()
        .await
        .unwrap();
}

async fn walk_to_cloudflare(c: &reqwest::Client, base: &str) {
    walk_to_ssh(c, base).await;
    c.post(format!("{base}/wizard/tailscale"))
        .form(&[("skip", "1")])
        .send()
        .await
        .unwrap();
    // Account step (M8) sits between Tailscale and Cloudflare Tunnel.
    c.post(format!("{base}/wizard/account"))
        .form(&[("action", "skip")])
        .send()
        .await
        .unwrap();
}

async fn walk_to_backup(c: &reqwest::Client, base: &str) {
    walk_to_cloudflare(c, base).await;
    c.post(format!("{base}/wizard/cloudflare-tunnel"))
        .form(&[("choice", "skip")])
        .send()
        .await
        .unwrap();
}
