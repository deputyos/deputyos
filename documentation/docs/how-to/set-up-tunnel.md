# Set up a tunnel (Tailscale or Cloudflare)

## What this guide does

Open up your deputyOS device to the internet — securely — so you can
reach the wizard, the PWA, and inbound webhook channels (WhatsApp
Cloud, SMS) from outside the LAN. deputyOS bakes both options:

- **Tailscale** — WireGuard mesh; private, zero-config, key-based.
- **Cloudflare Tunnel** — public HTTPS URL via Cloudflare's edge.
  Two flavours: Quick Tunnel (anonymous, ephemeral
  `*.trycloudflare.com` URL) and Named Tunnel (your own domain, durable).

Both binaries are baked into every non-cloud image
(`networking-baseline.yml` installs `tailscale` and `cloudflared` but
does not start them). The wizard collects auth + flips the switches;
`deputyctl tunnel` runs an ad-hoc Quick Tunnel from the CLI.

## Prerequisites

- A baked deputyOS image with the wizard finished.
- For Tailscale: an account at `tailscale.com`. Generate an auth key
  at `tailscale.com/admin/settings/keys`.
- For Cloudflare Named Tunnel: a Cloudflare account, a domain on
  Cloudflare, the `cloudflared` CLI authed once on a workstation
  (`cloudflared tunnel login`).

## Tailscale

### Path 1: during the wizard

The wizard's Tailscale step (route `/wizard/tailscale`) prompts for an
auth key. Format check is "longer than 30 chars" — bogus keys are
rejected with "Auth key looks too short. Generate one at
tailscale.com/admin/settings/keys."

The apply step runs:

```sh
tailscale up --auth-key="$TAILSCALE_AUTHKEY"
```

After apply, your device has a `100.x.x.x` IP visible in your
Tailscale admin panel as `deputyos-<hostname>`. The wizard can then be
reached at `http://100.x.x.x:8088/` from any other Tailscale-connected
machine.

### Path 2: post-wizard

```sh
sudo tailscale up --auth-key=<key>
```

Same effect. The wizard's "Tailscale enabled" indicator updates on
next refresh.

### Verification

```sh
# 1. The interface exists
ip addr show tailscale0

# 2. The connection is up
sudo tailscale status

# 3. Wizard is reachable from a Tailnet peer
curl http://<tailscale-ip>:8088/healthz
```

## Cloudflare Tunnel — three modes

The wizard's Cloudflare step (route `/wizard/cloudflare-tunnel`) offers
three modes:

### Mode 1: skip

Default. No public ingress. Use Tailscale or LAN only.

### Mode 2: Quick Tunnel

Anonymous, ephemeral. The wizard's apply step runs:

```sh
cloudflared tunnel --url http://localhost:8088
```

`cloudflared` prints a `https://*.trycloudflare.com` URL — the wizard
captures it and writes it to `/run/deputyos/cloudflared-url` so the PWA
and `deputyctl status` can surface it.

The URL is **rotation-prone**: every cloudflared restart picks a new
hostname. Suitable for one-off demos, never for production.

### Mode 3: Named Tunnel

Durable. The wizard prompts for:

- The named-tunnel JSON credential (paste the output of
  `cloudflared tunnel create deputyos` from your workstation).
- The hostname to route — `agent.example.com`.

The apply step writes:

- `/etc/cloudflared/config.yml`:

    ```yaml
    tunnel: <tunnel-uuid>
    credentials-file: /etc/cloudflared/<tunnel-uuid>.json
    ingress:
      - hostname: agent.example.com
        service: http://localhost:8088
      - service: http_status:404
    ```

- The credentials JSON.
- An enabled `cloudflared.service` systemd unit.

Your `agent.example.com` DNS record (CNAME to
`<tunnel-uuid>.cfargotunnel.com`) needs to land via your Cloudflare
DNS — the wizard does **not** mutate your DNS zone for you.

## `deputyctl tunnel` — one-off CLI Quick Tunnel

For a SSH session where you want to reach the wizard from a phone:

```sh
sudo deputyctl tunnel
```

This runs `cloudflared tunnel --url http://localhost:8088` in the
foreground. The published URL is printed to stdout once
(`https://*.trycloudflare.com`); the rest of cloudflared's stderr is
proxied through.

Background mode (write the PID to `/run/deputyos/cloudflared.pid`):

```sh
sudo deputyctl tunnel --background
```

Pick a different port:

```sh
sudo deputyctl tunnel --port 8089           # PWA instead of wizard
```

Default port is `8088` (the wizard).

## Where the binaries live

| Binary | Path | Installed by |
| --- | --- | --- |
| `tailscale` | `/usr/bin/tailscale` | `networking-baseline.yml` (apt) |
| `tailscaled` | `/usr/sbin/tailscaled` | same |
| `cloudflared` | `/usr/local/bin/cloudflared` | `networking-baseline.yml` (downloads upstream binary; pinned SHA) |

For cloud variants (`fly-machines`, `digitalocean`, `oracle-arm-free`,
`hetzner-cloud`, `vultr`, `linode`), tunnels rarely make sense — the
provider already gives you a public IP. The role still installs the
binaries (small footprint, harmless) but leaves them disabled.

## Verification

```sh
# Tailscale
sudo tailscale status
sudo tailscale ip -4

# Cloudflare Quick Tunnel (one-off)
sudo deputyctl tunnel
# Then visit the printed URL.

# Cloudflare Named Tunnel
sudo systemctl status cloudflared
curl https://agent.example.com/healthz
```

## Troubleshooting

!!! warning "Tailscale up exits with `Logged out`"
    Your auth key has expired or was reused beyond its limit. Generate
    a fresh one at `tailscale.com/admin/settings/keys` (look for
    "reusable" or "ephemeral" depending on intent).

!!! warning "Quick Tunnel URL changes after each reboot"
    Expected — Quick Tunnels are anonymous and ephemeral. Use a Named
    Tunnel for stable URLs, or Tailscale for a stable private IP.

!!! warning "Named Tunnel works but `agent.example.com` 522s"
    Your DNS CNAME is missing or wrong. The CNAME target is
    `<tunnel-uuid>.cfargotunnel.com`. Confirm with
    `dig agent.example.com` from outside your network.

!!! danger "Quick Tunnel exposes the wizard to the internet without auth"
    The wizard's `__Host-deputyos-session` cookie is set during the
    initial setup; if you started a Quick Tunnel **before** completing
    the wizard, anyone hitting the URL can finish the wizard for you.
    Either complete the wizard locally first (LAN, Tailscale) or set
    a temporary auth token via the wizard's `--token` flag.

!!! tip "Use Tailscale for personal access, Named Tunnel for inbound webhooks"
    Tailscale is the right shape for "I want to reach my agent from my
    phone." Named Tunnel is the right shape for "WhatsApp's webhook
    needs to POST to me." Run both — they don't conflict.

## Related

- [Reference → CLI → deputyctl](../reference/cli/deputyctl.md) (`tunnel` subcommand)
- [Reference → APIs → wizard HTTP](../reference/apis/wizard-http.md) (`/wizard/tailscale`, `/wizard/cloudflare-tunnel`)
- [Concepts → Architecture](../concepts/architecture.md)
- [Security → Default-on controls](../security/default-on-controls.md)
