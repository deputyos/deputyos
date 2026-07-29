//! `deputyctl tunnel` — open a Cloudflare Quick Tunnel and print the URL.
//!
//! Per `docs/07-networking.md`, `deputyctl tunnel` runs a `cloudflared
//! tunnel --url http://localhost:<port>` in the foreground; we capture
//! stderr to extract the published `https://*.trycloudflare.com` URL,
//! print it once on stdout, then proxy the rest of stderr through.
//!
//! `--background` daemonizes (well: detaches) and writes the PID to
//! `/run/deputyos/cloudflared.pid` so `systemctl` units / killscripts can
//! find it.

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use anyhow::{anyhow, Context, Result};
use futures_util::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::{
    client::IntoClientRequest,
    http::{HeaderName, HeaderValue},
    Message,
};
use url::Url;

use crate::paths;

#[derive(Debug, Clone)]
pub struct TunnelOpts {
    pub port: u16,
    pub background: bool,
}

impl Default for TunnelOpts {
    fn default() -> Self {
        Self {
            port: 8088,
            background: false,
        }
    }
}

fn cloudflared_binary() -> String {
    std::env::var("DEPUTYOS_CLOUDFLARED").unwrap_or_else(|_| "cloudflared".to_string())
}

fn cloudflared_available() -> bool {
    Command::new(cloudflared_binary())
        .arg("--version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

fn pid_file() -> PathBuf {
    if let Ok(p) = std::env::var("DEPUTYOS_CLOUDFLARED_PID") {
        PathBuf::from(p)
    } else {
        PathBuf::from("/run/deputyos/cloudflared.pid")
    }
}

pub fn run(opts: TunnelOpts) -> Result<u8> {
    if !cloudflared_available() {
        eprintln!("tunnel: cloudflared not installed — run `make doctor`");
        return Ok(1);
    }
    let url_arg = format!("http://localhost:{}", opts.port);
    let mut cmd = Command::new(cloudflared_binary());
    cmd.arg("tunnel")
        .arg("--url")
        .arg(&url_arg)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    let mut child = cmd
        .spawn()
        .with_context(|| format!("spawning cloudflared tunnel --url {url_arg}"))?;

    if opts.background {
        if let Some(parent) = pid_file().parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let _ = std::fs::write(pid_file(), format!("{}\n", child.id()));
        eprintln!("tunnel: spawned cloudflared (pid {})", child.id());
        // Detach: don't wait. Print URL when extracted, then return.
        if let Some(url) = extract_url_from_child(&mut child, /*forward_stderr=*/ false) {
            println!("{url}");
        }
        return Ok(0);
    }

    // Foreground: extract URL, then forward stderr until cloudflared exits.
    if let Some(url) = extract_url_from_child(&mut child, /*forward_stderr=*/ true) {
        println!("{url}");
        let _ = std::io::stdout().flush();
    }
    let status = child.wait().context("waiting on cloudflared")?;
    Ok(if status.success() { 0 } else { 1 })
}

/// Read cloudflared stderr line-by-line, extract the trycloudflare.com URL,
/// and (optionally) keep proxying subsequent stderr to our own stderr.
fn extract_url_from_child(child: &mut std::process::Child, forward_stderr: bool) -> Option<String> {
    let stderr = child.stderr.take()?;
    let mut reader = BufReader::new(stderr);
    let mut line = String::new();
    let mut found: Option<String> = None;
    loop {
        line.clear();
        match reader.read_line(&mut line) {
            Ok(0) => break,
            Ok(_) => {
                if forward_stderr {
                    eprint!("{line}");
                }
                if found.is_none() {
                    if let Some(url) = scan_for_trycloudflare_url(&line) {
                        found = Some(url.clone());
                        if !forward_stderr {
                            // Background: stop slurping; let the child run.
                            break;
                        }
                    }
                }
            }
            Err(_) => break,
        }
    }
    // If foreground and we found the URL early, continue piping stderr in
    // a background thread so the user sees connection logs.
    if forward_stderr {
        std::thread::spawn(move || {
            let mut buf = String::new();
            loop {
                buf.clear();
                match reader.read_line(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(_) => {
                        eprint!("{buf}");
                    }
                }
            }
        });
    }
    found
}

/// Scan one stderr line for an `https://*.trycloudflare.com` URL.
/// Cloudflared prints a banner with the URL surrounded by ASCII art —
/// we need only the bare URL.
pub fn scan_for_trycloudflare_url(line: &str) -> Option<String> {
    let needle = "https://";
    let idx = line.find(needle)?;
    let tail = &line[idx..];
    let mut end = tail.len();
    for (i, c) in tail.char_indices() {
        if c.is_whitespace() || c == '|' || c == '"' || c == '\'' || c == '>' || c == ')' {
            end = i;
            break;
        }
    }
    let candidate = &tail[..end];
    if candidate.contains(".trycloudflare.com") {
        Some(candidate.trim_end_matches(['.', ',', ';']).to_string())
    } else {
        None
    }
}

// ---- Integrated tunnel (M6+M8) ----

#[derive(Debug, Clone, Serialize, Deserialize)]
struct IntegratedTunnelFrame {
    #[serde(default = "default_frame_kind")]
    kind: String,
    #[serde(default)]
    id: String,
    #[serde(default = "default_service")]
    service: String,
    #[serde(default)]
    method: String,
    #[serde(default)]
    path: String,
    #[serde(default)]
    headers: HashMap<String, String>,
    #[serde(default)]
    body: Option<String>,
    #[serde(default)]
    status: Option<u16>,
    #[serde(default)]
    body_bytes: Option<Vec<u8>>,
    #[serde(default)]
    error: Option<String>,
    #[serde(default)]
    stream_id: String,
    #[serde(default)]
    message_type: String,
}

fn default_frame_kind() -> String {
    "http_request".to_string()
}

fn default_service() -> String {
    "webui".to_string()
}

/// Run the integrated WebSocket tunnel client. Connects to
/// `api.deputyos.com`, authenticates with the tunnel token, and
/// proxies HTTP requests from the cloud to the local port.
pub fn run_integrated(opts: TunnelOpts) -> Result<u8> {
    let token_path = paths::tunnel_token_file();
    if !token_path.is_file() {
        eprintln!(
            "tunnel --integrated: no tunnel token at {}",
            token_path.display()
        );
        eprintln!("  create an account and register this device via the wizard, or");
        eprintln!(
            "  place a tunnel token at {} (mode 0600)",
            token_path.display()
        );
        return Ok(1);
    }
    let token = std::fs::read_to_string(&token_path)
        .with_context(|| format!("reading {}", token_path.display()))?
        .trim()
        .to_string();
    if token.is_empty() {
        eprintln!("tunnel --integrated: tunnel token is empty");
        return Ok(1);
    }

    let url = integrated_url_override().unwrap_or_else(|| {
        // Honors --api-base/DEPUTYOS_API_BASE//etc/deputyos/api-base/default
        // (see apibase) so an integrated tunnel on a self-hosted-backend
        // appliance connects to the right coordinator.
        let api_base = crate::apibase::resolve(None);
        integrated_tunnel_url(&api_base, &token, registered_device_id().as_deref())
    });

    eprintln!(
        "tunnel: connecting to {}",
        url.split('?').next().unwrap_or("<invalid tunnel URL>")
    );
    eprintln!("tunnel: exposing native WebUI plus reserved control and terminal services");

    if opts.background {
        detach_integrated(&url, opts.port)?;
    } else {
        foreground_integrated(&url, opts.port)?;
    }
    Ok(0)
}

fn foreground_integrated(url: &str, port: u16) -> Result<()> {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .context("building integrated tunnel runtime")?;
    runtime.block_on(foreground_integrated_async(url, port))
}

async fn foreground_integrated_async(url: &str, port: u16) -> Result<()> {
    let (socket, _response) = tokio_tungstenite::connect_async(url)
        .await
        .with_context(|| format!("connecting integrated tunnel websocket {url}"))?;
    let (mut cloud_write, mut cloud_read) = socket.split();
    let (cloud_tx, mut cloud_rx) = mpsc::unbounded_channel::<Message>();
    let (stream_done_tx, mut stream_done_rx) = mpsc::unbounded_channel::<String>();
    let mut streams: HashMap<String, mpsc::UnboundedSender<IntegratedTunnelFrame>> = HashMap::new();
    let local_agent = ureq::AgentBuilder::new()
        .timeout_connect(std::time::Duration::from_secs(5))
        .timeout(std::time::Duration::from_secs(30))
        .build();

    loop {
        tokio::select! {
            outbound = cloud_rx.recv() => {
                match outbound {
                    Some(message) => cloud_write
                        .send(message)
                        .await
                        .context("sending integrated tunnel frame")?,
                    None => break,
                }
            }
            done = stream_done_rx.recv() => {
                if let Some(stream_id) = done {
                    streams.remove(&stream_id);
                }
            }
            incoming = cloud_read.next() => {
                let message = match incoming {
                    Some(Ok(message)) => message,
                    Some(Err(error)) => return Err(error).context("reading integrated tunnel frame"),
                    None => {
                        eprintln!("tunnel: server closed connection");
                        break;
                    }
                };
                match message {
                    Message::Text(text) => {
                        let frame: IntegratedTunnelFrame = match serde_json::from_str(&text) {
                            Ok(frame) => frame,
                            Err(error) => {
                                eprintln!("tunnel: parse error: {error}");
                                continue;
                            }
                        };
                        if let Some(error) = frame.error.as_deref() {
                            eprintln!("tunnel: server event: {error}");
                            if frame.id == "timeout" || frame.id == "evict" {
                                break;
                            }
                        }
                        match frame.kind.as_str() {
                            "http_request" => {
                                let agent = local_agent.clone();
                                let outbound = cloud_tx.clone();
                                tokio::task::spawn_blocking(move || {
                                    let response = proxy_frame_to_local(&agent, port, &frame);
                                    send_frame(&outbound, &response);
                                });
                            }
                            "stream_open" if !frame.stream_id.is_empty() => {
                                if streams.contains_key(&frame.stream_id) {
                                    send_stream_error(
                                        &cloud_tx,
                                        &frame,
                                        "duplicate stream id",
                                    );
                                    continue;
                                }
                                let stream_id = frame.stream_id.clone();
                                let (tx, rx) = mpsc::unbounded_channel();
                                streams.insert(stream_id.clone(), tx);
                                let outbound = cloud_tx.clone();
                                let done = stream_done_tx.clone();
                                tokio::spawn(async move {
                                    if let Err(error) =
                                        proxy_websocket_to_local(port, frame.clone(), rx, outbound.clone()).await
                                    {
                                        send_stream_error(&outbound, &frame, &error.to_string());
                                    }
                                    let _ = done.send(stream_id);
                                });
                            }
                            "stream_data" | "stream_close" if !frame.stream_id.is_empty() => {
                                if let Some(tx) = streams.get(&frame.stream_id) {
                                    let closing = frame.kind == "stream_close";
                                    let _ = tx.send(frame.clone());
                                    if closing {
                                        streams.remove(&frame.stream_id);
                                    }
                                }
                            }
                            _ => {}
                        }
                    }
                    Message::Ping(payload) => {
                        let _ = cloud_tx.send(Message::Pong(payload));
                    }
                    Message::Close(_) => {
                        eprintln!("tunnel: server closed connection");
                        break;
                    }
                    Message::Pong(_) | Message::Binary(_) | Message::Frame(_) => {}
                }
            }
        }
    }
    Ok(())
}

fn send_frame(tx: &mpsc::UnboundedSender<Message>, frame: &IntegratedTunnelFrame) {
    match serde_json::to_string(frame) {
        Ok(json) => {
            let _ = tx.send(Message::Text(json.into()));
        }
        Err(error) => eprintln!("tunnel: serialise error: {error}"),
    }
}

fn send_stream_error(
    tx: &mpsc::UnboundedSender<Message>,
    request: &IntegratedTunnelFrame,
    error: &str,
) {
    let mut response = request.clone();
    response.kind = "stream_close".to_string();
    response.error = Some(error.to_string());
    response.body = None;
    response.body_bytes = None;
    send_frame(tx, &response);
}

#[derive(Debug, Clone)]
struct LocalTarget {
    http_origin: String,
    websocket_origin: String,
}

fn target_for(service: &str, control_port: u16) -> Result<LocalTarget> {
    let origin = match service {
        "webui" => native_webui_origin()?,
        "control" => format!("http://127.0.0.1:{control_port}"),
        "terminal" => "http://127.0.0.1:8090".to_string(),
        other => return Err(anyhow!("tunnel target '{other}' is not allow-listed")),
    };
    let websocket_origin = if let Some(rest) = origin.strip_prefix("https://") {
        format!("wss://{rest}")
    } else if let Some(rest) = origin.strip_prefix("http://") {
        format!("ws://{rest}")
    } else {
        return Err(anyhow!("unsupported local target scheme"));
    };
    Ok(LocalTarget {
        http_origin: origin,
        websocket_origin,
    })
}

fn native_webui_origin() -> Result<String> {
    if let Ok(value) = std::env::var("DEPUTYOS_NATIVE_WEBUI_ORIGIN") {
        return validate_local_origin(&value);
    }
    let (_, manifest) = crate::profile::load_active()?;
    let health = Url::parse(&manifest.health.http_check).with_context(|| {
        format!(
            "parsing native WebUI health URL {}",
            manifest.health.http_check
        )
    })?;
    let scheme = health.scheme();
    let host = health
        .host_str()
        .ok_or_else(|| anyhow!("native WebUI health URL has no host"))?;
    let port = health
        .port_or_known_default()
        .ok_or_else(|| anyhow!("native WebUI health URL has no port"))?;
    validate_local_origin(&format!("{scheme}://{host}:{port}"))
}

fn validate_local_origin(value: &str) -> Result<String> {
    let parsed = Url::parse(value).with_context(|| format!("parsing local origin {value}"))?;
    if !matches!(parsed.scheme(), "http" | "https") {
        return Err(anyhow!("local origin must use http or https"));
    }
    let host = parsed
        .host_str()
        .ok_or_else(|| anyhow!("local origin has no host"))?;
    if !matches!(host, "127.0.0.1" | "localhost" | "::1") {
        return Err(anyhow!("local origin must be loopback, got {host}"));
    }
    let port = parsed
        .port_or_known_default()
        .ok_or_else(|| anyhow!("local origin has no port"))?;
    Ok(format!(
        "{}://{}:{port}",
        parsed.scheme(),
        format_host(host)
    ))
}

fn format_host(host: &str) -> String {
    if host.contains(':') {
        format!("[{host}]")
    } else {
        host.to_string()
    }
}

fn proxy_frame_to_local(
    agent: &ureq::Agent,
    port: u16,
    frame: &IntegratedTunnelFrame,
) -> IntegratedTunnelFrame {
    let target = match target_for(&frame.service, port) {
        Ok(target) => target,
        Err(error) => {
            return response_frame(
                frame,
                403,
                HashMap::new(),
                Vec::new(),
                Some(error.to_string()),
            )
        }
    };
    let path = if frame.path.starts_with('/') {
        frame.path.clone()
    } else {
        format!("/{}", frame.path)
    };
    let local_url = format!("{}{path}", target.http_origin);
    let method = frame.method.to_ascii_uppercase();
    let mut request = agent.request(&method, &local_url);
    for (name, value) in &frame.headers {
        let lower = name.to_ascii_lowercase();
        if matches!(
            lower.as_str(),
            "host" | "connection" | "content-length" | "transfer-encoding"
        ) {
            continue;
        }
        request = request.set(name, value);
    }

    let body = frame
        .body_bytes
        .clone()
        .unwrap_or_else(|| frame.body.clone().unwrap_or_default().into_bytes());
    let result = if matches!(method.as_str(), "GET" | "HEAD") && body.is_empty() {
        request.call()
    } else {
        request.send_bytes(&body)
    };

    match result {
        Ok(resp) => {
            let status = resp.status();
            let headers = response_headers(&resp);
            response_frame(frame, status, headers, read_response_body(resp), None)
        }
        Err(ureq::Error::Status(status, resp)) => {
            let headers = response_headers(&resp);
            response_frame(frame, status, headers, read_response_body(resp), None)
        }
        Err(e) => response_frame(frame, 502, HashMap::new(), Vec::new(), Some(e.to_string())),
    }
}

fn response_frame(
    request: &IntegratedTunnelFrame,
    status: u16,
    headers: HashMap<String, String>,
    body_bytes: Vec<u8>,
    error: Option<String>,
) -> IntegratedTunnelFrame {
    IntegratedTunnelFrame {
        kind: "http_response".to_string(),
        id: request.id.clone(),
        service: request.service.clone(),
        method: request.method.clone(),
        path: request.path.clone(),
        headers,
        body: None,
        status: Some(status),
        body_bytes: Some(body_bytes),
        error,
        stream_id: String::new(),
        message_type: String::new(),
    }
}

fn response_headers(resp: &ureq::Response) -> HashMap<String, String> {
    resp.headers_names()
        .into_iter()
        .filter_map(|name| {
            let lower = name.to_ascii_lowercase();
            if matches!(
                lower.as_str(),
                "connection" | "content-length" | "transfer-encoding" | "keep-alive" | "upgrade"
            ) {
                return None;
            }
            resp.header(&name).map(|value| {
                let value = rewrite_local_location(&lower, value);
                (lower, value)
            })
        })
        .collect()
}

fn rewrite_local_location(name: &str, value: &str) -> String {
    if name != "location" {
        return value.to_string();
    }
    if let Ok(url) = Url::parse(value) {
        if matches!(url.host_str(), Some("127.0.0.1" | "localhost" | "::1")) {
            let mut relative = url.path().to_string();
            if let Some(query) = url.query() {
                relative.push('?');
                relative.push_str(query);
            }
            return relative;
        }
    }
    value.to_string()
}

async fn proxy_websocket_to_local(
    control_port: u16,
    open: IntegratedTunnelFrame,
    mut cloud_rx: mpsc::UnboundedReceiver<IntegratedTunnelFrame>,
    cloud_tx: mpsc::UnboundedSender<Message>,
) -> Result<()> {
    let target = target_for(&open.service, control_port)?;
    let path = if open.path.starts_with('/') {
        open.path.clone()
    } else {
        format!("/{}", open.path)
    };
    let local_url = format!("{}{path}", target.websocket_origin);
    let mut request = local_url
        .as_str()
        .into_client_request()
        .context("building local WebSocket request")?;
    for (name, value) in &open.headers {
        let lower = name.to_ascii_lowercase();
        if matches!(
            lower.as_str(),
            "host"
                | "connection"
                | "content-length"
                | "transfer-encoding"
                | "upgrade"
                | "sec-websocket-key"
                | "sec-websocket-version"
                | "sec-websocket-extensions"
        ) {
            continue;
        }
        if let (Ok(name), Ok(value)) = (name.parse::<HeaderName>(), value.parse::<HeaderValue>()) {
            request.headers_mut().insert(name, value);
        }
    }
    let (socket, _) = tokio_tungstenite::connect_async(request)
        .await
        .with_context(|| format!("connecting local WebSocket {local_url}"))?;
    let (mut local_write, mut local_read) = socket.split();

    let mut accepted = open.clone();
    accepted.kind = "stream_accept".to_string();
    accepted.error = None;
    send_frame(&cloud_tx, &accepted);

    loop {
        tokio::select! {
            local = local_read.next() => {
                let message = match local {
                    Some(Ok(message)) => message,
                    Some(Err(error)) => return Err(error).context("reading local WebSocket"),
                    None => break,
                };
                let mut frame = open.clone();
                frame.kind = "stream_data".to_string();
                frame.body = None;
                frame.body_bytes = None;
                match message {
                    Message::Text(text) => {
                        frame.message_type = "text".to_string();
                        frame.body = Some(text.to_string());
                    }
                    Message::Binary(bytes) => {
                        frame.message_type = "binary".to_string();
                        frame.body_bytes = Some(bytes.to_vec());
                    }
                    Message::Ping(bytes) => {
                        frame.message_type = "ping".to_string();
                        frame.body_bytes = Some(bytes.to_vec());
                    }
                    Message::Pong(bytes) => {
                        frame.message_type = "pong".to_string();
                        frame.body_bytes = Some(bytes.to_vec());
                    }
                    Message::Close(_) => break,
                    Message::Frame(_) => continue,
                }
                send_frame(&cloud_tx, &frame);
            }
            cloud = cloud_rx.recv() => {
                let Some(frame) = cloud else { break };
                if frame.kind == "stream_close" {
                    let _ = local_write.send(Message::Close(None)).await;
                    break;
                }
                let payload = frame.body_bytes.clone().unwrap_or_default();
                let message = match frame.message_type.as_str() {
                    "text" => Message::Text(frame.body.unwrap_or_default().into()),
                    "binary" => Message::Binary(payload.into()),
                    "ping" => Message::Ping(payload.into()),
                    "pong" => Message::Pong(payload.into()),
                    _ => continue,
                };
                local_write.send(message).await.context("writing local WebSocket")?;
            }
        }
    }

    let mut closed = open;
    closed.kind = "stream_close".to_string();
    closed.body = None;
    closed.body_bytes = None;
    send_frame(&cloud_tx, &closed);
    Ok(())
}

fn read_response_body(resp: ureq::Response) -> Vec<u8> {
    let mut reader = resp.into_reader();
    let mut out = Vec::new();
    let _ = reader.read_to_end(&mut out);
    out
}

fn integrated_tunnel_url(api_base: &str, token: &str, device_id: Option<&str>) -> String {
    let trimmed = api_base.trim_end_matches('/');
    let base = if trimmed == "https://api.deputyos.com" {
        device_id
            .filter(|id| {
                !id.is_empty()
                    && id
                        .bytes()
                        .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
            })
            .map(|id| format!("wss://{id}.tunnel.deputyos.com"))
            .unwrap_or_else(|| "wss://api.deputyos.com".to_string())
    } else if let Some(rest) = trimmed.strip_prefix("https://") {
        format!("wss://{rest}")
    } else if let Some(rest) = trimmed.strip_prefix("http://") {
        format!("ws://{rest}")
    } else if trimmed.starts_with("ws://") || trimmed.starts_with("wss://") {
        trimmed.to_string()
    } else {
        format!("wss://{trimmed}")
    };
    format!("{base}/api/v1/tunnel/connect?token={token}")
}

fn registered_device_id() -> Option<String> {
    let account = std::fs::read_to_string(paths::account_file()).ok()?;
    serde_json::from_str::<serde_json::Value>(&account)
        .ok()?
        .get("device_id")?
        .as_str()
        .map(str::to_string)
}

fn detach_integrated(url: &str, port: u16) -> Result<()> {
    let exe = std::env::current_exe().context("resolving current deputyctl executable")?;
    let mut cmd = Command::new(exe);
    cmd.args(["tunnel", "--integrated", "--port", &port.to_string()])
        .env("DEPUTYOS_INTEGRATED_TUNNEL_URL", url)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    let child = cmd
        .spawn()
        .context("spawning integrated tunnel background process")?;
    let pid_file = Path::new("/run/deputyos/deputyos-tunnel.pid");
    if let Some(parent) = pid_file.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let _ = std::fs::write(pid_file, format!("{}\n", child.id()));
    println!("tunnel: daemonized to background (pid {})", child.id());
    Ok(())
}

fn integrated_url_override() -> Option<String> {
    std::env::var("DEPUTYOS_INTEGRATED_TUNNEL_URL")
        .ok()
        .filter(|s| !s.trim().is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scan_extracts_url_from_banner() {
        let line = "2026-04-26T12:00:00Z INF +-------------------------------------+\n";
        assert!(scan_for_trycloudflare_url(line).is_none());
        let url_line = "2026-04-26T12:00:00Z INF |  https://random-words.trycloudflare.com  |\n";
        let got = scan_for_trycloudflare_url(url_line).expect("url");
        assert_eq!(got, "https://random-words.trycloudflare.com");
    }

    #[test]
    fn scan_strips_trailing_punctuation() {
        let line = "url=https://x-y-z.trycloudflare.com.\n";
        let got = scan_for_trycloudflare_url(line).expect("url");
        assert_eq!(got, "https://x-y-z.trycloudflare.com");
    }

    #[test]
    fn integrated_tunnel_url_uses_websocket_v1_endpoint() {
        assert_eq!(
            integrated_tunnel_url("https://api.deputyos.com", "tok", Some("dev-123")),
            "wss://dev-123.tunnel.deputyos.com/api/v1/tunnel/connect?token=tok"
        );
        assert_eq!(
            integrated_tunnel_url("http://127.0.0.1:3000/", "tok", Some("dev-123")),
            "ws://127.0.0.1:3000/api/v1/tunnel/connect?token=tok"
        );
    }

    #[test]
    fn integrated_targets_are_compiled_in_and_loopback_only() {
        assert!(target_for("control", 8088).is_ok());
        assert!(target_for("terminal", 8088).is_ok());
        assert!(target_for("arbitrary", 8088).is_err());
        assert!(validate_local_origin("http://127.0.0.1:8080").is_ok());
        assert!(validate_local_origin("http://localhost:42110").is_ok());
        assert!(validate_local_origin("http://192.168.1.20:8080").is_err());
        assert!(validate_local_origin("file:///tmp/socket").is_err());
    }

    /// Spawn a fake `cloudflared` script that prints a known URL on stderr,
    /// run deputyctl's tunnel runner against it, and assert we extract the URL.
    #[cfg(unix)]
    #[test]
    fn extracts_url_from_fake_cloudflared() {
        use std::os::unix::fs::PermissionsExt;
        let _g = crate::env_mutex().lock().unwrap_or_else(|e| e.into_inner());
        let dir = tempfile::tempdir().expect("tempdir");
        let fake = dir.path().join("cloudflared");
        std::fs::write(
            &fake,
            "#!/bin/sh\n\
             echo 'INF starting' >&2\n\
             echo 'INF Your quick Tunnel has been created! Visit it at:' >&2\n\
             echo 'INF https://fake-words-test.trycloudflare.com' >&2\n\
             # Stay alive so the runner can see the URL.\n\
             sleep 1\n",
        )
        .expect("write fake");
        let mut perm = std::fs::metadata(&fake).expect("md").permissions();
        perm.set_mode(0o755);
        std::fs::set_permissions(&fake, perm).expect("chmod");
        std::env::set_var("DEPUTYOS_CLOUDFLARED", &fake);

        // Run tunnel in foreground; our extractor should pull the URL and
        // the fake binary will exit via sleep 1.
        let code = run(TunnelOpts {
            port: 8088,
            background: false,
        })
        .expect("run");
        assert_eq!(code, 0);
        std::env::remove_var("DEPUTYOS_CLOUDFLARED");
    }
}
