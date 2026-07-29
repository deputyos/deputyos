# 07 — Networking

A semi-technical user should not have to know about firewalls, mDNS, or NAT to run the device. This doc describes the network surface deputyOS exposes, the defaults that protect new users, and the optional features that let advanced users put the agent anywhere.

## Local network model

By default the device is a LAN appliance:

| Surface | Where | Default state |
|---|---|---|
| `deputyos.local` (mDNS) | LAN | published |
| `:22` SSH | LAN | open, key-only |
| `:8088` wizard | LAN | open during first-boot wizard, off thereafter |
| `:80` web (PWA + chat) | LAN | open |
| `:443` web (PWA + chat over TLS, self-signed) | LAN | open |
| Channel ports (e.g. `:8080` gateway, `:8443` callback) | LAN | open only when channel enabled |
| Outbound WAN | model provider, B2/R2, optional Tailscale/CFT | as configured |

## Authenticated deputyOS tunnel

After account registration, `deputyos-tunnel.service` maintains one outbound
WebSocket to the deputyOS API. It opens no inbound guest port and reconnects
automatically. The relay validates the account JWT, checks device ownership,
then gives each surface a separate wildcard-DNS origin:

| Origin | Guest target |
|---|---|
| `<device>.tunnel.deputyos.com` | Active profile's native WebUI |
| `control-<device>.tunnel.deputyos.com` | deputyOS system UI on loopback `:8088` |
| `terminal-<device>.tunnel.deputyos.com` | Audited, non-root terminal on loopback `:8090` |

Separate origins are required because native agent UIs commonly use
root-relative assets and WebSockets. The older
`/api/v1/tunnel/proxy/<device>/...` route remains available for local
development, but production clients use the wildcard origins.

The guest accepts only the three compiled-in services above. A relay request
cannot select an arbitrary hostname or port. The terminal additionally
requires a one-use ticket, valid for five minutes and minted when its landing
page is opened. Terminal sessions run as `agent`; correlated start/end records
are written to both the cloud relay and guest system journals, outside the
shell user's write access.

`ufw` is `default-deny` for inbound; allow rules are added per-channel by the wizard.

## mDNS / `deputyos.local`

On boot, `avahi-daemon` (or the Pi-flavoured `mdns-publisher`) advertises:

- `deputyos.local` → device IP
- `_http._tcp` on `:80`
- `_https._tcp` on `:443`

Most home networks pass mDNS by default. Some enterprise / locked-down networks block it; in that case the wizard prints the device's IP on HDMI and the QR code embeds the IP-form URL. There is no DNS dependency.

When multiple deputyOS devices share a LAN, each picks `deputyos-<hostname>.local` — the wizard nudges the user to set a unique hostname during init.

## SSH

- `agent` is the only login user.
- Password auth is disabled. Root login is denied.
- Authorized keys come from `/boot/firmware/deputyos.yaml` at first boot (parsed once, written to `~agent/.ssh/authorized_keys`) and from any keys added via `deputyctl ssh add-key`.
- `fail2ban` watches `/var/log/auth.log` and bans IPs that fail auth ≥3 times in 10 minutes for 1 hour.
- LAN-only `Match Address` blocks reduce surface — SSH is only reachable on RFC1918 ranges by default. To expose SSH to the WAN, the user must opt in via `deputyctl ssh expose-wan` (which prints a clear warning and bumps `fail2ban` parameters).

## Channel exposure

OpenClaw and Hermes serve some channels by *connecting outward* (Telegram polling, Discord gateway, Slack RTM) and others by *receiving callbacks* (WhatsApp Cloud, Slack Events API webhook, Twilio SMS). The two require different network paths.

| Mode | Examples | Network requirement |
|---|---|---|
| Outbound polling / WS | Telegram bot, Discord, Slack RTM, Matrix client | Outbound HTTPS only — works behind any NAT |
| Inbound webhook | WhatsApp Cloud, Slack Events API, Twilio SMS, Stripe | A public URL the channel can reach |

For inbound-webhook channels, deputyOS offers three paths:

1. **Cloudflare Tunnel** (recommended for non-technical users) — `deputyctl tunnel install` runs `cloudflared` as a service and registers a stable public hostname. No port-forwarding; works behind CGNAT.
2. **Tailscale Funnel** — if the user is on Tailscale, `tailscale funnel 443` exposes the gateway on a `*.ts.net` URL.
3. **Manual port-forward** — for users with a static IP who want to expose `:8443` directly. The wizard discourages this for non-technical users.

ufw rules are written by the wizard and are minimal: `allow from any to any port <channel-port>` only for channels actually configured, scoped to the chosen exposure path (Tailscale CGNAT, Cloudflare Tunnel egress, or `any`).

## Outbound — what calls home

deputyOS does not phone home for telemetry or analytics. The only outbound calls in normal operation are:

| Destination | Why | Frequency |
|---|---|---|
| Model provider API | every chat round-trip | message-driven |
| Channel APIs (Telegram, etc.) | per channel choice | message-driven |
| `cdn.deputyos.com` | signed automatic update cycle | daily, with randomized delay |
| User's B2/R2 bucket | `deputyctl backup` | per backup schedule |
| NTP pool | clock sync | per OS default |
| Skill mirror in project bucket | when user adds a skill | on-demand |

OpenTelemetry traces (M6+) are opt-in; if enabled they go to a user-configurable OTLP endpoint, not back to us.

## Tailscale

Optional but well-supported. `tailscale` is preinstalled (binary) but **not** running. The wizard offers a one-off auth key field; if provided, `tailscaled` starts and the device joins the user's tailnet. ufw is updated to allow the tailnet CGNAT range.

After joining, the user can:

- SSH via the device's tailnet IP from anywhere.
- Reach the web UI / chat / PWA over the tailnet (no separate exposure setup).
- Use Tailscale Funnel for webhook channels (alternative to Cloudflare Tunnel).

`deputyctl ts status` shows the device's tailscale IP and online status; `deputyctl ts disable` stops `tailscaled` and removes the firewall allowance.

## Cloudflare Tunnel

Also optional. `cloudflared` is preinstalled but **not** running. `deputyctl tunnel install` launches the device-code OAuth flow against Cloudflare, mints a tunnel, and writes the credentials to `/etc/cloudflared/`. From then on, `deputyos-<hostname>.<your-cf-domain>` resolves to the device.

Quick share: `deputyctl tunnel quick` opens an ephemeral `*.trycloudflare.com` URL — useful to share access with a friend without long-lived setup.

## WiFi

- NetworkManager handles WiFi (and ethernet).
- Pre-shared key configured via `/boot/firmware/deputyos.yaml` or the wizard.
- WiFi power-save is disabled by default — `iwconfig wlan0 power off` is set as a systemd oneshot at boot. (This solves the documented "WiFi drops every few minutes on Pi" pain.)
- For enterprise WPA-EAP, the wizard walks the user through configuring `wpa_supplicant.conf`; pre-fill via `deputyos.yaml` is also supported.

## IPv6

Enabled. `ufw` rules apply to v6 too. SLAAC is the default; static v6 is configurable.

## DNS

Systemd-resolved with the upstream DNS the user configured (default: ISP via DHCP). The wizard offers Cloudflare 1.1.1.1, Quad9 9.9.9.9, or "use DHCP" as a quick toggle. DoT is opt-in.

## Firewall summary

`deputyctl doctor` verifies:

- `ufw status` is `active`.
- Default policies are `deny incoming`, `allow outgoing`, `deny routed`.
- Only the channels the user enabled have rules.
- Tailscale CGNAT range has rules iff `tailscaled` is running.
- `:22` is restricted to LAN unless `expose-wan` was run.
