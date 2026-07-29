# Filesystem layout

A canonical map of every file and directory that deputyOS owns on a
baked image. Grouped by tree (config, install, runtime state, runtime
ephemeral, agent home), with owner, mode, format, producer, consumer,
and env-override columns.

The path-resolution module is
[`deputyctl/src/paths.rs`](https://github.com/deputyos/deputyos/blob/main/deputyctl/src/paths.rs);
the rendering Ansible role lives at `roles/deputyos/`.

[TOC]

## Conventions

- **Owner** is the Linux owner:group. `agent:agent` is the unprivileged
  agent service user; `root:root` is the OS-level superuser; `root:agent`
  appears for files root-owned but agent-readable (e.g. `secrets.env`).
- **Mode** is octal Unix permissions.
- **Producer** is what writes the file in production: the Ansible role
  at bake time, the wizard at first boot, `deputyctl <subcmd>` at
  runtime, etc.
- **Consumer** is what reads the file at runtime.
- **Env override** points to the env var (when present) that overrides
  the resolution path during dev/test.

## `/etc/deputyos/` — runtime configuration

Read-only at runtime for unprivileged services; writable only by root
and the wizard's privileged surface.

| Path | Owner | Mode | Format | Producer | Consumer | Env override |
|---|---|---|---|---|---|---|
| `/etc/deputyos/` | root:root | 0755 | dir | bake (`deputyos` Ansible role) | all deputyctl/wizard/PWA | – |
| `/etc/deputyos/active-profile` | root:root | 0644 | one-line text (profile id) | wizard step 2 / `deputyctl profile activate` | `deputyctl status`, `up`, `down`, `doctor` | `DEPUTYOS_ACTIVE_PROFILE_FILE` |
| `/etc/deputyos/active-provider` | root:root | 0644 | one-line text (provider id) | `deputyctl model set` | `deputyctl model test`, PWA keys page | `DEPUTYOS_ACTIVE_PROVIDER_FILE` |
| `/etc/deputyos/secrets.env` | root:agent | 0640 | KEY=VALUE shell-style | `deputyctl model set`, wizard apply | gateway services (`EnvironmentFile=-`), `deputyctl model list` | `DEPUTYOS_SECRETS_FILE` |
| `/etc/deputyos/profiles/` | root:root | 0755 | dir | bake | `deputyctl profile`, wizard | `DEPUTYOS_PROFILES_DIR` |
| `/etc/deputyos/profiles/openclaw.toml` | root:root | 0644 | TOML ([profile-toml](../schemas/profile-toml.md)) | bake (copies `profiles/openclaw.toml`) | `deputyctl profile`, wizard, PWA | – |
| `/etc/deputyos/profiles/hermes.toml` | root:root | 0644 | TOML | bake | as above | – |
| `/etc/deputyos/profiles/khoj.toml` | root:root | 0644 | TOML | bake | as above | – |
| `/etc/deputyos/providers.json` | root:root | 0644 | JSON ([providers-json](../schemas/providers-json.md)) | bake (copies `deputyctl/etc/providers.json`) | `deputyctl model`, wizard step 3, PWA keys page | `DEPUTYOS_PROVIDERS_FILE` |
| `/etc/deputyos/cost-defaults.json` | root:root | 0644 | JSON | bake | `deputyctl cost`, ledger | – |
| `/etc/deputyos/limits.json` | root:root | 0644 | JSON ([limits-json](../schemas/limits-json.md)) | bake (copies target's `limits.<target>.json`) | `deputyctl limits`, doctor, wizard, PWA | `DEPUTYOS_LIMITS_FILE` |
| `/etc/deputyos/voice.toml` | root:root | 0644 | TOML | bake (renders `voice.toml.j2`) | `deputyos-voice-relay.service` | – |
| `/etc/deputyos/rclone.conf` | root:agent | 0640 | rclone INI | wizard apply, `deputyctl backup` | `deputyctl backup`, `restore` | `DEPUTYOS_RCLONE_CONFIG` |
| `/etc/deputyos/backup.toml` | root:root | 0644 | TOML | `deputyctl backup schedule` | `deputyctl backup now`, restore | `DEPUTYOS_BACKUP_CONFIG` |
| `/etc/deputyos/hooks.d/` | root:root | 0755 | dir | operator | dispatcher (`deputyctl/src/hooks.rs`) | `DEPUTYOS_HOOKS_DIR` |
| `/etc/deputyos/hooks.d/pre-message/` | root:root | 0755 | dir of executables | operator | dispatcher / relay | – |
| `/etc/deputyos/hooks.d/post-message/` | root:root | 0755 | dir of executables | operator | dispatcher / relay | – |
| `/etc/deputyos/hooks.d/cost-alert/` | root:root | 0755 | dir of executables | operator | `cost::evaluate` | – |
| `/etc/deputyos/hooks.d/update-applied/` | root:root | 0755 | dir of executables | operator | `update::run_apply` | – |
| `/etc/deputyos/openclaw/channels.d/` | root:root | 0755 | dir of TOML/JSON | wizard apply, `deputyctl channel` | `openclaw-gateway` (read by AppArmor profile) | – |
| `/etc/deputyos/hermes/` | root:root | 0755 | dir | wizard apply | `hermes-gateway` (allowed by AppArmor profile) | – |
| `/etc/deputyos/khoj/` | root:root | 0755 | dir | wizard apply | `khoj-gateway` (allowed by AppArmor profile) | – |
| `/etc/deputyos/deputyos-pubkey.pub` | root:root | 0644 | minisign pubkey | bake | `deputyctl update --check` | `DEPUTYOS_PUBKEY_FILE` |
| `/etc/deputyos/mounts-policy.json` | root:root | 0644 | JSON ([mounts-policy](../schemas/mounts-policy.md)) | `deputyctl mounts add`/`network-add`/`remove`, wizard /mounts | `deputyos-mounts.service` materialiser, `deputyctl mounts list/health`, PWA `/app/mounts` | `DEPUTYOS_MOUNTS_POLICY` |
| `/etc/deputyos/network-policy.json` | root:root | 0644 | JSON ([network-policy](../schemas/network-policy.md)) | `deputyctl network`, airgap bake | `deputyctl network status`, PWA "Your device"/"Network" cards, nftables renderer, `deputyos-network-apply.service` | `DEPUTYOS_NETWORK_POLICY` |
| `/etc/deputyos/network-defaults.json` | root:root | 0644 | JSON ([network-defaults](../schemas/network-defaults.md)) | bake (per-profile) | read-only seed: `deputyctl network mode whitelist` copies `allow_hosts` into `network-policy.json` when empty | `DEPUTYOS_NETWORK_DEFAULTS` (tests) |

`secrets.env` mode is 0640, owned `root:agent` so the gateway services
(running as `agent`) can `EnvironmentFile=-` it without granting world
read. The wizard writes it; `deputyctl model set` updates it. See
[Security / Secrets storage](../../security/secrets-storage.md).

## `/etc/apparmor.d/` — confinement profiles

Set to enforce by the bake role; loaded by `apparmor.service`.

| Path | Owner | Mode | Producer | Confines |
|---|---|---|---|---|
| `/etc/apparmor.d/deputyos.openclaw` | root:root | 0644 | bake (copies `roles/deputyos/files/apparmor/deputyos.openclaw`) | `/opt/deputyos/profiles/openclaw/node_modules/.bin/openclaw` |
| `/etc/apparmor.d/deputyos.hermes` | root:root | 0644 | bake | `/opt/deputyos/profiles/hermes/.venv/bin/hermes` |
| `/etc/apparmor.d/deputyos.khoj` | root:root | 0644 | bake | `/opt/deputyos/profiles/khoj/.venv/bin/khoj` |
| `/etc/apparmor.d/deputyos.voice-relay` | root:root | 0644 | bake | `/opt/deputyos/voice/voice-relay.sh` |

See [AppArmor profiles reference](apparmor-profiles.md) for the rule
walkthroughs.

## `/etc/systemd/system/` — deputyOS units

Rendered from the templates in `roles/deputyos/templates/`.

| Path | Producer | Documented at |
|---|---|---|
| `/etc/systemd/system/openclaw-gateway.service` | bake | [systemd-units](systemd-units.md#openclaw-gatewayservice) |
| `/etc/systemd/system/hermes-gateway.service` | bake | [systemd-units](systemd-units.md#hermes-gatewayservice) |
| `/etc/systemd/system/khoj-gateway.service` | bake | [systemd-units](systemd-units.md#khoj-gatewayservice) |
| `/etc/systemd/system/deputywizard.service` | bake | [systemd-units](systemd-units.md#deputywizardservice) |
| `/etc/systemd/system/deputyos-qr-on-tty.service` | bake | [systemd-units](systemd-units.md#deputyos-qr-on-ttyservice) |
| `/etc/systemd/system/deputyos-voice-relay.service` | bake (gated on `deputyos_voice_enabled`) | [systemd-units](systemd-units.md#deputyos-voice-relayservice) |
| `/etc/systemd/system/deputyos-backup.timer` | `deputyctl backup schedule` | [systemd-units](systemd-units.md#deputyos-backuptimer) |
| `/etc/systemd/system/deputyos-backup.service` | `deputyctl backup schedule` | [systemd-units](systemd-units.md#deputyos-backuptimer) |
| `/etc/systemd/system/deputyos-mounts.service` | bake (`deputyos-mounts.service.j2`) | [systemd-units](systemd-units.md#deputyos-mountsservice) |

## `/opt/deputyos/` — install root

Read-only data; never modified at runtime. Each profile gets a subtree;
voice gets its own.

| Path | Owner | Mode | Format | Producer | Consumer |
|---|---|---|---|---|---|
| `/opt/deputyos/` | root:root | 0755 | dir | bake | – |
| `/opt/deputyos/profiles/openclaw/` | root:root | 0755 | install tree (Node) | bake (`tasks/profile-openclaw.yml`) | `openclaw-gateway.service` |
| `/opt/deputyos/profiles/openclaw/node_modules/.bin/openclaw` | root:root | 0755 | shebang script | bake | `ExecStart=` of openclaw-gateway |
| `/opt/deputyos/profiles/hermes/` | root:root | 0755 | install tree (Python venv) | bake | `hermes-gateway.service` |
| `/opt/deputyos/profiles/hermes/.venv/bin/hermes` | root:root | 0755 | venv entrypoint | bake | `ExecStart=` of hermes-gateway |
| `/opt/deputyos/profiles/khoj/` | root:root | 0755 | install tree (Python venv) | bake | `khoj-gateway.service` |
| `/opt/deputyos/profiles/khoj/.venv/bin/khoj` | root:root | 0755 | venv entrypoint | bake | `ExecStart=` of khoj-gateway |
| `/opt/deputyos/voice/` | root:root | 0755 | dir | bake (`tasks/voice-baseline.yml`) | `deputyos-voice-relay.service` |
| `/opt/deputyos/voice/voice-relay.sh` | root:root | 0755 | shell script | bake | systemd ExecStart |
| `/opt/deputyos/voice/whisper-cli` | root:root | 0755 | binary (whisper.cpp) | bake | voice-relay.sh |
| `/opt/deputyos/voice/piper` | root:root | 0755 | binary (Piper) | bake | voice-relay.sh |
| `/opt/deputyos/voice/whisper-tiny.en.bin` | root:root | 0644 | model | bake | whisper-cli |
| `/opt/deputyos/voice/piper/en_US-amy-medium.onnx` | root:root | 0644 | model | bake | piper |

## `/mnt/deputyos/` — agent-visible mount root (M3.5)

The single tree every agent-visible mount lives under, so AppArmor's
per-profile rules can confine it (`/mnt/deputyos/** rwk` in all three
gateway profiles). `deputyos-mounts.service` materialises `host_fs` and
`network` entries from
[`mounts-policy.json`](../schemas/mounts-policy.md) at boot
(`mount --bind` for host-FS, `mount -t cifs|nfs` for network); the udev
rule handles `removable` media on insert. Nothing is baked into this
tree — it is populated entirely at runtime from the policy.

| Path | Owner | Mode | Format | Producer | Consumer |
|---|---|---|---|---|---|
| `/mnt/deputyos/` | root:root | 0755 | dir | bake (`mounts-baseline.yml`) | `deputyos-mounts.service` materialiser |
| `/mnt/deputyos/<id>` | root:root | 0755 | mount point | `deputyos-mount-materialise.sh` / `deputyos-mount-removable.sh` | gateway services (AppArmor-confined) |

Every `guest_path` in the policy must match `/mnt/deputyos/.*`;
`deputyctl::mounts::validate_guest_path` rejects anything outside it.

## `/usr/local/bin/` — deputyOS binaries

| Path | Owner | Mode | Producer | Notes |
|---|---|---|---|---|
| `/usr/local/bin/deputyctl` | root:root | 0755 | bake | static-linked Rust binary |
| `/usr/local/bin/deputywizard` | root:root | 0755 | bake | first-boot wizard server |
| `/usr/local/bin/deputypwa` | root:root | 0755 | bake | PWA dashboard server |

## `/var/lib/deputyos/` — persistent state

Survives reboots. NOT cleared by `deputyctl factory-reset` unless
explicitly listed.

| Path | Owner | Mode | Format | Producer | Consumer | Env override |
|---|---|---|---|---|---|---|
| `/var/lib/deputyos/` | agent:agent | 0755 | dir | bake (`StateDirectory=deputyos`) | wizard, PWA, deputyctl | – |
| `/var/lib/deputyos/wizard-state.json` | root:agent | 0640 | JSON (step + answers + completed_at) | wizard each step | wizard, `deputyctl status` | `DEPUTYOS_WIZARD_STATE_FILE` |
| `/var/lib/deputyos/slots.json` | root:root | 0644 | JSON (A/B slot pointers) | M6 update flow | `deputyctl rollback` | – |
| `/var/lib/deputyos/cost-tripped` | agent:agent | 0644 | empty marker file | `cost::evaluate` | `deputyctl cost`, gateway | – |
| `/var/lib/deputyos/staging/` | root:root | 0755 | dir | `deputyctl update --apply` | next-boot bootloader / A/B swap | – |
| `/var/lib/deputyos/staging/<release>/<filename>` | root:root | 0644 | image artefact | `deputyctl update --apply` | A/B swap (M6) | – |
| `/var/lib/deputyos/push-subscriptions.jsonl` | agent:agent | 0640 | JSON-Lines | PWA `/app/push/subscribe` | PWA push sender | (per-instance override) |
| `/var/lib/deputyos/vapid.pem` | agent:agent | 0600 | PEM (VAPID keypair) | PWA on first run (openssl shells out) | PWA push sender | – |
| `/var/lib/deputyos/cost-ledger.jsonl` | agent:agent | 0640 | JSON-Lines | gateway via `deputyctl cost record` | `deputyctl cost report`, PWA dashboard | – |

## `/run/deputyos/` — runtime (tmpfs, cleared on reboot)

`RuntimeDirectory=deputyos` is set on the wizard and gateway units;
systemd creates and tears it down per-boot.

| Path | Owner | Mode | Format | Producer | Consumer | Env override |
|---|---|---|---|---|---|---|
| `/run/deputyos/` | root:root | 0755 | dir | systemd `RuntimeDirectory=deputyos` | – | – |
| `/run/deputyos/relay.sock` | agent:agent | 0600 | Unix socket (`SOCK_STREAM`) | `deputyctl --internal-run-relay` | gateway agents | `DEPUTYOS_RELAY_SOCKET` |
| `/run/deputyos/wizard.token` | root:root | 0600 | hex token | wizard at start | first request to / wizard | – |
| `/run/deputyos/wizard.url` | root:root | 0644 | one-line URL | `deputywizard print-qr` / wizard | `deputyos-qr-on-tty.service` | – |
| `/run/deputyos/cloudflared.pid` | root:root | 0644 | text (pid) | `deputyctl tunnel up` | `deputyctl tunnel down` | – |
| `/run/deputyos/deputyos-desktop.pid` | (host) | 0644 | text (pid) | `deputyos-desktop start` (host-side, M2.5) | `deputyos-desktop stop`/`status` | – |
| `/run/deputyos/cost-tripped` | agent:agent | 0644 | (alternate marker location during tests) | `cost::evaluate` | gateway | – |

## `/home/agent/` — agent user home

Owned by the unprivileged `agent` user. Each profile has its own
`~/.<profile>` data directory; the shipped profiles all live under
this tree.

| Path | Owner | Mode | Format | Producer | Consumer |
|---|---|---|---|---|---|
| `/home/agent/` | agent:agent | 0750 | dir | bake (`useradd agent`) | – |
| `/home/agent/.openclaw/` | agent:agent | 0700 | OpenClaw data dir | OpenClaw at first run | OpenClaw |
| `/home/agent/.hermes/` | agent:agent | 0700 | Hermes data dir | Hermes at first run | Hermes |
| `/home/agent/.hermes/sessions.sqlite` | agent:agent | 0600 | SQLite (FTS5 session store) | Hermes | Hermes |
| `/home/agent/.hermes/skills/` | agent:agent | 0700 | dir of authored skills | Hermes self-improvement loop | Hermes (and AppArmor `ix`) |
| `/home/agent/.khoj/` | agent:agent | 0700 | Khoj data dir | Khoj at first run | Khoj |
| `/home/agent/.khoj/khoj.sqlite` | agent:agent | 0600 | SQLite (Django ORM + chat sessions) | Khoj | Khoj |
| `/home/agent/.khoj/content/` | agent:agent | 0700 | indexed PDFs/markdown/org-mode | user import via Khoj | Khoj |
| `/home/agent/.khoj/skills/` | agent:agent | 0700 | JSON persona/tool configs | Khoj at runtime | Khoj |
| `/home/agent/chat-history.jsonl` | agent:agent | 0640 | JSON-Lines (wizard `/chat`) | wizard `/chat/message` | wizard `/chat` page | `DEPUTYWIZARD_CHAT_HISTORY` |

`[upgrade].preserve_dirs` in each profile's manifest names the data
dir; `deputyctl update` keeps these across image swaps. See
[Profile manifest reference](../schemas/profile-toml.md#upgrade-update-behaviour-optional).

## `/etc/sysctl.d/` — kernel tunables

| Path | Producer | Notes |
|---|---|---|
| `/etc/sysctl.d/90-deputyos.conf` | bake (`90-deputyos.conf.j2`) | Hardened sysctl baseline for deputyOS hosts. Includes `kernel.unprivileged_userns_clone=1` only when a profile that needs it (Hermes) is the active bake target. |

## `/etc/ufw/` — firewall

| Path | Producer | Notes |
|---|---|---|
| `/etc/ufw/before.rules` (replaced) | bake (`ufw.rules.j2`) | Default-deny inbound; allow loopback, established, channel ports per active profile. |

## See also

- [Reference / Schemas / Profile manifest](../schemas/profile-toml.md) —
  files under `/etc/deputyos/profiles/`.
- [Reference / Schemas / Limits](../schemas/limits-json.md) —
  `/etc/deputyos/limits.json` schema.
- [Reference / Schemas / Providers](../schemas/providers-json.md) —
  `/etc/deputyos/providers.json`, `cost-defaults.json`, `voice.toml`.
- [Reference / System / systemd units](systemd-units.md) — units that
  `RuntimeDirectory=deputyos` and `StateDirectory=deputyos` provision
  the `/run/` and `/var/lib/` trees.
- [Reference / System / AppArmor profiles](apparmor-profiles.md) — what
  paths each profile reads/writes/denies.
- [Reference / APIs / Message relay](../apis/message-relay.md) —
  `/run/deputyos/relay.sock`.

## /run/deputyos — runtime tmpfs cleared on reboot

(Anchor target for cross-references; see the
"`/run/deputyos/` — runtime (tmpfs, cleared on reboot)" table above.)
- [Security / Secrets storage](../../security/secrets-storage.md) —
  `/etc/deputyos/secrets.env` lifecycle.
