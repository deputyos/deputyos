//! Tiny HTML rendering for the wizard.
//!
//! We deliberately avoid a templating engine (askama/tera/etc.) to keep the
//! binary small and the build cheap. The wizard has six pages, each is a
//! single-purpose form, and the markup is hand-written. Every dynamic value
//! goes through [`escape`] before interpolation.
//!
//! The CSS lives at `static/style.css` and is `include_str!`d into the
//! binary so we ship one file with no filesystem dependency for assets.

use deputyctl::limits::Limits;

use crate::chat::ChatTurn;
use crate::state::{WizardState, TOTAL_STEPS};

pub const STYLE_CSS: &str = include_str!("../static/style.css");

/// HTML-escape a user-controlled string for safe interpolation in attributes
/// or text content.
pub fn escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#x27;"),
            _ => out.push(c),
        }
    }
    out
}

/// Render a complete page using the layout. `body` is the inner HTML.
pub fn page(state: &WizardState, limits: Option<&Limits>, title: &str, body: &str) -> String {
    let total = TOTAL_STEPS;
    let progress_pct = (state.step.index() as f64 / total as f64 * 100.0).round() as u32;
    let limits_panel = limits.map(render_limits_panel).unwrap_or_default();
    format!(
        r#"<!doctype html>
<html lang="en">
<head>
  <meta charset="utf-8">
  <meta name="viewport" content="width=device-width, initial-scale=1">
  <title>{title} — deputyOS wizard</title>
  <link rel="stylesheet" href="/static/style.css">
</head>
<body>
  <header class="bar">
    <h1>deputyOS first-boot wizard</h1>
    <div class="progress" aria-label="Progress: step {step} of {total}">
      <div class="progress-bar" style="width: {pct}%"></div>
      <span>Step {step} of {total}</span>
    </div>
  </header>
  <main class="layout">
    <section id="main" class="content">
      <h2>{title}</h2>
      {body}
    </section>
    <aside class="sidebar">
      <h3>Your device</h3>
      {limits_panel}
    </aside>
  </main>
</body>
</html>
"#,
        title = escape(title),
        step = state.step.index(),
        pct = progress_pct,
    )
}

/// Render the "your device" sidebar from limits.json.
fn render_limits_panel(l: &Limits) -> String {
    let mut s = String::new();
    s.push_str(&format!(
        "<dl><dt>target</dt><dd>{}</dd><dt>tier</dt><dd>{}</dd><dt>RAM</dt><dd>{} MB</dd></dl>",
        escape(&l.target),
        escape(&l.tier),
        l.ram_mb
    ));
    if !l.limitations.is_empty() {
        s.push_str("<h4>Active limitations</h4><ul>");
        for lim in &l.limitations {
            s.push_str(&format!(
                "<li><strong>{}</strong>: {}<br><em>Unblock:</em> {}</li>",
                escape(&lim.id),
                escape(&lim.reason),
                escape(&lim.unblock),
            ));
        }
        s.push_str("</ul>");
    }
    if !l.capabilities.channels_disabled_by_ram.is_empty() {
        s.push_str("<p class=\"warn\">Channels disabled by RAM tier: ");
        s.push_str(&escape(&l.capabilities.channels_disabled_by_ram.join(", ")));
        s.push_str("</p>");
    }
    s
}

/// Step 1: hostname + timezone form.
pub fn step_system(state: &WizardState, limits: Option<&Limits>, error: Option<&str>) -> String {
    let hostname = state.answers.hostname.as_deref().unwrap_or("deputyos");
    let timezone = state.answers.timezone.as_deref().unwrap_or("UTC");
    let err = render_error(error);
    let body = format!(
        r#"<p>Pick a hostname and timezone for this device. You can change either later.</p>
{err}
<form method="post" action="/wizard/system">
  <label>Hostname
    <input name="hostname" value="{hn}" required pattern="[a-z0-9-]+" maxlength="63">
  </label>
  <label>Timezone (IANA, e.g. <code>America/Los_Angeles</code>, <code>Europe/Berlin</code>)
    <input name="timezone" value="{tz}" required>
  </label>
  <div class="actions">
    <button type="submit">Next →</button>
  </div>
</form>"#,
        hn = escape(hostname),
        tz = escape(timezone),
    );
    page(state, limits, "1. System", &body)
}

/// Step 2: profile picker.
pub fn step_profile(
    state: &WizardState,
    limits: Option<&Limits>,
    profiles: &[ProfileChoice],
    error: Option<&str>,
) -> String {
    let err = render_error(error);
    let device_ram = limits.map(|l| l.ram_mb).unwrap_or(0);
    let mut options = String::new();
    for p in profiles {
        let too_low = device_ram > 0 && p.min_ram_mb > device_ram;
        let warn = if too_low {
            format!(
                " <span class=\"warn\">(needs {} MB; device has {} MB — will warn but still allow)</span>",
                p.min_ram_mb, device_ram
            )
        } else {
            String::new()
        };
        let is_selected = state.answers.profile.as_deref() == Some(&p.id)
            || (state.answers.profile.is_none() && p.id == "openclaw");
        let checked = if is_selected { " checked" } else { "" };
        options.push_str(&format!(
            r#"<label class="choice"><input type="radio" name="profile" value="{id}"{checked} required>
              <strong>{name}</strong> <code>{ver}</code>{warn}</label>"#,
            id = escape(&p.id),
            name = escape(&p.display_name),
            ver = escape(&p.pinned_version),
        ));
    }
    let body = format!(
        r#"<p>Pick the agent profile to run. Profiles are pre-installed; switching is reversible.</p>
{err}
<form method="post" action="/wizard/profile">
  {options}
  <div class="actions">
    <a class="back" href="/wizard/system">← Back</a>
    <button type="submit">Next →</button>
  </div>
</form>"#
    );
    page(state, limits, "2. Profile", &body)
}

#[derive(Debug, Clone)]
pub struct ProfileChoice {
    pub id: String,
    pub display_name: String,
    pub pinned_version: String,
    pub min_ram_mb: u32,
}

/// Step 3: provider picker + API key.
pub fn step_provider(
    state: &WizardState,
    limits: Option<&Limits>,
    providers: &[ProviderChoice],
    error: Option<&str>,
    airgap: bool,
) -> String {
    let err = render_error(error);
    let mut options = String::new();
    for p in providers {
        let is_selected = state.answers.provider.as_deref() == Some(&p.id)
            || (state.answers.provider.is_none() && p.default);
        let checked = if is_selected { " checked" } else { "" };
        let key_cell = if p.key_env_var.is_empty() {
            String::new()
        } else {
            format!("<code>{}</code><br>", escape(&p.key_env_var))
        };
        let hint = if p.key_format.is_empty() {
            "local model — no API key, no network"
        } else {
            &p.key_format
        };
        options.push_str(&format!(
            r#"<label class="choice"><input type="radio" name="provider" value="{id}"{checked} required>
              <strong>{name}</strong> {key}<small>{hint}</small></label>"#,
            id = escape(&p.id),
            name = escape(&p.display_name),
            key = key_cell,
            hint = escape(hint),
        ));
    }
    let intro = if airgap {
        r#"<p>This device was baked <strong>air-gapped</strong>, so the model
runs locally — no API key, no network egress. Pick which baked model the
agent should use. You can register more later with
<code>deputyctl model register</code>.</p>"#
    } else {
        r#"<p>Pick a model provider. Your API key is written to
<code>/etc/deputyos/secrets.env</code> (mode 0600) and never leaves the device.
We do a single round-trip request to the provider's <code>/models</code>
endpoint to confirm the key works before persisting it. Tick
<em>Skip validation</em> if you're on a restricted network.</p>"#
    };
    let key_field = if airgap {
        // No API key for local models — send a hidden placeholder so the
        // form still satisfies `required`-free handling in post_provider.
        r#"<input name="api_key" type="hidden" value="">"#
    } else {
        r#"<label>API key
    <input name="api_key" type="password" required>
  </label>
  <label class="choice">
    <input type="checkbox" name="skip_validation" value="1">
    Skip validation (don't try to reach the provider — useful on offline / firewalled networks).
  </label>"#
    };
    let body = format!(
        r#"{intro}
{err}
<form method="post" action="/wizard/provider" autocomplete="off">
  {options}
  {key_field}
  <div class="actions">
    <a class="back" href="/wizard/profile">← Back</a>
    <button type="submit">Next →</button>
  </div>
</form>"#
    );
    page(state, limits, "3. Model provider", &body)
}

#[derive(Debug, Clone)]
pub struct ProviderChoice {
    pub id: String,
    pub display_name: String,
    pub key_env_var: String,
    pub key_format: String,
    /// Pre-select this choice when no provider has been chosen yet. Cloud
    /// providers set this for `openrouter`; airgap choices set it for the
    /// catalog's default model (resolved from the profile's
    /// `[airgap] default_provider` alias).
    pub default: bool,
}

/// Step 4: channel checklist (with limits-driven disabling).
pub fn step_channels(
    state: &WizardState,
    limits: Option<&Limits>,
    supported: &[String],
    disabled: &[String],
    error: Option<&str>,
) -> String {
    let err = render_error(error);
    let selected: std::collections::BTreeSet<&String> = state.answers.channels.iter().collect();
    let mut options = String::new();
    for ch in supported {
        let is_disabled = disabled.contains(ch);
        let checked = if selected.contains(ch) {
            " checked"
        } else {
            ""
        };
        let disabled_attr = if is_disabled { " disabled" } else { "" };
        let suffix = if is_disabled {
            " <span class=\"warn\">(disabled — RAM tier too low for current limits)</span>"
        } else {
            ""
        };
        options.push_str(&format!(
            r#"<label class="choice"><input type="checkbox" name="channels" value="{ch}"{checked}{disabled_attr}>
              <code>{ch}</code>{suffix}</label>"#,
            ch = escape(ch),
        ));
    }
    let body = format!(
        r#"<p>Pick the chat channels to enable. Channels disabled by your RAM tier
are greyed out; selecting them anyway will be rejected with a one-line reason.</p>
{err}
<form method="post" action="/wizard/channels">
  {options}
  <div class="actions">
    <a class="back" href="/wizard/provider">← Back</a>
    <button type="submit">Next →</button>
  </div>
</form>"#
    );
    page(state, limits, "4. Channels", &body)
}

/// Step 5: Network egress policy (M5.5). `hint_mode` is the profile's
/// recommended default (pre-selected radio); `hint_hosts` are suggested
/// allow-list hosts shown as a hint (the live list is seeded from
/// network-defaults.json when the operator picks whitelist).
pub fn step_egress(
    state: &WizardState,
    limits: Option<&Limits>,
    hint_mode: &str,
    hint_hosts: &[String],
) -> String {
    let radio = |value: &str, label: &str| {
        let checked = if value == hint_mode { "checked" } else { "" };
        format!(
            r#"<label><input type="radio" name="egress_mode" value="{value}" {checked} />{label}</label><br/><br/>"#,
        )
    };
    let hosts_hint = if hint_hosts.is_empty() {
        String::new()
    } else {
        format!(
            r#"<p class="hint">Suggested allow-hosts for this profile: <code>{}</code> (seeded automatically on whitelist).</p>"#,
            hint_hosts.join(", ")
        )
    };
    let body = format!(
        r#"<p>Choose how this device reaches the internet.</p>
        <form method="post">
        {open}
        {whitelist}
        {airgap}
        {hosts_hint}
        <p class="hint">You can change this later with <code>deputyctl network</code>.</p>
        <button type="submit">Continue &rarr;</button>
        </form>"#,
        open = radio(
            "open",
            "<strong>Open (recommended)</strong> — unrestricted outbound access. ufw still protects inbound ports."
        ),
        whitelist = radio(
            "whitelist",
            "<strong>Whitelist</strong> — only allow-listed hosts (seeded from your profile). All other outbound traffic is blocked by nftables."
        ),
        airgap = radio(
            "airgap",
            "<strong>Airgap</strong> — local network only (RFC1918 + mDNS). No internet access. Cloud providers will be hidden."
        ),
        hosts_hint = hosts_hint,
    );
    page(state, limits, "5. Egress", &body)
}

/// Step 6: SSH allowlist.
pub fn step_ssh(state: &WizardState, limits: Option<&Limits>, error: Option<&str>) -> String {
    let err = render_error(error);
    let existing = state.answers.ssh_keys.join("\n");
    let body = format!(
        r#"<p>Paste one or more SSH public keys, one per line. SSH on this device is
key-only — no password auth. You can edit this list later via
<code>~agent/.ssh/authorized_keys</code>.</p>
{err}
<form method="post" action="/wizard/ssh">
  <label>Public keys
    <textarea name="ssh_keys" rows="5" required>{val}</textarea>
  </label>
  <div class="actions">
    <a class="back" href="/wizard/channels">← Back</a>
    <button type="submit">Next →</button>
  </div>
</form>"#,
        val = escape(&existing)
    );
    page(state, limits, "5. SSH allowlist", &body)
}

/// Step 6: tailscale.
pub fn step_tailscale(state: &WizardState, limits: Option<&Limits>, error: Option<&str>) -> String {
    let err = render_error(error);
    let body = format!(
        r#"<p>Tailscale gives this device a private mesh address you can reach
from anywhere — no port forwarding, no DNS. <a href="https://tailscale.com" target="_blank" rel="noopener">What is Tailscale?</a></p>
{err}
<form method="post" action="/wizard/tailscale" autocomplete="off">
  <label>Tailscale auth key (optional)
    <input name="authkey" type="password" placeholder="tskey-auth-...">
  </label>
  <p><small>Generate at <code>tailscale.com/admin/settings/keys</code>.
  Saved to <code>/etc/deputyos/secrets.env</code> (mode 0600). Leave empty and click
  <em>Skip</em> to continue without setup.</small></p>
  <div class="actions">
    <a class="back" href="/wizard/ssh">← Back</a>
    <button type="submit" name="skip" value="1" class="secondary">Skip</button>
    <button type="submit">Enable Tailscale →</button>
  </div>
</form>"#
    );
    page(state, limits, "6. Tailscale (remote access)", &body)
}

/// View model for the Account step (M8).
#[derive(Debug)]
pub struct AccountView<'a> {
    /// True if this device is already registered to an deputyOS account.
    pub registered: bool,
    /// A device code awaiting user authorization, if mid-flow.
    pub user_code: Option<&'a str>,
    /// Where the user goes to enter the code (app.deputyos.com/device).
    pub verification_uri: Option<&'a str>,
    /// Self-reported account email (non-secret local label).
    pub email: Option<&'a str>,
    /// Custom/self-hosted backend API base URL chosen for this device. None
    /// (or empty) means "use the production deputyOS backend".
    pub api_base: Option<&'a str>,
    /// An inline status/error message to show above the form.
    pub note: Option<&'a str>,
    /// Whether `note` is an error (red) or informational.
    pub note_is_error: bool,
}

/// Step 7: account (M8) — register this device against an deputyOS account.
/// Optional per the M8 hard rule: "Skip" advances with no account and no
/// tokens, and the rest of the appliance works unchanged.
pub fn step_account(state: &WizardState, limits: Option<&Limits>, view: &AccountView) -> String {
    let note = match view.note {
        Some(n) if view.note_is_error => format!(r#"<p class="error">{}</p>"#, escape(n)),
        Some(n) => format!("<p>{}</p>", escape(n)),
        None => String::new(),
    };
    let email_val = escape(view.email.unwrap_or(""));
    let api_base_val = escape(view.api_base.unwrap_or(""));
    // Hidden carry-along so the begin→poll cycle preserves a custom backend
    // choice on the code-pending + registered re-renders (where the visible
    // input isn't shown).
    let api_base_hidden =
        format!(r#"<input type="hidden" name="api_base" value="{api_base_val}">"#);
    // Visible custom-backend field, shown only on the initial (not-registered,
    // no-code) form so a first-boot user can point the device at a self-hosted
    // deputyOS API before starting the device-code flow.
    let custom_backend_block = format!(
        r#"<details><summary>Custom backend (self-hosted / on-prem)</summary>
  <p><small>Point this device at a different deputyOS API — e.g. a self-hosted
  instance — instead of <code>api.deputyos.com</code>. Leave blank for the
  default. Saved to <code>/etc/deputyos/api-base</code> at registration so the
  integrated tunnel and remote command poller use the same backend.</small></p>
  <label>API base URL
    <input name="api_base" type="url" value="{api_base_val}" placeholder="https://deputyos.example.internal">
  </label>
</details>"#,
    );
    let body = if view.registered {
        format!(
            r#"<p>This device is <strong>registered</strong> to an deputyOS account{email_label}.
That unlocks the integrated tunnel (next step) and managed encrypted
backup/restore — no Cloudflare account, no port forwarding.</p>
{note}
<form method="post" action="/wizard/account" autocomplete="off">
  <input type="hidden" name="email" value="{email_val}">
  {api_base_hidden}
  <div class="actions">
    <a class="back" href="/wizard/tailscale">← Back</a>
    <button type="submit" name="action" value="begin" class="secondary">Use a different account</button>
    <button type="submit" name="action" value="continue">Continue →</button>
  </div>
</form>"#,
            email_label = view
                .email
                .map(|e| format!(" ({})", escape(e)))
                .unwrap_or_default(),
            note = note,
            email_val = email_val,
            api_base_hidden = api_base_hidden,
        )
    } else if let (Some(code), Some(uri)) = (view.user_code, view.verification_uri) {
        format!(
            r#"<p>Open <a href="{uri}" target="_blank" rel="noopener">{uri}</a> and enter this
code to sign in, then come back and click <em>Continue</em>:</p>
<p><code class="device-code">{code}</code></p>
<p><small>The code expires in 15 minutes.</small></p>
{note}
<form method="post" action="/wizard/account" autocomplete="off">
  <input type="hidden" name="email" value="{email_val}">
  {api_base_hidden}
  <div class="actions">
    <a class="back" href="/wizard/tailscale">← Back</a>
    <button type="submit" name="action" value="cancel" class="secondary">Cancel</button>
    <button type="submit" name="action" value="skip" class="secondary">Skip</button>
    <button type="submit" name="action" value="poll">I've authorized — Continue →</button>
  </div>
</form>"#,
            uri = escape(uri),
            code = escape(code),
            note = note,
            email_val = email_val,
            api_base_hidden = api_base_hidden,
        )
    } else {
        format!(
            r#"<p>An deputyOS account is optional. With one, this device gets an
integrated tunnel (a stable public URL) and managed encrypted backup/restore —
no Cloudflare account, no port forwarding. You can skip and set it up later;
the appliance works fully without an account.</p>
{note}
<form method="post" action="/wizard/account" autocomplete="off">
  <label>Account email (optional label)
    <input name="email" type="email" value="{email_val}" placeholder="you@example.com">
  </label>
  {custom_backend_block}
  <div class="actions">
    <a class="back" href="/wizard/tailscale">← Back</a>
    <button type="submit" name="action" value="skip" class="secondary">Skip — set up later</button>
    <button type="submit" name="action" value="begin">Sign in with deputyOS account →</button>
  </div>
</form>"#,
            note = note,
            email_val = email_val,
            custom_backend_block = custom_backend_block,
        )
    };
    page(state, limits, "7. Account (optional)", &body)
}

/// Step 8: cloudflare-tunnel. When the device is registered to an account
/// (M8), the integrated tunnel is the recommended path — it reuses the
/// account's tunnel token for a stable public URL with no Cloudflare account.
/// The cloudflared quick/named options remain under "Advanced".
pub fn step_cloudflare_tunnel(
    state: &WizardState,
    limits: Option<&Limits>,
    registered: bool,
    error: Option<&str>,
) -> String {
    let err = render_error(error);
    let intro = if registered {
        r#"<p>Your device is registered to an deputyOS account, so the
<strong>integrated tunnel</strong> is recommended — it reuses your account's
tunnel token for a stable public URL
(<code>https://&lt;account&gt;.deputyos.com</code>), with no Cloudflare
account and no inbound ports. Run it with <code>deputyctl tunnel --integrated</code>.</p>"#
    } else {
        r#"<p>Cloudflare Tunnel exposes this agent on a public URL without
opening any inbound ports. Pick a tunnel kind, or skip. (Register an account
on the previous step to use the integrated tunnel instead — no Cloudflare
account needed.)</p>"#
    };
    let integrated_radio = if registered {
        r#"<label class="choice">
    <input type="radio" name="choice" value="integrated" checked>
    <strong>Integrated tunnel</strong> (recommended) — stable public URL via your
    account; run <code>deputyctl tunnel --integrated</code>.
  </label>"#
    } else {
        ""
    };
    let skip_checked = if registered { "" } else { "checked" };
    let body = format!(
        r#"{intro}
{err}
<form method="post" action="/wizard/cloudflare-tunnel" autocomplete="off">
  {integrated_radio}
  <details><summary>Advanced — Cloudflare Tunnel (cloudflared)</summary>
  <label class="choice">
    <input type="radio" name="choice" value="skip" {skip_checked}>
    <strong>Skip</strong> — no public URL.
  </label>
  <label class="choice">
    <input type="radio" name="choice" value="quick">
    <strong>Quick Tunnel</strong> — auto-assigned <code>*.trycloudflare.com</code> URL, no Cloudflare account.
  </label>
  <label class="choice">
    <input type="radio" name="choice" value="named">
    <strong>Named Tunnel</strong> — paste credentials JSON from <code>cloudflared tunnel create</code>.
  </label>
  <label>Credentials JSON (named only)
    <textarea name="credentials" rows="5" placeholder='{{"AccountTag":"...","TunnelID":"...","TunnelName":"my-agent","TunnelSecret":"..."}}'></textarea>
  </label>
  </details>
  <div class="actions">
    <a class="back" href="/wizard/account">← Back</a>
    <button type="submit">Next →</button>
  </div>
</form>"#,
        intro = intro,
        err = err,
        integrated_radio = integrated_radio,
        skip_checked = skip_checked,
    );
    page(state, limits, "8. Public URL (tunnel)", &body)
}

/// Step 8: backup destination.
pub fn step_backup(state: &WizardState, limits: Option<&Limits>, error: Option<&str>) -> String {
    let err = render_error(error);
    let body = format!(
        r#"<p>Configure encrypted, quiesced daily backups. A separate recovery
key is created locally so account-token rotation cannot lock you out. Export
it after setup with <code>deputyctl backup recovery-key export</code>.</p>
{err}
<form method="post" action="/wizard/backup" autocomplete="off">
  <label class="choice">
    <input type="radio" name="kind" value="skip" checked>
    <strong>Skip</strong> — no remote backup; data lives on-device only.
  </label>
  <label class="choice">
    <input type="radio" name="kind" value="managed">
    <strong>deputyOS managed backup</strong> — included with Business and Enterprise; encrypted before upload.
  </label>
  <label class="choice">
    <input type="radio" name="kind" value="b2">
    <strong>Backblaze B2</strong>
  </label>
  <label class="choice">
    <input type="radio" name="kind" value="r2">
    <strong>Cloudflare R2</strong>
  </label>
  <label class="choice">
    <input type="radio" name="kind" value="s3">
    <strong>S3-compatible (custom)</strong>
  </label>
  <fieldset><legend>B2 fields</legend>
    <label>Account ID <input name="b2_account_id"></label>
    <label>Application key <input name="b2_application_key" type="password"></label>
    <label>Bucket <input name="b2_bucket"></label>
  </fieldset>
  <fieldset><legend>R2 fields</legend>
    <label>Account ID <input name="r2_account_id"></label>
    <label>Access key <input name="r2_access_key" type="password"></label>
    <label>Secret key <input name="r2_secret_key" type="password"></label>
    <label>Bucket <input name="r2_bucket"></label>
  </fieldset>
  <fieldset><legend>S3 fields</legend>
    <label>Endpoint URL <input name="s3_endpoint" placeholder="https://s3.us-west-2.amazonaws.com"></label>
    <label>Access key <input name="s3_access_key" type="password"></label>
    <label>Secret key <input name="s3_secret_key" type="password"></label>
    <label>Bucket <input name="s3_bucket"></label>
  </fieldset>
  <div class="actions">
    <a class="back" href="/wizard/cloudflare-tunnel">← Back</a>
    <button type="submit">Next →</button>
  </div>
</form>"#
    );
    page(state, limits, "8. Encrypted backups", &body)
}

/// Drives step (M3.5, hybrid). Surfaces detected/configured mounts and the
/// active profile's suggested paths, links to the standalone `/mounts` page
/// for add/revoke, and asks the user to acknowledge before moving on. It does
/// NOT mutate policy in the step machine — mounts are live-mutable, not a
/// one-shot wizard choice — so this step only flips `drives_acknowledged`.
pub fn step_drives(
    state: &WizardState,
    limits: Option<&Limits>,
    entries: &[deputyctl::mounts::ListEntry],
    suggested: &[String],
    default_mode: &str,
    error: Option<&str>,
) -> String {
    let err = render_error(error);
    let rows = if entries.is_empty() {
        r#"<tr><td colspan="5" class="muted">No mounts configured yet — that's fine, you can add them any time from /mounts.</td></tr>"#
            .to_string()
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
</tr>"#,
                    kind = escape(&e.kind),
                    id = escape(&e.id),
                    guest = escape(&e.guest_path),
                    mode = escape(&e.mode),
                    source = escape(&e.source),
                )
            })
            .collect::<String>()
    };
    let suggested_html = if suggested.is_empty() {
        r#"<p class="muted">This profile suggests no specific paths. Mount whatever you like under <code>/mnt/deputyos/</code>.</p>"#
            .to_string()
    } else {
        let items = suggested
            .iter()
            .map(|p| format!("<li><code>{}</code></li>", escape(p)))
            .collect::<String>();
        format!(
            r#"<p>This profile suggests sharing (default mode <code>{mode}</code>):</p>
<ul class="suggested">{items}</ul>"#,
            mode = escape(default_mode),
            items = items,
        )
    };
    let body = format!(
        r#"<p>Where can the agent see your files? Every mount must live under
<code>/mnt/deputyos/</code> so AppArmor can confine it per profile. Removable
USB drives are auto-mounted read-only with <code>nosuid,nodev,noexec</code>;
encrypted (LUKS) devices are refused. Network shares (SMB/NFS) keep their
credentials in <code>/etc/deputyos/secrets.env</code> — never in the policy
file.</p>
{err}
{suggested_html}
<h3>Configured mounts</h3>
<table>
<thead><tr><th>Kind</th><th>Id</th><th>Guest path</th><th>Mode</th><th>Source</th></tr></thead>
<tbody>
{rows}
</tbody>
</table>
<p>To add a host folder or an SMB/NFS share, or to revoke one, open
<a href="/mounts"><strong>the mounts page</strong></a> — it stays available
after the wizard finishes, so you can plug drives in any time.</p>
<form method="post" action="/wizard/drives" autocomplete="off">
  <div class="actions">
    <a class="back" href="/wizard/backup">← Back</a>
    <button type="submit">Continue →</button>
  </div>
</form>"#,
        err = err,
        suggested_html = suggested_html,
        rows = rows,
    );
    page(state, limits, "11. Drives", &body)
}

/// Final step: review + apply.
pub fn step_review(state: &WizardState, limits: Option<&Limits>) -> String {
    let a = &state.answers;
    let channels = if a.channels.is_empty() {
        "(none)".to_string()
    } else {
        a.channels.join(", ")
    };
    let ssh_count = a.ssh_keys.len();
    let tailscale = if a.tailscale_enabled {
        "enabled"
    } else {
        "skipped"
    };
    let cf = a.cloudflare_tunnel_choice.as_deref().unwrap_or("skip");
    let backup = a.backup_kind.as_deref().unwrap_or("skip");
    let body = format!(
        r#"<p>Review your answers. Clicking <strong>Apply</strong> writes to
<code>/etc/deputyos/</code>, configures ufw, and starts the profile.</p>
<dl class="review">
  <dt>Hostname</dt><dd>{hn}</dd>
  <dt>Timezone</dt><dd>{tz}</dd>
  <dt>Profile</dt><dd>{prof}</dd>
  <dt>Provider</dt><dd>{prov}</dd>
  <dt>Channels</dt><dd>{ch}</dd>
  <dt>SSH keys</dt><dd>{ssh} key(s)</dd>
  <dt>Tailscale</dt><dd>{tailscale}</dd>
  <dt>Cloudflare Tunnel</dt><dd>{cf}</dd>
  <dt>Backup</dt><dd>{backup}</dd>
</dl>
<form method="post" action="/wizard/review/apply">
  <div class="actions">
    <a class="back" href="/wizard/drives">← Back</a>
    <button type="submit" class="apply">Apply</button>
  </div>
</form>"#,
        hn = escape(a.hostname.as_deref().unwrap_or("(unset)")),
        tz = escape(a.timezone.as_deref().unwrap_or("(unset)")),
        prof = escape(a.profile.as_deref().unwrap_or("(unset)")),
        prov = escape(a.provider.as_deref().unwrap_or("(unset)")),
        ch = escape(&channels),
        ssh = ssh_count,
        tailscale = escape(tailscale),
        cf = escape(cf),
        backup = escape(backup),
    );
    page(state, limits, "Review", &body)
}

/// Render the chat page.
pub fn page_chat(state: &WizardState, limits: Option<&Limits>, history: &[ChatTurn]) -> String {
    let mut hist = String::new();
    for turn in history {
        hist.push_str(&render_turn(turn));
    }
    let body = format!(
        r##"<p>Talk to the active agent here. Messages are stored locally in
<code>chat-history.jsonl</code> under the profile's data dir. This is a
fallback for the period before you wire up Telegram/Slack/etc. from
<a href="/wizard/channels">/wizard/channels</a>.</p>
<div id="messages" class="chat-history">
  {hist}
</div>
<form hx-post="/chat/message" hx-target="#messages" hx-swap="innerHTML"
      method="post" action="/chat/message" autocomplete="off">
  <label>Your message
    <textarea name="message" rows="3" required></textarea>
  </label>
  <div class="actions">
    <button type="submit">Send</button>
  </div>
</form>
<script src="https://unpkg.com/htmx.org@1.9.12"
        integrity="sha384-ujb1lZYygJmzgSwoxRggbCHcjc0rB2XoQrxeTUQyRjrOnlCoYta87iKBWq3EsdM2"
        crossorigin="anonymous"></script>"##
    );
    page(state, limits, "Chat", &body)
}

/// Standalone /mounts page. Lives outside the linear step machine so the
/// wizard renders a non-step shell. M3.5.
pub fn page_mounts(
    limits: Option<&Limits>,
    entries: &[deputyctl::mounts::ListEntry],
    flash: Option<&str>,
) -> String {
    let flash_html = flash
        .map(|f| format!(r#"<div class="banner">{}</div>"#, escape(f)))
        .unwrap_or_default();

    let rows = if entries.is_empty() {
        r#"<tr><td colspan="6" class="muted">No mounts configured yet.</td></tr>"#.to_string()
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
<form method="post" action="/mounts/remove">
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
        r##"{flash}
<p>Drives + shares the agent can see. Every mount must live under
<code>/mnt/deputyos/</code> so AppArmor's per-profile rules can confine it.
Network shares store credentials in <code>/etc/deputyos/secrets.env</code>; the
policy file never sees them.</p>

<h3>Configured mounts</h3>
<table>
<thead><tr><th>Kind</th><th>Id</th><th>Guest path</th><th>Mode</th><th>Source</th><th></th></tr></thead>
<tbody>
{rows}
</tbody>
</table>

<h3>Add a host-FS share</h3>
<form method="post" action="/mounts" class="stacked">
<label>Id <input name="id" required pattern="[a-zA-Z0-9_-]+" placeholder="documents"></label>
<label>Host path <input name="host_path" required placeholder="/home/me/Documents"></label>
<label>Guest path <input name="guest_path" required pattern="/mnt/deputyos/.*" placeholder="/mnt/deputyos/documents"></label>
<label>Mode
<select name="mode">
<option value="ro" selected>read-only</option>
<option value="rw">read-write</option>
</select>
</label>
<div class="actions"><button type="submit">Add mount</button></div>
</form>

<h3>Add a network share (SMB / NFS) — advanced</h3>
<p><small>For SMB/CIFS, put the credentials in <code>/etc/deputyos/secrets.env</code> as
<code>&lt;KEY&gt;=username:password</code> (or <code>username</code> + <code>password</code> lines) and
name that key below. NFS usually needs no credentials. The policy file never
stores secrets.</small></p>
<form method="post" action="/mounts/network-add" class="stacked">
<label>Id <input name="id" required pattern="[a-zA-Z0-9_-]+" placeholder="nas-photos"></label>
<label>Kind
<select name="kind">
<option value="cifs">SMB / CIFS</option>
<option value="nfs">NFS</option>
</select>
</label>
<label>Source <input name="source" required placeholder="//nas.lan/photos (SMB) or nas.lan:/srv/photos (NFS)"></label>
<label>Guest path <input name="guest_path" required pattern="/mnt/deputyos/.*" placeholder="/mnt/deputyos/nas-photos"></label>
<label>Mode
<select name="mode">
<option value="ro" selected>read-only</option>
<option value="rw">read-write</option>
</select>
</label>
<label>Credentials env key (SMB only; optional) <input name="credentials_env" placeholder="NAS_PHOTOS_CREDS"></label>
<div class="actions"><button type="submit">Add network share</button></div>
</form>"##,
        flash = flash_html,
        rows = rows,
    );
    // Reuse page chrome but synthesise a minimal WizardState so the layout
    // doesn't leak step-progression UI into a non-step page.
    let dummy_state = WizardState::default();
    page(&dummy_state, limits, "Mounts", &body)
}

pub fn render_chat_messages(history: &[ChatTurn]) -> String {
    let mut s = String::new();
    for turn in history {
        s.push_str(&render_turn(turn));
    }
    s
}

fn render_turn(turn: &ChatTurn) -> String {
    let role_class = match turn.role.as_str() {
        "user" => "turn-user",
        "assistant" => "turn-assistant",
        _ => "turn-system",
    };
    format!(
        r#"<div class="turn {cls}"><strong>{role}:</strong> {content}</div>"#,
        cls = role_class,
        role = escape(&turn.role),
        content = escape(&turn.content),
    )
}

/// Done page.
pub fn page_done(state: &WizardState, limits: Option<&Limits>, mode: &str) -> String {
    let body = format!(
        r#"<p class="success">Setup complete ({mode} mode).</p>
<ul>
  <li>Open the chat UI at <a href="/chat">/chat</a> <em>(M3-rest stub)</em>.</li>
  <li>Inspect the running profile: <code>deputyctl status</code></li>
  <li>Re-run the device report: <code>deputyctl doctor</code></li>
</ul>
<p>You can close this tab.</p>"#
    );
    page(state, limits, "All set", &body)
}

/// Inline error block.
fn render_error(error: Option<&str>) -> String {
    match error {
        Some(e) => format!(r#"<p class="error">{}</p>"#, escape(e)),
        None => String::new(),
    }
}

/// Plain 401 page.
pub fn page_unauthorized() -> String {
    r#"<!doctype html><html><head><meta charset="utf-8"><title>401</title></head>
<body><h1>401 Unauthorized</h1>
<p>This wizard is gated by a single-use token. Re-launch <code>deputyctl init</code> on the device to get a fresh URL.</p>
</body></html>"#
        .into()
}
