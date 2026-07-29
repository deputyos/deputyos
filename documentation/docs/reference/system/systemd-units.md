# systemd units

Every long-lived process on a baked deputyOS image runs under systemd.
Units are rendered from Jinja templates in `roles/deputyos/templates/`
at bake time and dropped into `/etc/systemd/system/`.

This page walks each unit: the real `ExecStart`, the user it runs as,
the hardening directives, the `Condition*` gates, and the
dependencies. Every claim here is grounded in the corresponding
`*.service.j2` template.

[TOC]

## Unit summary

| Unit | Runs as | First-boot only? | Port(s) | AppArmor profile |
|---|---|---|---|---|
| [`openclaw-gateway.service`](#openclaw-gatewayservice) | `agent:agent` | no | 8080, 8443 | `deputyos.openclaw` |
| [`hermes-gateway.service`](#hermes-gatewayservice) | `agent:agent` | no | 8080 | `deputyos.hermes` |
| [`khoj-gateway.service`](#khoj-gatewayservice) | `agent:agent` | no | 42110 | `deputyos.khoj` |
| [`deputywizard.service`](#deputywizardservice) | `root:root` | yes (`ConditionPathExists=!/var/lib/deputyos/wizard-state.json`) | configurable (`deputyos_wizard_port`) | none (root + minimal hardening) |
| [`deputyos-qr-on-tty.service`](#deputyos-qr-on-ttyservice) | `root:root` | yes (same gate) | n/a (oneshot) | none |
| [`deputyos-voice-relay.service`](#deputyos-voice-relayservice) | `agent:agent` (+ supplementary `audio`) | no (gated on `voice.toml` `enabled=true`) | n/a | `deputyos.voice-relay` |
| [`avahi-deputyos.service`](#avahi-deputyosservice) | n/a (avahi-daemon's job) | no | mDNS (5353/udp) | confined by avahi |
| [`deputyos-backup.timer`](#deputyos-backuptimer) | `root:root` (oneshot triggers) | no | n/a | none |
| [`deputyos-mounts.service`](#deputyos-mountsservice) | `root:root` (oneshot, `RemainAfterExit`) | no (runs every boot) | n/a | none |
| [`deputyos-network-apply.service`](#deputyos-network-applyservice) | `root:root` (oneshot, `RemainAfterExit`) | no (runs every boot) | n/a | none |

## Hardening baseline

The three gateway services share a hardened baseline. The relaxations
are in **bold** below; everything else is identical across the three
units:

```
NoNewPrivileges=true
PrivateTmp=true
PrivateDevices=true
ProtectSystem=strict
ProtectHome=read-only
ReadWritePaths=<data_dir> /etc/deputyos
ProtectKernelTunables=true
ProtectKernelModules=true
ProtectKernelLogs=true
ProtectControlGroups=true
ProtectClock=true
ProtectHostname=true
RestrictNamespaces=true        # Hermes overrides: ~mnt cgroup pid
RestrictRealtime=true
RestrictSUIDSGID=true
LockPersonality=true
MemoryDenyWriteExecute=true    # voice-relay overrides: false (Piper/onnxruntime JIT)
SystemCallArchitectures=native
SystemCallFilter=@system-service
SystemCallFilter=~@privileged @resources @mount
                               # Hermes adds: SystemCallFilter=unshear clone clone3 setns
```

Hermes' relaxations exist to support its skill-sandbox child processes
(`unshare(CLONE_NEWUSER)`); see the dedicated section below for the
rationale.

## `openclaw-gateway.service`

Source: `roles/deputyos/templates/openclaw-gateway.service.j2`.

```ini
[Unit]
Description=OpenClaw gateway (deputyOS)
Documentation=https://github.com/openclaw/openclaw
After=network-online.target
Wants=network-online.target

[Service]
Type=simple
User=agent
Group=agent
WorkingDirectory={{ openclaw_data_dir }}      # /home/agent/.openclaw
EnvironmentFile=-/etc/deputyos/secrets.env
ExecStart={{ openclaw_install_root }}/node_modules/.bin/{{ openclaw_entrypoint }}
Restart=always
RestartSec=5

# Hardening — see baseline above. Identical to the boilerplate.
NoNewPrivileges=true
PrivateTmp=true
PrivateDevices=true
ProtectSystem=strict
ProtectHome=read-only
ReadWritePaths={{ openclaw_data_dir }} /etc/deputyos
…
SystemCallFilter=@system-service
SystemCallFilter=~@privileged @resources @mount

[Install]
WantedBy=multi-user.target
```

**Confined by**: `/etc/apparmor.d/deputyos.openclaw`, attached by binary
path. See [AppArmor / openclaw](apparmor-profiles.md#deputyosopenclaw).

## `hermes-gateway.service`

Source: `roles/deputyos/templates/hermes-gateway.service.j2`.

Hermes-specific deviations from the gateway baseline:

- `Environment=PYTHONUNBUFFERED=1`, `PYTHONDONTWRITEBYTECODE=1`.
- `RestrictNamespaces=~mnt cgroup pid` — allows the skill-executor to
  call `unshare(CLONE_NEWUSER)`, `CLONE_NEWIPC`, `CLONE_NEWUTS`,
  `CLONE_NEWNET` for sandboxing children. Mount, cgroup, and pid
  namespaces still blocked to keep escape surface small.
- `SystemCallFilter=unshare clone clone3 setns` — additive grant for
  the namespace-creating syscalls.
- `MemoryDenyWriteExecute=true` stays on (Hermes is interpreted Python,
  no JIT).

```ini
[Unit]
Description=Hermes Agent gateway (deputyOS)
After=network-online.target
Wants=network-online.target

[Service]
Type=simple
User=agent
Group=agent
WorkingDirectory={{ hermes_data_dir }}
EnvironmentFile=-/etc/deputyos/secrets.env
Environment=PYTHONUNBUFFERED=1
Environment=PYTHONDONTWRITEBYTECODE=1
ExecStart={{ hermes_entrypoint }}
Restart=always
RestartSec=5
…
RestrictNamespaces=~mnt cgroup pid
…
SystemCallFilter=@system-service
SystemCallFilter=~@privileged @resources @mount
SystemCallFilter=unshare clone clone3 setns
```

**Confined by**: `/etc/apparmor.d/deputyos.hermes`. The AppArmor profile
also grants `capability sys_admin` and `sys_ptrace` to support the
sandbox.

## `khoj-gateway.service`

Source: `roles/deputyos/templates/khoj-gateway.service.j2`.

Khoj has no skill-child sandbox, so its hardening matches the strict
gateway baseline (no `RestrictNamespaces` relaxation). Additional
environment:

- `KHOJ_DB_TYPE=sqlite` — pins the embedded SQLite path (Khoj's
  Postgres+pgvector default is replaced for the appliance "no separate
  database service" rule).
- `KHOJ_DATA_DIR={{ khoj_data_dir }}` (i.e. `/home/agent/.khoj`).
- `KHOJ_HOST=127.0.0.1`.

```ini
[Unit]
Description=Khoj gateway (deputyOS)
After=network-online.target
Wants=network-online.target

[Service]
Type=simple
User=agent
Group=agent
WorkingDirectory={{ khoj_data_dir }}
EnvironmentFile=-/etc/deputyos/secrets.env
Environment=PYTHONUNBUFFERED=1
Environment=PYTHONDONTWRITEBYTECODE=1
Environment=KHOJ_DB_TYPE=sqlite
Environment=KHOJ_DATA_DIR={{ khoj_data_dir }}
Environment=KHOJ_HOST=127.0.0.1
ExecStart={{ khoj_entrypoint }}
Restart=always
RestartSec=5
…
RestrictNamespaces=true   # strict (no sandbox children)
```

**Confined by**: `/etc/apparmor.d/deputyos.khoj`.

## `deputywizard.service`

Source: `roles/deputyos/templates/deputywizard.service.j2`.

The wizard runs **as root**. It has to:

- Write `/etc/hostname`, `/etc/timezone`, `/etc/deputyos/...`.
- Call `hostnamectl`, `timedatectl`, `systemctl`.
- Generate `/run/deputyos/wizard.token` (mode 0600).

Running root + axum is a hardening regression accepted **only** for
first boot. The gate below stops the unit from auto-starting once the
wizard finishes (`/var/lib/deputyos/wizard-state.json` exists with
`step="done"`).

```ini
[Unit]
Description=deputyOS first-boot setup wizard
After=network-online.target
Wants=network-online.target
ConditionPathExists=!/var/lib/deputyos/wizard-state.json

[Service]
Type=simple
User=root
Group=root
RuntimeDirectory=deputyos
RuntimeDirectoryMode=0755
StateDirectory=deputyos
StateDirectoryMode=0755
ExecStart=/usr/local/bin/deputywizard serve \
  --port {{ deputyos_wizard_port }} \
  --bind {{ deputyos_wizard_bind }} \
  --production
Restart=on-failure
RestartSec=5

# Hardening — best effort given root. ProtectSystem=strict is incompatible
# with writing /etc, so we use ProtectSystem=full and explicitly grant
# ReadWritePaths.
NoNewPrivileges=false
ProtectSystem=full
ProtectHome=read-only
ReadWritePaths=/etc/deputyos /etc/hostname /etc/timezone /var/lib/deputyos /run/deputyos
ProtectKernelTunables=true
ProtectKernelModules=true
ProtectKernelLogs=true
ProtectControlGroups=true
ProtectClock=false
RestrictRealtime=true
RestrictSUIDSGID=true
LockPersonality=true
```

`RuntimeDirectory=deputyos` and `StateDirectory=deputyos` create
`/run/deputyos/` and `/var/lib/deputyos/` with the right modes.

A future M3 task (tracked as `TODO(M3-rest)` in the template) will move
the wizard to the `agent` user with a polkit policy + a small setuid
`deputyctl-bootstrap` helper for the privileged surface. For now: bounded
to one boot.

## `deputyos-qr-on-tty.service`

Source: `roles/deputyos/templates/deputyos-qr-on-tty.service.j2`.

A `Type=oneshot` unit that prints the wizard URL and a scannable QR
code on **tty1** (HDMI) and the **system console** (serial / hypervisor
console). Each redirect is its own `ExecStart=` so a missing tty (e.g.
headless server) doesn't fail the unit.

```ini
[Unit]
Description=deputyOS first-boot QR code on TTY
After=deputywizard.service
Requires=deputywizard.service
ConditionPathExists=!/var/lib/deputyos/wizard-state.json

[Service]
Type=oneshot
User=root
Group=root
ExecStart=/bin/sh -c '/usr/local/bin/deputywizard print-qr > /dev/tty1 2>&1 || true'
ExecStart=/bin/sh -c '/usr/local/bin/deputywizard print-qr > /dev/console 2>&1 || true'
RemainAfterExit=true

NoNewPrivileges=true
ProtectSystem=strict
ProtectHome=true
PrivateTmp=true
ProtectKernelTunables=true
ProtectKernelModules=true
ProtectKernelLogs=true
ProtectControlGroups=true
```

## `deputyos-voice-relay.service`

Source: `roles/deputyos/templates/deputyos-voice-relay.service.j2`.

Bridges audio capture (whisper.cpp) → wake-word detection → the
message-relay socket as a `pre-message` hook with `source="voice"`.
The agent's reply is piped back through Piper to `/dev/snd/`.

Refuses to start unless **both**:

1. `/etc/deputyos/voice.toml` exists with `enabled=true` (the systemd
   `ConditionPathExists=` plus an `[ -f ]` check inside
   `voice-relay.sh`).
2. `/opt/deputyos/voice/voice-relay.sh` is in place.

```ini
[Unit]
Description=deputyOS voice relay (whisper.cpp + Piper bridge)
After=network-online.target sound.target
Wants=network-online.target
ConditionPathExists=/etc/deputyos/voice.toml
ConditionPathExists=/opt/deputyos/voice/voice-relay.sh

[Service]
Type=simple
User=agent
Group=agent
SupplementaryGroups=audio
WorkingDirectory=/var/lib/deputyos
EnvironmentFile=-/etc/deputyos/secrets.env
ExecStart=/opt/deputyos/voice/voice-relay.sh
Restart=on-failure
RestartSec=5
Nice=-5

# Audio devices — load-bearing relaxation
DeviceAllow=/dev/snd/* rw
DevicePolicy=closed
PrivateDevices=false

# FS hardening
NoNewPrivileges=true
ProtectSystem=strict
ProtectHome=read-only
ReadWritePaths=/run/deputyos /var/lib/deputyos
ReadOnlyPaths=/etc/deputyos /opt/deputyos/voice

# Kernel + capability lockdown
ProtectKernelTunables=true
ProtectKernelModules=true
ProtectKernelLogs=true
ProtectControlGroups=true
ProtectClock=true
ProtectHostname=true
RestrictNamespaces=true
RestrictSUIDSGID=true
LockPersonality=true
SystemCallArchitectures=native
SystemCallFilter=@system-service
SystemCallFilter=~@privileged @resources @mount

# Audio realtime priority + Piper JIT
RestrictRealtime=false
MemoryDenyWriteExecute=false

CapabilityBoundingSet=
AmbientCapabilities=
```

### Why the relaxations

- **`DeviceAllow=/dev/snd/* rw`**: required to open the soundcard.
  Default systemd hardening hides `/dev/snd`; we re-grant explicit `rw`.
- **`RestrictRealtime=false`**: ALSA + Piper can want SCHED_FIFO/RR for
  glitch-free playback under load. CAP_SYS_NICE is *not* granted —
  `audio` group + `Nice=-5` covers the latency budget on stock
  Debian/Ubuntu kernels.
- **`MemoryDenyWriteExecute=false`**: Piper's onnxruntime JITs at
  startup; can't deny W^X.

**Confined by**: `/etc/apparmor.d/deputyos.voice-relay`.

## `avahi-deputyos.service`

Source: `roles/deputyos/templates/avahi-deputyos.service.j2`.

Not a systemd unit — an avahi-daemon **service file** that publishes
mDNS records under `<hostname>.local`:

| Type | Port | Path | Purpose |
|---|---|---|---|
| `_workstation._tcp` | 22 | – | SSH browsers see deputyOS hosts as Linux desktops. |
| `_https._tcp` | `deputyos_wizard_port` | `/wizard` | First-boot wizard. |
| `_http._tcp` | 8080 | `/chat` | Agent chat endpoint (profile-specific gateway). |

```xml
<service-group>
  <name replace-wildcards="yes">deputyOS on %h</name>

  <service>
    <type>_workstation._tcp</type>
    <port>22</port>
  </service>

  <service>
    <type>_https._tcp</type>
    <port>{{ deputyos_wizard_port }}</port>
    <txt-record>path=/wizard</txt-record>
    <txt-record>deputyos=wizard</txt-record>
  </service>

  <service>
    <type>_http._tcp</type>
    <port>8080</port>
    <txt-record>path=/chat</txt-record>
    <txt-record>deputyos=chat</txt-record>
  </service>
</service-group>
```

Lives at `/etc/avahi/services/deputyos.service`. avahi-daemon picks it
up automatically.

## `deputyos-backup.timer`

**Not** rendered by the bake role. Created at runtime by `deputyctl
backup schedule`. The body is generated in
[`deputyctl/src/backup.rs`](https://github.com/deputyos/deputyos/blob/main/deputyctl/src/backup.rs):

```ini
# /etc/systemd/system/deputyos-backup.timer
[Unit]
Description=deputyOS backup timer

[Timer]
OnCalendar=*-*-* 00/6:00:00     # default: every 6h. Override via --every / --at.
Persistent=true
Unit=deputyos-backup.service

[Install]
WantedBy=timers.target
```

```ini
# /etc/systemd/system/deputyos-backup.service
[Unit]
Description=deputyOS backup (rclone push)

[Service]
Type=oneshot
ExecStart=/usr/local/bin/deputyctl backup now
```

Schedule format:

- `--every 6h` → `OnCalendar=*-*-* 00/6:00:00`
- `--every 1d` → `OnCalendar=daily`
- `--at 03:00` → `OnCalendar=*-*-* 03:00:00`
- default (no flag) → `*-*-* 00/6:00:00`

`deputyctl backup schedule` runs `systemctl daemon-reload` and
`systemctl enable --now deputyos-backup.timer` after writing the unit
files (skipped in dev mode when `DEPUTYOS_DEV_OUT` is set).

## `deputyos-mounts.service`

Rendered at bake time from
[`roles/deputyos/templates/deputyos-mounts.service.j2`](https://github.com/deputyos/deputyos/blob/main/roles/deputyos/templates/deputyos-mounts.service.j2)
and **enabled at boot** (M3.5: the role runs `daemon-reload` + `enable
--now` gated on the unit changing). It is a `oneshot` unit with
`RemainAfterExit=true` so its "active" state reflects "the policy was
materialised this boot", and it orders `Before=deputywizard.service` so
mounts exist before the first-boot wizard runs.

```ini
# /etc/systemd/system/deputyos-mounts.service
[Unit]
Description=deputyOS drive + share mount materialiser
DefaultDependencies=no
After=local-fs.target
Before=deputywizard.service

[Service]
Type=oneshot
RemainAfterExit=yes
ExecStart=/usr/local/sbin/deputyos-mount-materialise.sh
StandardOutput=journal
StandardError=journal

[Install]
WantedBy=multi-user.target
```

`deputyos-mount-materialise.sh` reads
[`/etc/deputyos/mounts-policy.json`](../schemas/mounts-policy.md) and
bind-mounts each `host_fs` entry (`mount --bind`) and mounts each
`network` entry (`mount -t cifs` / `mount -t nfs`). Credentials for CIFS
come from `/etc/deputyos/secrets.env` via the `credentials_env` key.
Removable USB/SD media is handled out-of-band by the udev rule
`99-deputyos-removable.rules` → `deputyos-mount-removable.sh`, not by this
unit.

`deputyctl mounts apply` re-runs the materialiser (`systemctl restart
deputyos-mounts.service`) after a policy edit — skipped in dev mode when
`DEPUTYOS_DEV_OUT` is set.

## `deputyos-network-apply.service`

[`roles/deputyos/templates/deputyos-network-apply.service.j2`](https://github.com/deputyos/deputyos/blob/main/roles/deputyos/templates/deputyos-network-apply.service.j2)

```ini
# /etc/systemd/system/deputyos-network-apply.service
[Unit]
Description=deputyOS network egress policy re-applier
After=network-online.target nftables.service
Before=deputywizard.service

[Service]
Type=oneshot
RemainAfterExit=yes
ExecStart=/usr/local/lib/deputyos/deputyos-network-apply.sh

[Install]
WantedBy=multi-user.target
```

A boot oneshot that re-renders `/etc/nftables.conf` from
`/etc/deputyos/network-policy.json` so a runtime-injected nftables rule (a
compromised process smuggling in an allow) does not survive a reboot. The
wrapper `deputyos-network-apply.sh` **skips in `open` mode** — `deputyctl
network apply` regenerates the file beginning with `flush ruleset`, which is
load-bearing for airgap/whitelist (it re-declares the whole posture) but
would, in `open` mode, also wipe ufw's inbound default-deny on a connected
image. Open has no egress posture to self-heal, so the running ruleset is
left untouched and ufw owns inbound. In `whitelist` mode the boot re-apply
also re-resolves each allow-host so DNS rotation is followed (see the
[egress threat model](../../concepts/threat-model-egress.md)). Always-on,
not gated on `deputyos_airgap`; the airgap static ruleset in
`airgap-baseline.yml` still wins for `AIRGAP=1` images (the policy
regenerates an equivalent deny).

## See also

- [Reference / System / AppArmor profiles](apparmor-profiles.md) — what
  binds to each unit's binary.
- [Reference / System / Filesystem layout](filesystem-layout.md) —
  every path each unit reads/writes/runs from.
- [Reference / Schemas / Profile manifest](../schemas/profile-toml.md) —
  the `[service]` section that gets rendered into these units.
- [Reference / Schemas / Providers](../schemas/providers-json.md) —
  `voice.toml` consumed by `deputyos-voice-relay.service`.
- [How-to / Backup and restore](../../how-to/backup-and-restore.md) —
  configures the backup timer.
- [How-to / Enable voice](../../how-to/enable-voice.md) — flips the
  voice-relay's master switch.
- [Operations / Monitoring and logs](../../operations/monitoring-and-logs.md) —
  `journalctl -u <unit>` per-unit recipes.
