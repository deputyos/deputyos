//! `deputywizard` binary entry point.
//!
//! `deputywizard serve` brings up the Axum web wizard described in
//! [`deputywizard`]'s crate-level docs. `deputyctl init` shells out to this
//! binary; production bakes will run it from systemd.
//!
//! TODO(M3-rest): bake `deputywizard` into the Lane B Ansible role and have
//! systemd start it on first boot. See module-level docs in `lib.rs`.

use std::net::{IpAddr, SocketAddr};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};

use deputyctl::{limits, model, profile};

use deputywizard::apply::ApplyMode;
use deputywizard::auth::{AuthMode, AuthState};
use deputywizard::routes::{router, AppState};
use deputywizard::runtime_bridge::SocketRuntimeAgent;
use deputywizard::state;

#[derive(Debug, Parser)]
#[command(
    name = "deputywizard",
    version,
    about = "deputyOS first-boot web wizard"
)]
struct Cli {
    #[arg(short, long, action = clap::ArgAction::Count, global = true)]
    verbose: u8,

    #[command(subcommand)]
    command: Cmd,
}

#[derive(Debug, Subcommand)]
enum Cmd {
    /// Run the HTTP server.
    Serve {
        /// TCP port to listen on (default: 8088, per docs/01).
        #[arg(long, default_value_t = 8088)]
        port: u16,
        /// Address to bind to. Default 127.0.0.1 for dev safety; production
        /// bakes use 0.0.0.0.
        #[arg(long, default_value = "127.0.0.1")]
        bind: String,
        /// Pre-shared single-use token. If unset, a random one is generated.
        #[arg(long)]
        token: Option<String>,
        /// Disable the auth gate entirely. Used by `make wizard` and tests.
        #[arg(long)]
        no_token: bool,
        /// AccountOwner auth: accept the account owner's JWT (validated against
        /// /etc/deputyos/api-pubkey.pem, `sub` matched to /etc/deputyos/account.json)
        /// for remote management via the tunnel. Falls back to Token mode if
        /// either file is missing (e.g. before first-boot registration).
        #[arg(long)]
        account_owner: bool,
        /// Override the wizard state file path.
        #[arg(long)]
        state_file: Option<PathBuf>,
        /// Force production-mode apply behaviour (write to /etc/, run
        /// hostnamectl/timedatectl/systemctl). Default is auto-detected.
        #[arg(long)]
        production: bool,
    },
    /// Print an ASCII QR code for the wizard launch URL.
    ///
    /// Used at first boot by a systemd unit that pipes the output to
    /// `/dev/tty1` so the operator can scan it from a phone. Resolution:
    /// `--url` if set; else `http://<host>:8088/wizard?token=<token>`
    /// where `<token>` comes from `--token` or `/run/deputyos/wizard.token`,
    /// and `<host>` is `--host` or `deputyos.local`.
    PrintQr {
        /// Override the URL to encode. Bypasses host/token resolution.
        #[arg(long)]
        url: Option<String>,
        /// Token to embed. Default: read from `/run/deputyos/wizard.token`.
        #[arg(long)]
        token: Option<String>,
        /// Hostname to embed. Default: `deputyos.local`.
        #[arg(long, default_value = "deputyos.local")]
        host: String,
        /// Port to embed. Default: 8088.
        #[arg(long, default_value_t = 8088)]
        port: u16,
        /// Also write the URL (plaintext) to this path.
        #[arg(long)]
        url_file: Option<PathBuf>,
    },
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    init_tracing(cli.verbose);
    match cli.command {
        Cmd::Serve {
            port,
            bind,
            token,
            no_token,
            account_owner,
            state_file,
            production,
        } => {
            let runtime = tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()
                .context("creating tokio runtime")?;
            runtime.block_on(serve(ServeOpts {
                port,
                bind,
                token,
                no_token,
                account_owner,
                state_file,
                production,
            }))
        }
        Cmd::PrintQr {
            url,
            token,
            host,
            port,
            url_file,
        } => print_qr(url, token, host, port, url_file),
    }
}

fn print_qr(
    url: Option<String>,
    token: Option<String>,
    host: String,
    port: u16,
    url_file: Option<PathBuf>,
) -> Result<()> {
    let final_url = url.unwrap_or_else(|| {
        let token = token.unwrap_or_else(|| {
            std::fs::read_to_string("/run/deputyos/wizard.token")
                .ok()
                .map(|s| s.trim().to_string())
                .unwrap_or_default()
        });
        if token.is_empty() {
            format!("http://{host}:{port}/wizard")
        } else {
            format!("http://{host}:{port}/wizard?token={token}")
        }
    });

    let rendered = deputywizard::qr::render_url(&final_url)
        .map_err(|e| anyhow::anyhow!("rendering QR: {e}"))?;
    println!("{rendered}");
    println!("URL: {final_url}");

    let target_url_file = url_file.unwrap_or_else(|| PathBuf::from("/run/deputyos/wizard.url"));
    if let Some(parent) = target_url_file.parent() {
        if !parent.as_os_str().is_empty() {
            let _ = std::fs::create_dir_all(parent);
        }
    }
    if let Err(e) = std::fs::write(&target_url_file, format!("{final_url}\n")) {
        tracing::debug!(error = %e, "could not write wizard.url (best-effort)");
    }
    Ok(())
}

fn init_tracing(verbose: u8) {
    let default = match verbose {
        0 => "info",
        1 => "debug",
        _ => "trace",
    };
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new(default));
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(false)
        .without_time()
        .with_writer(std::io::stderr)
        .init();
}

struct ServeOpts {
    port: u16,
    bind: String,
    token: Option<String>,
    no_token: bool,
    account_owner: bool,
    state_file: Option<PathBuf>,
    production: bool,
}

async fn serve(opts: ServeOpts) -> Result<()> {
    let state_file = opts.state_file.unwrap_or_else(state::state_path);
    let wizard_state = state::load_or_new(&state_file)?;

    let providers = model::load_providers().context("loading providers catalogue")?;
    let installed = profile::list().unwrap_or_default();
    let mut profiles = Vec::new();
    for ip in installed {
        match deputyctl::manifest::load(&ip.manifest_path) {
            Ok(m) => profiles.push((ip.id, m)),
            Err(e) => {
                tracing::warn!(error = %e, "skipping profile that failed to load");
            }
        }
    }

    let limits_data = match limits::load() {
        Ok(l) => Some(l),
        Err(e) => {
            tracing::warn!(error = %e, "limits.json not found; sidebar will be sparse");
            None
        }
    };

    // Auth mode selection. `token_value` is the launch token shown in the
    // banner/QR (None for None/AccountOwner modes — those don't use a launch
    // token). `auth` is the constructed AuthState.
    let (auth, token_value) = if opts.no_token {
        (AuthState::new(AuthMode::None, None), None)
    } else if opts.account_owner {
        match load_account_owner_config() {
            Ok((pubkey_pem, account_id)) => {
                eprintln!("deputywizard: AccountOwner auth for account {account_id}");
                (AuthState::new_account_owner(pubkey_pem, account_id), None)
            }
            Err(e) => {
                // First-boot not complete (no account.json) or pubkey missing —
                // fall back to Token mode so the wizard is still reachable
                // locally for first-boot setup.
                tracing::warn!(
                    error = %e,
                    "AccountOwner mode unavailable; falling back to Token mode"
                );
                let t = opts
                    .token
                    .unwrap_or_else(|| deputywizard::auth::random_hex(32));
                eprintln!("deputywizard: auth token = {t}");
                write_token_file(&t);
                (AuthState::new(AuthMode::Token, Some(t.clone())), Some(t))
            }
        }
    } else {
        let t = opts
            .token
            .unwrap_or_else(|| deputywizard::auth::random_hex(32));
        eprintln!("deputywizard: auth token = {t}");
        write_token_file(&t);
        (AuthState::new(AuthMode::Token, Some(t.clone())), Some(t))
    };

    let apply_mode = if opts.production {
        ApplyMode::Production
    } else {
        ApplyMode::detect()
    };
    let secure_cookies = apply_mode == ApplyMode::Production;

    let app = AppState {
        auth,
        state_file,
        state: Arc::new(Mutex::new(wizard_state)),
        providers: Arc::new(providers),
        profiles: Arc::new(profiles),
        limits: Arc::new(limits_data),
        apply_mode,
        dev_out: None,
        secure_cookies,
        pending_secret: Arc::new(Mutex::new(None)),
        pending_tailscale: Arc::new(Mutex::new(None)),
        pending_cloudflared: Arc::new(Mutex::new(None)),
        pending_backup: Arc::new(Mutex::new(None)),
        pending_device_code: Arc::new(Mutex::new(None)),
        agent_base_override: None,
        chat_history_override: None,
        airgap_providers: None,
        runtime_agent: Arc::new(SocketRuntimeAgent::default()),
    };

    let bind_ip: IpAddr = opts.bind.parse().context("parsing --bind address")?;
    let addr = SocketAddr::new(bind_ip, opts.port);
    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .with_context(|| format!("binding {addr}"))?;
    let local = listener.local_addr().unwrap_or(addr);
    let path_q = match token_value.as_deref() {
        Some(t) => format!("/wizard?token={t}"),
        None => "/wizard".to_string(),
    };
    eprintln!("deputywizard: listening on http://{local}");
    eprintln!(
        "deputywizard: open http://{}:{}{}",
        local.ip(),
        local.port(),
        path_q
    );
    eprintln!(
        "deputywizard: apply_mode={} (state file: {})",
        match apply_mode {
            ApplyMode::Production => "production",
            ApplyMode::Dev => "dev",
        },
        deputyctl_path_display(&app.state_file)
    );

    let router = router(app);
    axum::serve(listener, router)
        .with_graceful_shutdown(shutdown_signal())
        .await
        .context("serving wizard")
}

async fn shutdown_signal() {
    let ctrl_c = async {
        let _ = tokio::signal::ctrl_c().await;
    };
    #[cfg(unix)]
    let term = async {
        if let Ok(mut s) = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
        {
            s.recv().await;
        }
    };
    #[cfg(not(unix))]
    let term = std::future::pending::<()>();
    tokio::select! {
        _ = ctrl_c => {},
        _ = term => {},
    }
}

/// Write the launch token to `/run/deputyos/wizard.token` (mode 0600) so the
/// QR-code printer (M3-rest) can read it. Best-effort: if the directory
/// can't be created or we lack permission, we just skip.
fn write_token_file(token: &str) {
    let path = std::env::var("DEPUTYWIZARD_TOKEN_FILE")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("/run/deputyos/wizard.token"));
    if let Some(parent) = path.parent() {
        if std::fs::create_dir_all(parent).is_err() {
            tracing::debug!(path = %path.display(), "skipping token file (no perms)");
            return;
        }
    }
    if std::fs::write(&path, format!("{token}\n")).is_err() {
        tracing::debug!(path = %path.display(), "skipping token file (write failed)");
        return;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Ok(meta) = std::fs::metadata(&path) {
            let mut perms = meta.permissions();
            perms.set_mode(0o600);
            let _ = std::fs::set_permissions(&path, perms);
        }
    }
}

fn deputyctl_path_display(p: &std::path::Path) -> String {
    p.display().to_string()
}

/// Load the AccountOwner auth config: the API's RSA256 public key (embedded at
/// image build under `/etc/deputyos/api-pubkey.pem`) and this device's account
/// id (the `account_id` field written to `/etc/deputyos/account.json` at
/// first-boot registration). Errors if either file is missing or account.json
/// has no `account_id` (device not yet registered) — the caller falls back to
/// Token mode in that case.
fn load_account_owner_config() -> Result<(Vec<u8>, String)> {
    let pubkey_pem = std::fs::read("/etc/deputyos/api-pubkey.pem")
        .context("reading /etc/deputyos/api-pubkey.pem")?;
    let account_json = std::fs::read_to_string("/etc/deputyos/account.json")
        .context("reading /etc/deputyos/account.json")?;
    #[derive(serde::Deserialize)]
    struct AccountLabel {
        #[serde(default)]
        account_id: Option<String>,
    }
    let label: AccountLabel =
        serde_json::from_str(&account_json).context("parsing /etc/deputyos/account.json")?;
    let account_id = label.account_id.ok_or_else(|| {
        anyhow::anyhow!("account.json has no account_id (device not registered yet)")
    })?;
    Ok((pubkey_pem, account_id))
}
