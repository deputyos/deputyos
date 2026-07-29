# Add a channel

## What this guide does

Add a new **channel** to a profile — a way for users to talk to the
agent (Slack, Discord, IRC, Matrix, SMS, …). deputyOS ships profile
manifests with the canonical list of channels each upstream agent
supports, the wizard surfaces them as a checkbox list, and per-channel
ufw rules + AppArmor mediations open only the ports the user enabled.

This guide covers two distinct activities, in increasing scope:

1. **Adding a channel deputyOS knows about** — extend the
   `[channels].supported` list in a profile manifest, add per-channel
   ufw rules and AppArmor mediations, optionally update the per-target
   `channels_disabled_by_ram` filter. **This is what deputyOS owns.**
2. **Implementing the channel inside the upstream agent** — writing
   the actual handler that speaks Slack's WebSocket / Discord's
   gateway / Matrix's CS API. **This is upstream's territory** —
   OpenClaw, Hermes, and Khoj each have their own channel
   implementations; deputyOS just turns them on or off.

## Prerequisites

- The upstream agent **already implements** the channel you want to
  add. deputyOS does not implement channels itself — it configures the
  upstream agent. If Khoj doesn't speak Matrix, you can't add Matrix
  to Khoj by editing deputyOS.
- A working contributor checkout — `make doctor` green.
- Familiarity with the profile manifest:
  [Reference → Schemas → profile.toml](../reference/schemas/profile-toml.md).

## The recipe

### 1. Add the channel to `profiles/<id>.toml`

The wizard reads this list and renders a checkbox per entry:

```toml
[channels]
supported = ["web", "telegram", "matrix", ...]
```

Use a stable slug, lowercase, hyphen-separated. The slug shows up:

- In the wizard's channel checklist.
- In `deputyctl status` output.
- In the per-profile `channels.d/` config tree the wizard writes
  during apply.

### 2. Add per-channel ufw rules

The channel may need an inbound port. Edit
`roles/deputyos/templates/ufw.rules.j2` (or the per-profile
ufw drop-in) and add the port. Channels that ride loopback only (e.g.
the web channel — the upstream agent speaks 127.0.0.1 only and the
wizard / desktop / Cloudflare Tunnel proxies it out) need no ufw rule.

Channel-to-port reference (for the channels shipped today):

| Channel | Port | Direction | Notes |
| --- | --- | --- | --- |
| web | 8088 | loopback only | wizard / PWA proxies |
| telegram | n/a | egress only | Bot API is HTTP polling |
| slack | 443 | egress only | Socket Mode (no public URL) |
| discord | 443 | egress only | gateway WebSocket |
| matrix | 443 | egress only | Client-Server API |
| whatsapp-cloud-webhook | 443 | inbound | requires public URL — Cloudflare Tunnel |
| whatsapp-twilio | 443 | inbound | webhook via Twilio |
| obsidian / emacs | 8088 | LAN | LAN-scoped via wizard's gateway_allowlist |
| sms | 443 | inbound | Twilio webhook |
| desktop | 8088 | loopback | local desktop client |

### 3. Add AppArmor mediations

If the channel needs filesystem access beyond what the profile already
allows (uploads, voice files, custom DBs), edit
`roles/deputyos/files/apparmor/deputyos.<profile>` and add the path
allows. Most channels need no AppArmor change — the existing
`network inet stream / inet6 stream` allow covers TCP egress.

### 4. Update per-target limits.json (if necessary)

If the channel is heavy enough to be unsuitable on low-RAM targets
(WhatsApp Cloud webhook is the canonical example — its handler RSS
exceeds 4GB-tier headroom), add it to `channels_disabled_by_ram`:

```json
"capabilities": {
  "channels_disabled_by_ram": ["whatsapp-cloud-webhook", "discord-voice", "<your-new-channel>"]
}
```

The wizard reads `/etc/deputyos/limits.json` and **filters the channel
checklist**: any channel in `channels_disabled_by_ram` is shown as
disabled with a tooltip linking to the limits page.

### 5. Document the upstream agent's per-channel config

The wizard collects channel-specific bits during the channels step
(for example: Telegram bot token, Matrix homeserver URL + access
token). Each goes to `/etc/deputyos/<profile>/channels.d/<channel>.env`
(mode 0600), and the upstream agent's startup reads them.

## Verification

```sh
# Manifest validates
cargo run -p deputyctl -- profile validate profiles/<id>.toml

# Wizard renders the new checkbox
make wizard           # browse to http://localhost:8088/wizard
                      # walk to the channels step

# Bake an image; confirm /etc/deputyos/<profile>/channels.d/.gitkeep is present
make build TARGET=qemu-aarch64 PROFILE=<id>
```

## Worked example: adding `matrix` to Hermes

Hermes already implements Matrix internally. To turn it on at the
deputyOS layer:

### `profiles/hermes.toml` edit

```toml
[channels]
supported = [
  "web", "slack", "telegram", "matrix",   # was: "web", "slack", "telegram"
  "obsidian", "desktop",
]
```

### `deputyctl profile validate`

```sh
cargo run -p deputyctl -- profile validate profiles/hermes.toml
# OK
```

### Wizard now offers Matrix

The user enables Matrix → wizard prompts for homeserver URL + access
token → writes `/etc/deputyos/hermes/channels.d/matrix.env`:

```sh
MATRIX_HOMESERVER_URL=https://matrix.example.org
MATRIX_ACCESS_TOKEN=syt_...
```

Hermes's startup reads this on next restart. No deputyOS code change.

## Channel vs. profile boundary

The clearest mental model: deputyOS is a **packager and policy layer**;
channel implementations live upstream.

- **deputyOS owns**: the channel registry (manifest), the ufw rules,
  the AppArmor mediations, the wizard UI, the limits filter, the
  config writeout location.
- **Upstream agent owns**: the actual handler — Slack WebSocket,
  Telegram polling, Matrix `/sync`, Discord gateway, etc. The handler
  reads its config from `/etc/deputyos/<profile>/channels.d/`.

If you need a channel **the upstream agent does not implement**, you
have two options:

1. Contribute the channel upstream. (Most deputyOS-class agents accept
   channel PRs.)
2. Fork the upstream agent and bake your fork into a custom profile.
   See [How-to → Add a profile](add-a-profile.md).

## Troubleshooting

!!! warning "Channel checkbox renders but is disabled"
    Check `/etc/deputyos/limits.json` — your channel is in
    `channels_disabled_by_ram` for this target. Either upgrade the
    target's RAM tier (e.g. rpi5-8gb instead of rpi4) or remove the
    entry from limits.

!!! warning "User enabled the channel but the agent doesn't respond"
    Almost always a missing or wrong env in
    `/etc/deputyos/<profile>/channels.d/<channel>.env`. Check
    `journalctl -u <profile>-gateway.service` for "missing token" /
    "auth failed" diagnostics. The wizard's review step shows you
    everything it wrote — re-run it if needed.

!!! danger "Inbound channel works only when Cloudflare Tunnel is up"
    This is by design. Channels that need a public URL
    (`whatsapp-cloud-webhook`, `sms`, custom webhook integrations) only
    work when the device has a public ingress. Tailscale, Cloudflare
    Quick Tunnel, or Cloudflare Named Tunnel — the wizard offers all
    three. See [How-to → Set up tunnel](set-up-tunnel.md).

## Related

- [How-to → Add a profile](add-a-profile.md)
- [How-to → Set up tunnel](set-up-tunnel.md)
- [Reference → Schemas → profile.toml](../reference/schemas/profile-toml.md)
- [Reference → Schemas → limits.json](../reference/schemas/limits-json.md)
- [Security → Default-on controls](../security/default-on-controls.md)
