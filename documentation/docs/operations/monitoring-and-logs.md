# Monitoring and logs

## What this guide does

Show you what's happening on a running deputyOS device. deputyOS exposes
**three visibility layers**:

1. **`deputyctl status`** — a one-shot snapshot of the active profile.
2. **`deputyctl doctor`** — a deeper health probe, machine-readable JSON
   on demand.
3. **The deputypwa dashboard** — always-on PWA at `/app/dashboard` with a
   logs viewer at `/app/logs`.

Underneath it all is the systemd journal — every unit logs there, and
`journalctl -u <unit>` is the source of truth.

## Layer 1: `deputyctl status`

```sh
sudo deputyctl status
```

Prints, in human-readable form:

- Active profile id, version (read from
  `/opt/deputyos/profiles/<id>/.bake-meta`), stub-or-real flag.
- Active profile's gateway unit state (`active (running)` /
  `failed` / `inactive`).
- Cost summary — today's spend / monthly spend / cap.
- Quiet-hours state.
- Tunnel state (Tailscale up / Cloudflare URL if any).
- Cost-tripped marker, if present.

JSON form:

```sh
sudo deputyctl status --json
```

…is identical to the human form, machine-parseable. Useful for
external monitoring (Prometheus exporters, Healthchecks.io HEAD probes,
etc.).

## Layer 2: `deputyctl doctor`

A deeper probe. Walks every check in `deputyctl/src/doctor.rs`:

- Kernel version + AppArmor enforcement state.
- ufw status.
- fail2ban running.
- Active profile unit health.
- Wizard service health.
- Provider key configured.
- Disk space + zram health.
- Tunnel health.
- (Per-target) thermal-throttle marker presence.

Emits non-zero on any failed check. Machine-readable JSON:

```sh
sudo deputyctl doctor --json | jq
```

`deputyctl doctor` is the canonical "is something wrong?" entry point.
Wire it into a Healthchecks.io ping or a Prometheus textfile exporter.

## Layer 3: The PWA dashboard

```text
http://deputyos.local/app/dashboard
http://deputyos.local/app/logs?lines=200
```

The dashboard is the user-facing always-on view. It surfaces:

- Active profile, version.
- Gateway state.
- Cost gauge (today / monthly).
- Tunnel + reachability indicators.
- Recent CostAlert events.
- Hardware tier + per-target limitations (the same content as
  `deputyctl limits`).

The logs viewer at `/app/logs?lines=N&unit=<unit>` shells out to
`journalctl -u <unit> -n N --no-pager` and renders the result in a
fixed-width pre-block. Authentication is the wizard session cookie.

## The unit map

Every deputyOS service that logs:

| Unit | What it does | Logs say |
| --- | --- | --- |
| `<profile>-gateway.service` | active profile gateway (openclaw / hermes / khoj) | per-message traffic, model errors |
| `deputywizard.service` | first-boot wizard + auth | apply-step diagnostics |
| `deputypwa.service` | always-on dashboard | (M5 wiring) |
| `deputyos-voice-relay.service` | whisper.cpp + Piper bridge | wake-word events, STT/TTS errors |
| `deputyos-relay.service` | Unix-socket message relay (when wired) | hook fire results |
| `deputyos-qr-on-tty.service` | first-boot QR code on console | one-shot at boot |
| `avahi-daemon.service` | mDNS publishing | resolve traffic |
| `cloudflared.service` | named tunnel (when configured) | tunnel state changes |
| `tailscaled.service` | Tailscale | mesh state changes |
| `deputyos-clamscan.timer` | on-demand virus scan (rpi4 only) | per-scan results |
| `deputyos-backup.timer` | scheduled rclone snapshots | per-run results |

Tail any of them:

```sh
sudo journalctl -u <unit> -f
sudo journalctl -u <unit> --since '1 hour ago'
sudo journalctl -u <unit> -n 200 --no-pager
```

## "What to look at when..."

| Symptom | First place to look | Then look at |
| --- | --- | --- |
| Something is **slow** | `deputyctl cost --json` (token rates per call) | `journalctl -u <profile>-gateway` for retries / fallbacks |
| Something is **silent** | wizard's channels.d/ — token / config maybe missing | `journalctl -u <profile>-gateway` for "no channel handler" |
| Gateway **refuses** to start | the cost-tripped marker — `ls /var/lib/deputyos/cost-tripped` | `journalctl -u <profile>-gateway` first 20 lines |
| Can't **connect** from LAN | `sudo ufw status verbose` — which ports allowed | wizard's gateway_allowlist (in `/etc/deputyos/<profile>/`) |
| Can't **connect** from outside | tunnel state — `tailscale status` or `deputyctl status` | `journalctl -u cloudflared` |
| AppArmor **denial** | `dmesg \| grep apparmor\|grep DENIED` | the relevant `/etc/apparmor.d/deputyos.<id>` profile rules |
| Cost **way too high** | `deputyctl cost ledger --last 50 --json` | the pre-message hook for retry storm? |
| Update **failed** | `journalctl -u deputyctl --since '15 min ago'` | the staging dir at `/var/lib/deputyos/staging/` for partial files |

## Aggregate views

`deputyctl status` + `deputyctl doctor` cover ~95% of routine
investigation. For longitudinal views:

- **Cost over time** — `deputyctl cost ledger --last 1000 --json | jq
  ...`. The ledger is JSONL, one entry per LLM request.
- **Gateway uptime** — `systemctl show <profile>-gateway.service
  --property=ActiveEnterTimestamp,InvocationID,NRestarts`.
- **Reboot count** — `last -F | head -10` plus `dmesg -T | head`.

For external aggregation, Healthchecks.io is the smallest-footprint
option: a systemd timer that runs `deputyctl doctor` and pings the
Healthchecks URL on success.

## Where the journal lives

- Persistent journal: `/var/log/journal/<machine-id>/`.
- Configured in `/etc/systemd/journald.conf.d/`. deputyOS leaves
  systemd defaults; review with `journalctl --disk-usage`.
- Rotation: defaults — 4 weeks or 10% of disk, whichever first.
- Clear stale: `sudo journalctl --vacuum-time=7d`.

## Troubleshooting

!!! warning "Logs vanish after reboot"
    The journal is persistent only if `/var/log/journal/<machine-id>/`
    exists with mode `2755`. If it's missing (some old images shipped
    without it), `mkdir -p /var/log/journal/$(cat /etc/machine-id)`
    and `systemctl restart systemd-journald`.

!!! warning "PWA logs viewer shows '(no entries)' but `journalctl` works"
    The PWA shells out to `journalctl` as the `agent` user, who must
    be in the `systemd-journal` group. Check `id agent`. If not,
    `usermod -aG systemd-journal agent` and restart `deputypwa`.

!!! tip "`journalctl -u <unit> -o cat` is your friend"
    Strips the timestamp + pid prefix; just the log line. Useful for
    grepping or piping into `jq` if the unit logs JSON.

!!! tip "Agent timeouts often look like 'silent'"
    A model API timeout returns to the agent, the agent retries, and
    the user sees a long pause. Look for high `duration_ms` values in
    `deputyctl cost ledger`. The CostAlert hook can fire on retry
    storms — wire one up.

## Related

- [Reference → CLI → deputyctl](../reference/cli/deputyctl.md) (`status`, `doctor`, `logs` subcommands)
- [Reference → APIs → PWA HTTP](../reference/apis/pwa-http.md) (the `/app/logs` route)
- [Operations → Cost guardrails](cost-guardrails.md)
- [Operations → Update and rollback](update-and-rollback.md)
- [Reference → System → systemd units](../reference/system/systemd-units.md)
