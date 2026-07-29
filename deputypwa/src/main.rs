//! `deputypwa` binary entry point.
//!
//! `deputypwa serve` starts the always-on Axum web server. Production bakes
//! invoke this from `deputypwa.service` (see `contrib/deputypwa.service`); for
//! local dev `make pwa` runs it with the dev-stub flag set.

use std::net::{IpAddr, SocketAddr};
use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};

use deputypwa::push;
use deputypwa::routes::{router, AppState};

#[derive(Debug, Parser)]
#[command(
    name = "deputypwa",
    version,
    about = "deputyOS always-on companion PWA"
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
        /// TCP port to listen on (default: 8089, distinct from deputywizard's 8088).
        #[arg(long, default_value_t = 8089)]
        port: u16,
        /// Address to bind to. Default 127.0.0.1 for dev safety; production
        /// bakes use 0.0.0.0 (LAN trust).
        #[arg(long, default_value = "127.0.0.1")]
        bind: String,
        /// Path to the VAPID PEM keypair. If unset, defaults to
        /// `<data_dir>/vapid.pem`. If missing on disk we attempt to generate
        /// one via `openssl ecparam`; if openssl isn't available we run in
        /// "push disabled" mode and log a warning.
        #[arg(long)]
        vapid_keys_path: Option<PathBuf>,
    },
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    init_tracing(cli.verbose);
    match cli.command {
        Cmd::Serve {
            port,
            bind,
            vapid_keys_path,
        } => {
            let runtime = tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()
                .context("creating tokio runtime")?;
            runtime.block_on(serve(port, bind, vapid_keys_path))
        }
    }
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

async fn serve(port: u16, bind: String, vapid_keys_path: Option<PathBuf>) -> Result<()> {
    let vapid_path = vapid_keys_path.unwrap_or_else(push::default_vapid_path);
    let vapid = match push::load_or_generate_vapid(&vapid_path) {
        Ok(Some(kp)) => Some(kp.public_b64url),
        Ok(None) => None,
        Err(e) => {
            tracing::warn!(error = %e, "failed to load/generate VAPID keypair; push disabled");
            None
        }
    };

    let state = AppState::new().with_vapid(vapid);

    let bind_ip: IpAddr = bind.parse().context("parsing --bind address")?;
    let addr = SocketAddr::new(bind_ip, port);
    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .with_context(|| format!("binding {addr}"))?;
    let local = listener.local_addr().unwrap_or(addr);
    eprintln!("deputypwa: listening on http://{local}");
    eprintln!(
        "deputypwa: open http://{}:{}/app/dashboard",
        local.ip(),
        local.port()
    );

    let router = router(state);
    axum::serve(listener, router)
        .with_graceful_shutdown(shutdown_signal())
        .await
        .context("serving pwa")
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
