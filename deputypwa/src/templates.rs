//! Hand-written `format!` HTML templates. Same approach as `deputywizard`:
//! lean, no askama, easy to grep. Each top-level page calls [`page`] which
//! supplies the chrome (header, manifest link, service-worker registration).

use crate::data::{AccountCard, Dashboard, ProviderEntry, TunnelCard};

pub const STYLE_CSS: &str = include_str!("../static/style.css");

/// Manifest body for a same-origin install. We render this as a const so
/// `tower-http`'s `ServeDir` isn't needed for the manifest itself.
pub const MANIFEST_JSON: &str = r##"{
  "name": "deputyOS",
  "short_name": "deputyOS",
  "description": "Always-on monitor + manage surface for an deputyOS appliance.",
  "start_url": "/app/dashboard",
  "scope": "/app/",
  "display": "standalone",
  "theme_color": "#0969da",
  "background_color": "#f7f7f8",
  "icons": [
    {
      "src": "/static/icon.svg",
      "sizes": "any",
      "type": "image/svg+xml",
      "purpose": "any"
    }
  ]
}"##;

/// Tiny SVG used as the install icon. Inline so we don't need a binary
/// asset for the M3 ship.
pub const ICON_SVG: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 64 64">
  <rect width="64" height="64" rx="12" fill="#0969da"/>
  <text x="50%" y="50%" dominant-baseline="middle" text-anchor="middle"
        fill="#fff" font-family="-apple-system,sans-serif" font-size="22" font-weight="700">aOS</text>
</svg>"##;

/// Service worker. Listens for `push` events and renders the system
/// notification. Kept minimal (no caching strategy — the dashboard is live
/// data, not an offline document).
pub const SERVICE_WORKER_JS: &str = r#"// deputyOS PWA service worker.
self.addEventListener('install', (e) => self.skipWaiting());
self.addEventListener('activate', (e) => self.clients.claim());
self.addEventListener('push', (event) => {
  let payload = { title: 'deputyOS', body: 'notification' };
  if (event.data) {
    try { payload = event.data.json(); } catch (_) { payload.body = event.data.text(); }
  }
  event.waitUntil(self.registration.showNotification(payload.title || 'deputyOS', {
    body: payload.body || '',
    icon: '/static/icon.svg',
    tag: payload.tag || 'deputyos'
  }));
});
self.addEventListener('notificationclick', (event) => {
  event.notification.close();
  event.waitUntil(self.clients.openWindow('/app/dashboard'));
});
"#;

/// HTML-escape user-supplied or runtime-derived strings. Same shape as the
/// wizard's helper.
pub fn escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '&' => out.push_str("&amp;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            _ => out.push(c),
        }
    }
    out
}

fn nav() -> &'static str {
    r#"<nav class="topnav">
  <a href="/app/dashboard">Dashboard</a>
  <a href="/app/logs">Logs</a>
  <a href="/app/keys">Keys</a>
  <a href="/app/network">Network</a>
  <a href="/app/tunnel">Tunnel</a>
  <a href="/app/account">Account</a>
  <a href="/app/mounts">Mounts</a>
</nav>"#
}

/// Render the /app/mounts page. Reads the live policy via
/// `deputyctl::mounts::list`. The list rows include a tiny inline form so
/// users can revoke a mount with one click.
pub fn mounts_page(entries: &[deputyctl::mounts::ListEntry], flash: Option<&str>) -> String {
    let flash_html = flash
        .map(|f| format!(r#"<div class="banner">{}</div>"#, escape(f)))
        .unwrap_or_default();

    let rows = if entries.is_empty() {
        r#"<tr><td colspan="6" class="muted">No mounts configured. Add one from the wizard or with <code>deputyctl mounts add</code>.</td></tr>"#.to_string()
    } else {
        entries
            .iter()
            .map(|e| {
                format!(
                    r#"<tr>
<td><code>{kind}</code></td>
<td><code>{id}</code></td>
<td><code>{guest}</code></td>
<td>{mode}</td>
<td><code>{source}</code></td>
<td>
  <form method="post" action="/app/mounts/remove">
    <input type="hidden" name="id" value="{id_attr}">
    <button type="submit" class="danger">Revoke</button>
  </form>
</td>
</tr>"#,
                    kind = escape(&e.kind),
                    id = escape(&e.id),
                    guest = escape(&e.guest_path),
                    mode = escape(&e.mode),
                    source = escape(&e.source),
                    id_attr = escape(&e.id),
                )
            })
            .collect::<String>()
    };

    let body = format!(
        r#"{flash}
<section class="card">
<h2>Mounts</h2>
<p class="muted">Drives + shares the agent can see. Every entry is reviewable and revocable.
Adding a new mount is easiest from the wizard or with <code>deputyctl mounts add</code>;
this page is the always-on revoke surface (PWA).</p>
<table class="grid">
<thead><tr><th>Kind</th><th>Id</th><th>Guest path</th><th>Mode</th><th>Source</th><th></th></tr></thead>
<tbody>
{rows}
</tbody>
</table>
<p class="muted">Network shares store credentials in <code>/etc/deputyos/secrets.env</code> (mode 0600); the policy file never sees them.</p>
</section>"#,
        flash = flash_html,
        rows = rows,
    );
    page("Mounts", &body, false)
}

/// Network egress policy page.
pub fn network_page(
    policy: &serde_json::Value,
    _mount_entries: &[deputyctl::mounts::ListEntry],
) -> String {
    let mode = policy
        .get("mode")
        .and_then(|v| v.as_str())
        .unwrap_or("unknown");
    let mode_badge = match mode {
        "open" => "<span class=\"badge ok\">open</span>",
        "whitelist" => "<span class=\"badge warn\">whitelist</span>",
        "airgap" => "<span class=\"badge fail\">airgap</span>",
        _ => "<span class=\"badge\">unknown</span>",
    };
    let hosts = policy
        .get("allow_hosts")
        .and_then(|v| v.as_array())
        .map(|a| {
            if a.is_empty() {
                "<li>(empty)</li>".to_string()
            } else {
                a.iter()
                    .filter_map(|h| h.as_str())
                    .map(|h| format!("<li><code>{}</code></li>", escape(h)))
                    .collect()
            }
        })
        .unwrap_or_else(|| "<li>(unavailable)</li>".to_string());
    let body = format!(
        r#"<article class="card wide">
<h2>Network Egress Policy</h2>
<dl>
<dt>Mode</dt><dd>{mode_badge} <strong>{mode}</strong></dd>
</dl>
<h3>Allow-listed hosts</h3>
<ul class="hosts">{hosts}</ul>
<p class="muted">Manage egress with <code>deputyctl network mode &lt;open|whitelist|airgap&gt;</code>
and <code>deputyctl network allow add &lt;host&gt;</code>, then
<code>deputyctl network apply</code>.</p>
</article>"#,
        mode = escape(mode),
        mode_badge = mode_badge,
        hosts = hosts,
    );
    page("Network", &body, false)
}

/// `/app/tunnel` (M8) — integrated cloud relay state + the copy-able public URL.
/// Token presence is shown as a badge; the secret itself is never rendered.
pub fn tunnel_page(card: &TunnelCard) -> String {
    let active_badge = if card.active {
        r#"<span class="badge ok">active</span>"#
    } else {
        r#"<span class="badge">inactive</span>"#
    };
    let enabled_badge = if card.enabled {
        r#"<span class="badge ok">enabled at boot</span>"#
    } else {
        r#"<span class="badge warn">not enabled at boot</span>"#
    };
    let token_badge = if card.token_present {
        r#"<span class="badge ok">present</span>"#
    } else {
        r#"<span class="badge fail">missing</span>"#
    };
    let body = format!(
        r#"<article class="card wide">
<h2>Integrated Tunnel</h2>
<p class="muted">The integrated cloud relay connects this device to <code>api.deputyos.com</code>
over an authenticated WebSocket — no inbound ports, no port forwarding. The relay is path-based:
<code>/api/v1/tunnel/proxy/&lt;account&gt;/&lt;path&gt;</code>.</p>
<dl>
<dt>State</dt><dd>{active_badge} <code>{kind}</code></dd>
<dt>Boot</dt><dd>{enabled_badge}</dd>
<dt>Tunnel token</dt><dd>{token_badge} <span class="muted">(presence only — the token itself is never shown)</span></dd>
</dl>
<h3>Public URL</h3>
<p class="muted">Once <code>deputyctl tunnel --integrated</code> is running, this device is reachable at:</p>
<div class="copyrow">
<code id="tunnel-url">{url}</code>
<button type="button" onclick="(function(btn){{navigator.clipboard.writeText(document.getElementById('tunnel-url').textContent).then(function(){{btn.textContent='Copied';}});}})(this)">Copy</button>
</div>
<p class="muted">If the URL still shows <code>&lt;account&gt;</code>, the device has no account label yet —
register one via the wizard Account step. The relay matches the account email <em>or</em> id.</p>
</article>"#,
        active_badge = active_badge,
        kind = escape(&card.kind),
        enabled_badge = enabled_badge,
        token_badge = token_badge,
        url = escape(&card.public_url),
    );
    page("Tunnel", &body, card.stub)
}

/// `/app/account` (M8) — device identity + capability-token presence. Tokens
/// are rendered as presence booleans only, never their contents.
pub fn account_page(card: &AccountCard) -> String {
    let registered_badge = if card.registered {
        r#"<span class="badge ok">registered</span>"#
    } else {
        r#"<span class="badge warn">no account</span>"#
    };
    let email = if card.email.is_empty() {
        r#"<span class="muted">(not set)</span>"#.to_string()
    } else {
        escape(&card.email)
    };
    let device_id = if card.device_id.is_empty() {
        r#"<span class="muted">(not registered)</span>"#.to_string()
    } else {
        format!("<code>{}</code>", escape(&card.device_id))
    };
    let body = format!(
        r#"<article class="card wide">
<h2>Account</h2>
<p class="muted">Identity + capability-token presence for this device. Every flow still works
without an account; the account only unlocks the cloud relay, managed backup/restore, and audit.</p>
<dl>
<dt>Status</dt><dd>{registered_badge}</dd>
<dt>Email</dt><dd>{email}</dd>
<dt>Device id</dt><dd>{device_id}</dd>
<dt>Device name</dt><dd><code>{device_name}</code></dd>
<dt>Tunnel token</dt><dd>{tunnel_token_badge}</dd>
<dt>Backup token</dt><dd>{backup_token_badge}</dd>
</dl>
<p class="muted">Tokens are shown as <strong>presence only</strong> (configured / not configured). The capability
secrets themselves are never read or rendered by the PWA. Manage them via the wizard Account step or
<code>deputyctl</code>; rotate by re-registering the device.</p>
</article>"#,
        registered_badge = registered_badge,
        email = email,
        device_id = device_id,
        device_name = escape(&card.device_name),
        tunnel_token_badge = presence_badge(card.tunnel_token_present, "tunnel token"),
        backup_token_badge = presence_badge(card.backup_token_present, "backup token"),
    );
    page("Account", &body, card.stub)
}

/// A presence badge: green "present" or red "missing", with the label muted.
fn presence_badge(present: bool, label: &str) -> String {
    let badge = if present {
        r#"<span class="badge ok">present</span>"#
    } else {
        r#"<span class="badge fail">missing</span>"#
    };
    format!(
        r#"{badge} <span class="muted">({label})</span>"#,
        badge = badge,
        label = label
    )
}

/// Page chrome. Always emits the manifest link + service-worker registration
/// so any visit upgrades the installable surface.
pub fn page(title: &str, body: &str, stub_banner: bool) -> String {
    let banner = if stub_banner {
        r#"<div class="banner">Dev-stub data — no live deputyctl on this host.</div>"#
    } else {
        ""
    };
    format!(
        r##"<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>{title} — deputyOS</title>
<link rel="manifest" href="/manifest.webmanifest">
<link rel="icon" type="image/svg+xml" href="/static/icon.svg">
<link rel="stylesheet" href="/static/style.css">
<meta name="theme-color" content="#0969da">
</head>
<body>
<header class="bar">
  <h1>deputyOS</h1>
  {nav}
</header>
{banner}
<main class="layout-pwa">
{body}
</main>
<script>
if ('serviceWorker' in navigator) {{
  navigator.serviceWorker.register('/sw.js').catch(() => {{}});
}}
</script>
</body>
</html>"##,
        title = escape(title),
        nav = nav(),
        banner = banner,
        body = body,
    )
}

/// Dashboard card grid. Renders four cards: status, your-device,
/// cost, doctor.
pub fn dashboard(d: &Dashboard) -> String {
    let banner = if d.status.cost_tripped {
        r#"<section class="banner warn" style="margin: 1rem; padding: 1rem; border-left: 4px solid var(--warn, #e6a23c);">
<h2>Spending Paused</h2>
<p>Your daily or monthly cost cap has been reached. The agent is paused.</p>
<form method="post" action="/app/cost/raise-cap" style="display: inline-flex; gap: 0.5rem;">
  <label>New daily cap (USD): <input type="number" name="daily_cap" step="1" min="1" placeholder="10" style="width: 6rem;" /></label>
  <button type="submit">Raise Cap &amp; Resume</button>
</form>
<form method="post" action="/app/reset-cost-trip" style="display: inline;">
  <button type="submit" class="secondary">Reset Tripped Marker (no cap change)</button>
</form>
</section>"#
    } else {
        ""
    };

    let body = format!(
        r#"{banner}
<section class="cards">
{status}
{device}
{network}
{cost}
{doctor}
</section>"#,
        status = card_status(d),
        device = card_device(d),
        network = card_network(d),
        cost = card_cost(d),
        doctor = card_doctor(d),
    );
    page("Dashboard", &body, d.stub)
}

fn card_status(d: &Dashboard) -> String {
    let healthy = crate::data::agent_healthy(&d.status);
    let badge = if healthy {
        r#"<span class="badge ok">active</span>"#
    } else {
        r#"<span class="badge fail">unhealthy</span>"#
    };
    let cost_badge = if d.status.cost_tripped {
        r#"<span class="badge fail">cap tripped</span>"#
    } else {
        ""
    };
    let uptime = crate::data::format_uptime(d.status.uptime_seconds);
    let tunnel_badge = match d.status.tunnel.active_state.as_str() {
        "active" => r#"<span class="badge ok">connected</span>"#,
        "activating" => r#"<span class="badge warn">connecting</span>"#,
        "failed" => r#"<span class="badge fail">failed</span>"#,
        _ => r#"<span class="badge">off</span>"#,
    };
    let tunnel_unit = if d.status.tunnel.unit.is_empty() {
        "deputyos-tunnel.service"
    } else {
        &d.status.tunnel.unit
    };
    let tunnel_mode = if d.status.tunnel.on_demand {
        "on-demand"
    } else if d.status.tunnel.enabled {
        "enabled"
    } else {
        "disabled"
    };
    format!(
        r#"<article class="card">
<h2>Status {badge}</h2>
<dl>
<dt>Profile</dt><dd>{profile}</dd>
<dt>Unit</dt><dd>{unit}</dd>
<dt>Active state</dt><dd>{state}</dd>
<dt>Remote access</dt><dd>{tunnel_badge} {tunnel_state} ({tunnel_mode}, {tunnel_unit})</dd>
<dt>Uptime</dt><dd>{uptime}</dd>
<dt>Version</dt><dd>{ver} ({channel}, kernel {kernel})</dd>
<dt>Cost</dt><dd>{cost_badge}</dd>
</dl>
</article>"#,
        badge = badge,
        cost_badge = if cost_badge.is_empty() {
            "ok".to_string()
        } else {
            cost_badge.to_string()
        },
        profile = escape(&d.status.profile_id),
        unit = escape(&d.status.unit),
        state = escape(&d.status.active_state),
        tunnel_badge = tunnel_badge,
        tunnel_state = escape(&d.status.tunnel.active_state),
        tunnel_mode = escape(tunnel_mode),
        tunnel_unit = escape(tunnel_unit),
        uptime = escape(&uptime),
        ver = escape(&d.version.binary_version),
        channel = escape(&d.version.channel),
        kernel = escape(&d.version.kernel),
    )
}

fn card_device(d: &Dashboard) -> String {
    let caps = match &d.limits.capabilities {
        serde_json::Value::Object(m) => m
            .iter()
            .filter(|(_, v)| matches!(v, serde_json::Value::Bool(true)))
            .map(|(k, _)| format!("<li>{}</li>", escape(k)))
            .collect::<String>(),
        _ => String::new(),
    };
    let limitations = if d.limits.limitations.is_empty() {
        "<li>None.</li>".to_string()
    } else {
        d.limits
            .limitations
            .iter()
            .map(|l| {
                format!(
                    "<li><strong>{id}</strong> — {reason}<br><em>Unblock: {unblock}</em></li>",
                    id = escape(&l.id),
                    reason = escape(&l.reason),
                    unblock = escape(&l.unblock),
                )
            })
            .collect::<String>()
    };
    // Airgap badge: an air-gapped build bakes an egress-deny nftables ruleset
    // and a network policy mode=airgap, so the network mode is the source of
    // truth for "this device is air-gapped". Mirrors card_network's badge.
    let airgap_badge = if d.network.mode == "airgap" {
        r#" <span class="badge fail" title="Air-gapped: no network egress. Local LLMs only.">airgap</span>"#
    } else {
        ""
    };
    format!(
        r#"<article class="card">
<h2>Your device{airgap_badge}</h2>
<dl>
<dt>Target</dt><dd>{target}</dd>
<dt>Tier</dt><dd>{tier}</dd>
<dt>RAM</dt><dd>{ram} MB</dd>
<dt>Storage class</dt><dd>{storage}</dd>
</dl>
<h3>Active capabilities</h3>
<ul class="caps">{caps}</ul>
<h3>Limitations</h3>
<ul class="lims">{limitations}</ul>
<p class="muted">Capability + limitation data comes from <code>limits.json</code>; see the
<a href="https://www.deputyos.com/docs/reference/schemas/limits-json/" rel="noopener">limits reference</a>{airgap_docs}.</p>
</article>"#,
        airgap_badge = airgap_badge,
        target = escape(&d.limits.target),
        tier = escape(&d.limits.tier),
        ram = d.limits.ram_mb,
        storage = escape(&d.limits.storage_class),
        caps = if caps.is_empty() {
            "<li>None reported.</li>".to_string()
        } else {
            caps
        },
        limitations = limitations,
        airgap_docs = if d.network.mode == "airgap" {
            r#" and the <a href="https://www.deputyos.com/docs/concepts/airgap/" rel="noopener">air-gapped build guide</a>"#
        } else {
            ""
        },
    )
}

fn card_network(d: &Dashboard) -> String {
    let mode = &d.network.mode;
    let mode_badge = match mode.as_str() {
        "open" => r#"<span class="badge ok">open</span>"#,
        "whitelist" => r#"<span class="badge warn">whitelist</span>"#,
        "airgap" => r#"<span class="badge fail">airgap</span>"#,
        _ => r#"<span class="badge">unknown</span>"#,
    };
    let hosts = if d.network.allow_hosts.is_empty() {
        "<li>(none)</li>".to_string()
    } else {
        d.network
            .allow_hosts
            .iter()
            .map(|h| format!("<li><code>{}</code></li>", escape(h)))
            .collect()
    };
    let built_in = if d.network.set_at_build_time {
        " (set at build time)"
    } else {
        ""
    };
    format!(
        r#"<article class="card">
<h2>Network {mode_badge}</h2>
<dl>
<dt>Egress mode</dt><dd>{mode}{built_in}</dd>
</dl>
<h3>Allow-listed hosts</h3>
<ul class="hosts">{hosts}</ul>
<p class="muted">Manage with <code>deputyctl network</code>.</p>
</article>"#,
        mode = escape(mode),
        mode_badge = mode_badge,
        built_in = built_in,
        hosts = hosts,
    )
}

fn card_cost(d: &Dashboard) -> String {
    let day_pct = if d.cost.daily_cap_usd > 0.0 {
        (d.cost.today_usd / d.cost.daily_cap_usd * 100.0).clamp(0.0, 999.0)
    } else {
        0.0
    };
    let mon_pct = if d.cost.monthly_cap_usd > 0.0 {
        (d.cost.month_usd / d.cost.monthly_cap_usd * 100.0).clamp(0.0, 999.0)
    } else {
        0.0
    };
    let recent = if d.cost.recent.is_empty() {
        "<li>(no recent calls)</li>".to_string()
    } else {
        d.cost
            .recent
            .iter()
            .take(5)
            .map(|r| {
                format!(
                    "<li>${usd:.4} — {provider}/{model} <small>{ts}</small></li>",
                    usd = r.usd,
                    provider = escape(&r.provider),
                    model = escape(&r.model),
                    ts = escape(&r.timestamp),
                )
            })
            .collect()
    };
    format!(
        r#"<article class="card">
<h2>Cost</h2>
<dl>
<dt>Today</dt><dd>${today:.2} / ${daily_cap:.2} ({day_pct:.0}%)</dd>
<dt>Month</dt><dd>${month:.2} / ${monthly_cap:.2} ({mon_pct:.0}%)</dd>
</dl>
<h3>Recent expensive calls</h3>
<ul class="recent">{recent}</ul>
</article>"#,
        today = d.cost.today_usd,
        daily_cap = d.cost.daily_cap_usd,
        day_pct = day_pct,
        month = d.cost.month_usd,
        monthly_cap = d.cost.monthly_cap_usd,
        mon_pct = mon_pct,
        recent = recent,
    )
}

fn card_doctor(d: &Dashboard) -> String {
    let checks = d
        .doctor
        .checks
        .iter()
        .map(|c| {
            let kind = c
                .outcome
                .get("kind")
                .and_then(|s| s.as_str())
                .unwrap_or("");
            let detail = c
                .outcome
                .get("detail")
                .and_then(|s| s.as_str())
                .unwrap_or("");
            format!(
                "<tr><td>{name}</td><td><span class=\"k k-{kind_low}\">{kind}</span></td><td>{detail}</td></tr>",
                name = escape(&c.name),
                kind_low = escape(&kind.to_lowercase()),
                kind = escape(kind),
                detail = escape(detail),
            )
        })
        .collect::<String>();
    format!(
        r#"<article class="card">
<h2>Doctor</h2>
<p><strong>{passes}</strong> pass · <strong>{warns}</strong> warn · <strong>{fails}</strong> fail · {skips} skip</p>
<details><summary>Show full check list</summary>
<table class="checks"><thead><tr><th>Check</th><th>State</th><th>Detail</th></tr></thead><tbody>
{checks}
</tbody></table></details>
</article>"#,
        passes = d.doctor.passes,
        warns = d.doctor.warns,
        fails = d.doctor.fails,
        skips = d.doctor.skips,
        checks = checks,
    )
}

/// Logs page with HTMX-style auto-refresh. We avoid the htmx dep — the
/// `<meta refresh>` does the same job for the M3 ship.
pub fn logs_page(unit: &str, lines: usize, body: &str, stub: bool) -> String {
    let inner = format!(
        r#"<section class="card wide">
<h2>Logs — {unit}</h2>
<p class="muted">Last {lines} lines. Auto-refreshes every 5 seconds.</p>
<pre class="logs">{body}</pre>
<p><a href="/app/logs?lines={more}">show more</a></p>
</section>
<meta http-equiv="refresh" content="5">"#,
        unit = escape(unit),
        lines = lines,
        body = escape(body),
        more = (lines + 100).min(1000),
    );
    page("Logs", &inner, stub)
}

/// Provider-keys page: lists configured providers and a rotation form.
pub fn keys_page(providers: &[ProviderEntry], flash: Option<&str>, stub: bool) -> String {
    let flash_html = match flash {
        Some(m) if !m.is_empty() => format!(r#"<div class="flash">{}</div>"#, escape(m)),
        _ => String::new(),
    };
    let rows = providers
        .iter()
        .map(|p| {
            let state = if p.configured { "configured" } else { "not set" };
            format!(
                "<tr><td>{name}</td><td>{id}</td><td>{env}</td><td>{prefix}</td><td>{state}</td></tr>",
                name = escape(&p.display_name),
                id = escape(&p.id),
                env = escape(&p.key_env_var),
                prefix = escape(&p.masked_key_prefix),
                state = state,
            )
        })
        .collect::<String>();

    let options = providers
        .iter()
        .map(|p| {
            format!(
                "<option value=\"{}\">{}</option>",
                escape(&p.id),
                escape(&p.display_name)
            )
        })
        .collect::<String>();

    let body = format!(
        r#"<section class="card wide">
<h2>Provider keys</h2>
{flash}
<table class="providers"><thead>
<tr><th>Provider</th><th>Id</th><th>Env var</th><th>Key prefix</th><th>State</th></tr>
</thead><tbody>
{rows}
</tbody></table>

<h3>Rotate a key</h3>
<form method="post" action="/app/keys/rotate" autocomplete="off">
  <label for="provider">Provider</label>
  <select name="provider" id="provider" required>{options}</select>
  <label for="api_key">New API key</label>
  <input type="password" name="api_key" id="api_key" required minlength="8" autocomplete="off">
  <button type="submit">Rotate</button>
</form>
</section>"#,
        flash = flash_html,
        rows = rows,
        options = options,
    );
    page("Keys", &body, stub)
}

/// Generic error page (used for unknown providers, journal errors, etc.).
pub fn error_page(title: &str, message: &str) -> String {
    let body = format!(
        r#"<section class="card wide">
<h2>{title}</h2>
<p class="error">{message}</p>
<p><a href="/app/dashboard">Back to dashboard</a></p>
</section>"#,
        title = escape(title),
        message = escape(message),
    );
    page("Error", &body, false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escape_handles_all_meta() {
        assert_eq!(escape("a<b>c&d\"e'f"), "a&lt;b&gt;c&amp;d&quot;e&#39;f");
    }

    #[test]
    fn manifest_is_valid_json() {
        let v: serde_json::Value = serde_json::from_str(MANIFEST_JSON).expect("parse manifest");
        assert_eq!(v["start_url"], "/app/dashboard");
        assert_eq!(v["display"], "standalone");
    }

    #[test]
    fn dashboard_renders_stub_banner_when_stubbed() {
        std::env::set_var("DEPUTYPWA_DEV_STUB", "1");
        let d = crate::data::fetch_dashboard();
        let html = dashboard(&d);
        assert!(html.contains("Dev-stub data"));
        std::env::remove_var("DEPUTYPWA_DEV_STUB");
    }

    #[test]
    fn card_device_airgap_badge_and_docs_link_track_network_mode() {
        std::env::set_var("DEPUTYPWA_DEV_STUB", "1");
        // Airgap build: network policy mode=airgap → badge + airgap docs link.
        let mut d = crate::data::fetch_dashboard();
        d.network.mode = "airgap".into();
        let html = card_device(&d);
        assert!(
            html.contains("airgap") && html.contains("Air-gapped"),
            "airgap badge present when mode=airgap"
        );
        assert!(
            html.contains("/docs/concepts/airgap/"),
            "airgap docs link present when mode=airgap"
        );
        assert!(
            html.contains("/docs/reference/schemas/limits-json/"),
            "limits docs link always present"
        );

        // Non-airgap: no badge, no airgap docs link — but the limits link stays.
        let mut d2 = crate::data::fetch_dashboard();
        d2.network.mode = "open".into();
        let html2 = card_device(&d2);
        assert!(
            !html2.contains("/docs/concepts/airgap/"),
            "no airgap docs link in open mode"
        );
        assert!(
            html2.contains("/docs/reference/schemas/limits-json/"),
            "limits docs link present in open mode"
        );
        std::env::remove_var("DEPUTYPWA_DEV_STUB");
    }
}
