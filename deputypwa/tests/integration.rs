#![allow(clippy::unwrap_used)]
//! Integration tests for the deputypwa router.
//!
//! Boots the Axum app on a random port, exercises every public route,
//! then asserts on the response shape. We force `DEPUTYPWA_DEV_STUB=1` so
//! the data layer returns synthetic fixtures rather than shelling out to
//! `deputyctl` (which won't exist on a test runner without the workspace
//! release build).

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};

use deputypwa::routes::{router, AppState};

/// Tests that mutate process-wide env vars must serialize.
fn env_lock() -> &'static Mutex<()> {
    static M: OnceLock<Mutex<()>> = OnceLock::new();
    M.get_or_init(|| Mutex::new(()))
}

struct Harness {
    base: String,
    _data_dir: tempfile::TempDir,
    handle: tokio::task::JoinHandle<()>,
}

impl Drop for Harness {
    fn drop(&mut self) {
        self.handle.abort();
    }
}

async fn boot() -> Harness {
    boot_with_subscriptions(None).await
}

async fn boot_with_subscriptions(path: Option<PathBuf>) -> Harness {
    let _guard = env_lock().lock().unwrap();
    std::env::set_var("DEPUTYPWA_DEV_STUB", "1");
    let data_dir = tempfile::tempdir().unwrap();
    std::env::set_var("DEPUTYPWA_DATA_DIR", data_dir.path());
    drop(_guard);

    let mut state = AppState::new().with_vapid(Some("BPP_test_public_key".to_string()));
    if let Some(p) = path {
        state = state.with_subscriptions_path(p);
    }
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr: SocketAddr = listener.local_addr().unwrap();
    let r = router(state);
    let handle = tokio::spawn(async move {
        let _ = axum::serve(listener, r).await;
    });

    Harness {
        base: format!("http://{addr}"),
        _data_dir: data_dir,
        handle,
    }
}

fn client() -> reqwest::Client {
    reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap()
}

#[tokio::test(flavor = "multi_thread")]
async fn healthz_is_ok() {
    let h = boot().await;
    let r = client()
        .get(format!("{}/healthz", h.base))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    assert_eq!(r.text().await.unwrap(), "ok");
}

#[tokio::test(flavor = "multi_thread")]
async fn root_redirects_to_dashboard() {
    let h = boot().await;
    let r = client().get(format!("{}/", h.base)).send().await.unwrap();
    assert!(r.status().is_redirection());
    assert_eq!(r.headers().get("location").unwrap(), "/app/dashboard");
}

#[tokio::test(flavor = "multi_thread")]
async fn dashboard_renders_cards() {
    let h = boot().await;
    let r = client()
        .get(format!("{}/app/dashboard", h.base))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    let body = r.text().await.unwrap();
    // Card titles must all be present.
    assert!(body.contains("Status"));
    assert!(body.contains("Your device"));
    assert!(body.contains("Cost"));
    assert!(body.contains("Doctor"));
    // Stub banner must be visible since we're in dev-stub mode.
    assert!(body.contains("Dev-stub data"));
    // Manifest + service-worker registration are wired.
    assert!(body.contains(r#"href="/manifest.webmanifest""#));
    assert!(body.contains("serviceWorker.register"));
    // Stub data check: the synthetic profile id surfaces.
    assert!(body.contains("openclaw"));
}

#[tokio::test(flavor = "multi_thread")]
async fn logs_renders_with_auto_refresh() {
    let h = boot().await;
    let r = client()
        .get(format!("{}/app/logs", h.base))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    let body = r.text().await.unwrap();
    assert!(body.contains("Logs"));
    // Auto-refresh meta must be present.
    assert!(body.contains(r#"http-equiv="refresh""#));
    // Synthetic journal lines from the stub:
    assert!(body.contains("openclaw"));
}

#[tokio::test(flavor = "multi_thread")]
async fn keys_page_lists_providers() {
    let h = boot().await;
    let r = client()
        .get(format!("{}/app/keys", h.base))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    let body = r.text().await.unwrap();
    assert!(body.contains("Provider keys"));
    assert!(body.contains("Anthropic"));
    assert!(body.contains("OpenAI"));
    assert!(body.contains("Rotate a key"));
}

#[tokio::test(flavor = "multi_thread")]
async fn keys_rotate_in_dev_stub_flashes_message() {
    let h = boot().await;
    let r = client()
        .post(format!("{}/app/keys/rotate", h.base))
        .form(&[("provider", "anthropic"), ("api_key", "sk-ant-test1234")])
        .send()
        .await
        .unwrap();
    assert!(r.status().is_redirection());
    // Follow the redirect manually so the flash is consumed.
    let r2 = client()
        .get(format!("{}/app/keys", h.base))
        .send()
        .await
        .unwrap();
    let body = r2.text().await.unwrap();
    assert!(
        body.contains("dev-stub") || body.contains("Rotated") || body.contains("would rotate"),
        "expected flash, got: {body}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn keys_rotate_rejects_empty_inputs() {
    let h = boot().await;
    let r = client()
        .post(format!("{}/app/keys/rotate", h.base))
        .form(&[("provider", ""), ("api_key", "")])
        .send()
        .await
        .unwrap();
    assert!(r.status().is_redirection());
}

#[tokio::test(flavor = "multi_thread")]
async fn mounts_page_renders_in_dev_stub() {
    // M3.5: /app/mounts goes through data::fetch_mounts, so in dev-stub mode
    // it renders the stub entries + a revoke form per row.
    let h = boot().await;
    let r = client()
        .get(format!("{}/app/mounts", h.base))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    let body = r.text().await.unwrap();
    assert!(body.contains("Mounts"));
    // Stub fixture entries surface.
    assert!(body.contains("documents"));
    assert!(body.contains("/mnt/deputyos/documents"));
    assert!(body.contains("nas-photos"));
    // Revoke form posts to the remove route.
    assert!(body.contains(r#"action="/app/mounts/remove""#));
    assert!(body.contains("Revoke"));
}

#[tokio::test(flavor = "multi_thread")]
async fn mounts_remove_redirects_to_mounts() {
    // M3.5: revoking a mount redirects to /app/mounts and sets a flash that
    // the next GET renders. In dev-stub the policy file isn't real, so the
    // remove errors out — but the handler still flashes + redirects, which
    // is what the PWA contract guarantees.
    let h = boot().await;
    let r = client()
        .post(format!("{}/app/mounts/remove", h.base))
        .form(&[("id", "documents")])
        .send()
        .await
        .unwrap();
    assert!(r.status().is_redirection());
    assert_eq!(
        r.headers().get("location").unwrap().to_str().unwrap(),
        "/app/mounts"
    );
    // Follow up: the flash banner is rendered.
    let r2 = client()
        .get(format!("{}/app/mounts", h.base))
        .send()
        .await
        .unwrap();
    assert_eq!(r2.status(), 200);
    let body = r2.text().await.unwrap();
    assert!(
        body.contains(r#"class="banner""#),
        "expected flash banner, got: {body}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn manifest_is_served_with_correct_type() {
    let h = boot().await;
    let r = client()
        .get(format!("{}/manifest.webmanifest", h.base))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    assert_eq!(
        r.headers().get("content-type").unwrap(),
        "application/manifest+json"
    );
    let v: serde_json::Value = r.json().await.unwrap();
    assert_eq!(v["start_url"], "/app/dashboard");
    assert_eq!(v["display"], "standalone");
}

#[tokio::test(flavor = "multi_thread")]
async fn service_worker_is_served_as_javascript() {
    let h = boot().await;
    let r = client()
        .get(format!("{}/sw.js", h.base))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    assert_eq!(r.headers().get("content-type").unwrap(), "text/javascript");
    let body = r.text().await.unwrap();
    assert!(body.contains("addEventListener('push'"));
}

#[tokio::test(flavor = "multi_thread")]
async fn vapid_public_returns_configured_key() {
    let h = boot().await;
    let r = client()
        .get(format!("{}/app/push/vapid-public", h.base))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    let body = r.text().await.unwrap();
    assert_eq!(body, "BPP_test_public_key");
}

#[tokio::test(flavor = "multi_thread")]
async fn push_subscribe_appends_to_jsonl() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("subs.jsonl");
    let h = boot_with_subscriptions(Some(path.clone())).await;

    let payload = serde_json::json!({
        "endpoint": "https://push.example.invalid/abc",
        "keys": {"p256dh": "BPP_p", "auth": "AAA"}
    });
    let r = client()
        .post(format!("{}/app/push/subscribe", h.base))
        .json(&payload)
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 201);

    let raw = std::fs::read_to_string(&path).unwrap();
    assert_eq!(raw.lines().count(), 1);
    let parsed: serde_json::Value = serde_json::from_str(raw.trim()).unwrap();
    assert_eq!(parsed["endpoint"], "https://push.example.invalid/abc");
}

#[tokio::test(flavor = "multi_thread")]
async fn push_subscribe_rejects_malformed_json() {
    let h = boot().await;
    let r = client()
        .post(format!("{}/app/push/subscribe", h.base))
        .header("content-type", "application/json")
        .body("not json")
        .send()
        .await
        .unwrap();
    // axum returns 400 on bad JSON for `Json<T>` extractors.
    assert!(r.status().is_client_error());
}

#[tokio::test(flavor = "multi_thread")]
async fn icon_svg_is_served() {
    let h = boot().await;
    let r = client()
        .get(format!("{}/static/icon.svg", h.base))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    assert_eq!(r.headers().get("content-type").unwrap(), "image/svg+xml");
    let body = r.text().await.unwrap();
    assert!(body.starts_with("<svg"));
}

#[tokio::test(flavor = "multi_thread")]
async fn style_css_is_served() {
    let h = boot().await;
    let r = client()
        .get(format!("{}/static/style.css", h.base))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    let ct = r.headers().get("content-type").unwrap().to_str().unwrap();
    assert!(ct.starts_with("text/css"));
}
