# 02 — Profiles

A profile is a TOML manifest that tells `deputyctl` which agent is running on the device, where its files live, what systemd unit drives it, what channels it understands, and how the wizard should ask the user about it.

**Critically, the manifest only describes what is *already in the image*.** Manifests do not install anything. Adding a new profile means (1) the build pipeline learns to bake it in, (2) a manifest file lands in `profiles/`. There is no code change to `deputyctl`.

## Where manifests live

| Where | What |
|---|---|
| `profiles/<id>.toml` in this repo | Source of truth. Edited by humans and the release-tracker bot. |
| `/etc/deputyos/profiles/<id>.toml` on the device | Bake-time copy. Read by `deputyctl` at boot. |

When the bake pipeline produces an image, it copies the matching manifest into `/etc/deputyos/profiles/`. `deputyctl profile list` enumerates these.

## Schema

```toml
[profile]
id              = "openclaw"             # required, unique. Becomes /etc/deputyos/profiles/<id>.toml
display_name    = "OpenClaw"             # required, shown in wizard and `deputyctl profile list`
upstream_repo   = "openclaw/openclaw"    # required. Used by the release-tracker
release_channel = "stable"               # "stable" | "beta"
min_ram_mb      = 4096                   # wizard warns below this
pinned_version  = "2026.4.25"            # baked-in version; bumped by tracker bot

[paths]
install_root    = "/opt/deputyos/profiles/openclaw"  # required
data_dir        = "/home/agent/.openclaw"           # required, lives on data partition
binary          = "/opt/deputyos/profiles/openclaw/node_modules/.bin/openclaw"

[runtime]
language        = "node"                 # "node" | "python" | "binary"
node_version    = "24"                   # if language=node
python_version  = "3.11"                 # if language=python
package_manager = "npm"                  # "npm" | "pnpm" | "uv" | "pip" | "none"
extra_apt       = []                     # documentation-only; baked at build, never installed at boot

[service]
unit            = "openclaw-gateway.service"   # systemd unit name
entrypoint      = "openclaw onboard --daemon"  # used to render the unit template
ports           = [8080, 8443]                 # ufw allows these when channels enabled
restart_policy  = "always"               # default

[health]
http_check      = "http://127.0.0.1:8080/healthz"
journal_unit    = "openclaw-gateway.service"
startup_grace_s = 30                     # how long after start before doctor expects healthy

[apparmor]
profile         = "/etc/apparmor.d/deputyos.openclaw"   # path to the AppArmor profile baked in

[kernel]
required_sysctls = { }                   # optional. Doctor verifies these are set

[wizard]
prompts = [                              # order matters; wizard asks in this sequence
  "model_provider",
  "channels",
  "gateway_allowlist",
  "backup_destination",
]

[channels]
supported = ["telegram", "slack", "discord", ...]   # subset that this profile can serve

[memory]
session_db      = "~/.openclaw/sessions.sqlite"     # backed up by `deputyctl backup`
backup_strategy = "rclone-sync"

[upgrade]
preserve_dirs   = ["~/.openclaw"]        # not touched on A/B swap
post_upgrade_hooks = ["deputyctl doctor"] # runs after slot switch
```

The two concrete examples in this repo are [`profiles/openclaw.toml`](../profiles/openclaw.toml) and [`profiles/hermes.toml`](../profiles/hermes.toml). Treat them as the definition of "good."

## What `deputyctl` does with a manifest

| Command | Manifest fields it reads |
|---|---|
| `deputyctl up` | `[paths].binary`, `[service]` |
| `deputyctl down` | `[service].unit` |
| `deputyctl profile switch <id>` | `[paths]`, `[service]`, `[apparmor]` |
| `deputyctl doctor` | `[runtime]`, `[health]`, `[kernel]`, `[apparmor]` |
| `deputyctl logs` | `[health].journal_unit` |
| `deputyctl model set` | always reads `/etc/deputyos/secrets.env`; profile-agnostic |
| `deputyctl backup` | `[memory]`, `[paths].data_dir` |
| `deputyctl update --apply` | `[upgrade]` (preserve_dirs, post_upgrade_hooks) |

## `deputyctl` command surface (frozen for M0)

```
deputyctl init                     # first-boot wizard (TTY + web on :8088)
deputyctl profile list             # show installed profiles + versions
deputyctl profile status           # detail on the active profile
deputyctl profile switch <id>      # stop old, start new

deputyctl up | down | restart      # active profile
deputyctl status                   # short health summary
deputyctl logs [--follow]          # journal of the active profile
deputyctl shell                    # drop into the profile's CLI

deputyctl doctor                   # full health check; nonzero on any failure
deputyctl limits                   # what this device CAN and CANNOT do, with reasons
deputyctl version                  # SBOM + build manifest

deputyctl model list               # show configured + available providers
deputyctl model set                # interactive provider/model picker
deputyctl model test               # 1-token round-trip on the current config

deputyctl update --check           # read signed manifest from CDN
deputyctl update --apply           # A/B swap, watchdog rollback
deputyctl rollback                 # force boot to the other slot

deputyctl backup now               # one-off rclone push
deputyctl backup schedule          # configure cron-like schedule
deputyctl restore --list           # snapshots in user bucket
deputyctl restore --snapshot <id>  # atomic restore

deputyctl factory-reset            # wipe data partition, keep system

deputyctl tunnel                   # open a Cloudflare Quick Tunnel + print URL
```

Every command is non-destructive by default; `factory-reset`, `rollback`, and `update --apply` prompt unless `--yes` is passed.

`deputyctl limits` is a load-bearing UX surface — see [14-limitations.md §"The `deputyctl limits` command (spec)"](14-limitations.md#the-deputyctl-limits-command-spec) for the output format. Every "won't work" message printed by `deputyctl` (e.g. "channel disabled by memory budget") refers users to `deputyctl limits` for the full picture.

## Adding a third profile

To add a new agent (must be in the OpenClaw/Hermes class — multi-channel personal assistant with persistent memory and a skill system; see [CONTRIBUTING.md](../CONTRIBUTING.md#profile-class)):

1. **Bake recipe** — add Ansible tasks under `roles/deputyos/tasks/profile-<id>.yml` that lay the agent down at `/opt/deputyos/profiles/<id>/` *at build time*. Pre-resolve all native modules and Python wheels into the offline cache.
2. **systemd unit template** — add `templates/<id>.service.j2`.
3. **AppArmor profile** — add `apparmor/<id>` with the right `r/w/ix` rules for the agent's data dir, install root, and any sockets.
4. **Manifest** — add `profiles/<id>.toml` matching the schema above.
5. **CI matrix entry** — add a row to the build matrix in `.github/workflows/build.yml`.
6. **Documentation** — link to the upstream from `README.md` and add a section to [05-model-providers.md](05-model-providers.md) if the new profile supports providers we haven't seen yet.

That's it. Six files, no Rust changes. The third-profile milestone (M7) exists specifically to validate this is true.

## Profile-class rule (quick recap)

A profile **must** be a personal AI assistant in the shape of OpenClaw/Hermes:

- multi-channel gateway (Telegram, Slack, Discord, … — text-message-driven)
- persistent memory across conversations
- skill / tool system the agent can invoke

A profile **must not** be:

- an IDE coding agent (Aider, Continue.dev) — those belong as IDE plugins
- an agent framework (AutoGen, LangGraph) — those are libraries, not appliances
- an integration shim for a different ecosystem (Home Assistant) — those should consume deputyOS, not be a profile in it

Community PRs that don't fit get a polite redirection rather than a merge.
