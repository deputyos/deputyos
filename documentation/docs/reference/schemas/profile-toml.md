# Profile manifest (`profile.toml`)

A **profile manifest** is the source of truth for what an agent IS to
deputyOS: where its binary lives, which port it listens on, which AppArmor
profile confines it, which channels it can serve, what data it persists.
Manifests are TOML files; they are bake-time inputs (the build pipeline
copies the binary, renders the systemd unit, lays down the AppArmor
profile, all from the manifest) and runtime inputs (`deputyctl` reads the
active profile's manifest on every invocation).

This page walks every field of every section. The authoritative struct
is [`deputyctl/src/manifest.rs`](https://github.com/deputyos/deputyos/blob/main/deputyctl/src/manifest.rs);
the validator that enforces semantic invariants beyond raw TOML shape is
[`deputyctl/src/validate.rs`](https://github.com/deputyos/deputyos/blob/main/deputyctl/src/validate.rs).

deputyOS ships with **three** profiles today, all of which fit the
[OpenClaw / Hermes / Khoj profile class](../../concepts/profile-class.md):

| Profile id | Display name | Manifest path | Upstream |
|---|---|---|---|
| `openclaw` | OpenClaw | `profiles/openclaw.toml` | [openclaw/openclaw](https://github.com/openclaw/openclaw) |
| `hermes` | Hermes Agent | `profiles/hermes.toml` | [NousResearch/hermes-agent](https://github.com/NousResearch/hermes-agent) |
| `khoj` | Khoj | `profiles/khoj.toml` | [khoj-ai/khoj](https://github.com/khoj-ai/khoj) |

At runtime they live under `/etc/deputyos/profiles/<id>.toml`. Path
resolution is documented in [Filesystem layout](../system/filesystem-layout.md).

[TOC]

## File location and discovery

- **Source-tree path** (where the build pipeline reads from): `profiles/<id>.toml`
- **Baked image path**: `/etc/deputyos/profiles/<id>.toml`
- **Resolution order** (`deputyctl/src/paths.rs::profiles_dir`):
  1. `$DEPUTYOS_PROFILES_DIR` env var if set
  2. `/etc/deputyos/profiles/` if it exists
  3. `./profiles/` (workspace-root copy used during dev)
- **Active profile pointer**: `/etc/deputyos/active-profile` (one-line plain text holding the profile id; absence means "no active profile")

The validator (`deputyctl profile validate <path>...`) deserialises each
manifest via [`crate::manifest`](https://github.com/deputyos/deputyos/blob/main/deputyctl/src/manifest.rs)
and then runs the rules in [`crate::validate`](https://github.com/deputyos/deputyos/blob/main/deputyctl/src/validate.rs).
See [How-to / Add a profile](../../how-to/add-a-profile.md) for the
authoring workflow.

!!! info "Permissive shape"
    Beyond `[profile]`, `[paths]`, `[runtime]`, `[service]`, and
    `[health]`, every section is optional. Profiles legitimately omit
    sections that don't apply (OpenClaw has no `[memory]` block today;
    Hermes has no `[channels]` defaults override).

## `[profile]` — identity

| Field | Type | Required | Description |
|---|---|---|---|
| `id` | string | yes | Profile identifier. Validator: must match `^[a-z][a-z0-9-]*$` (lowercase, kebab-case, starts with a letter) and equal the file stem. |
| `display_name` | string | yes | Human-readable name shown in the wizard, PWA dashboard, and `deputyctl status`. |
| `upstream_repo` | string | yes | `<owner>/<repo>` slug for the upstream agent project. Free-form; release tracker matches against this. |
| `release_channel` | string | yes | Upstream tracker channel (`stable` or `beta`). Distinct from deputyOS image channel; this controls what the release-tracker bot watches for new tags. |
| `min_ram_mb` | integer | yes | Wizard warns when the device's RAM (from `limits.json`) is below this threshold. |
| `pinned_version` | string | yes | Upstream tag baked into the current image. Bumped by the release-tracker bot when a new upstream release is integrated. |

Example:

```toml
[profile]
id              = "openclaw"
display_name    = "OpenClaw"
upstream_repo   = "openclaw/openclaw"
release_channel = "stable"
min_ram_mb      = 4096
pinned_version  = "2026.4.25"
```

## `[paths]` — filesystem layout

| Field | Type | Required | Description |
|---|---|---|---|
| `install_root` | string | yes | Absolute path where the bake pipeline lays down the agent's install tree. **Validator**: must be absolute and start with `/opt/deputyos/profiles/<id>`. |
| `data_dir` | string | yes | Absolute or `~/`-prefixed path where the agent persists data. **Validator**: must live under `/home/agent/` or `~/` so the `agent` user owns it. |
| `binary` | string | yes | Absolute path of the entrypoint executable. **Validator**: must be absolute and live inside `install_root`. |

Why `binary` lives inside `install_root`: AppArmor profiles attach by
binary path. Forcing the binary under the install root means a single
profile path covers the whole agent tree.

Example:

```toml
[paths]
install_root = "/opt/deputyos/profiles/openclaw"
data_dir     = "/home/agent/.openclaw"
binary       = "/opt/deputyos/profiles/openclaw/node_modules/.bin/openclaw"
```

## `[runtime]` — language + bake-time deps

| Field | Type | Required | Description |
|---|---|---|---|
| `language` | string | yes | One of `node`, `python`, `binary`. Validator-enforced. |
| `node_version` | string | required when `language="node"` | Major version (e.g. `"24"`). |
| `python_version` | string | required when `language="python"` | `<major>.<minor>` (e.g. `"3.11"`). |
| `package_manager` | string | yes | `npm` / `pip` / `uv` / etc. Free-form. |
| `extra_apt` | array of strings | optional | Extra apt packages baked at image build. Always empty in the three real profiles — they prefer baking through the install role rather than the manifest. |

`runtime` is **declarative bake-time data**. Nothing in the manifest is
installed at boot; everything is already in the image. `deputyctl doctor`
uses `language`/`node_version`/`python_version` to verify the runtime
matches what was promised.

## `[service]` — systemd unit shape

| Field | Type | Required | Description |
|---|---|---|---|
| `unit` | string | yes | systemd unit name. **Validator**: must end with `.service`. |
| `entrypoint` | string | yes | Command rendered into the unit's `ExecStart=`. May include arguments. |
| `ports` | array of u16 | yes | TCP listen ports. **Validator**: must be non-empty; port `0` rejected. |
| `restart_policy` | string | optional, defaults to `"always"` | systemd `Restart=` value. |

The bake-time Ansible role renders the unit template
([systemd-units reference](../system/systemd-units.md)) using these
values.

## `[health]` — liveness probe

| Field | Type | Required | Description |
|---|---|---|---|
| `http_check` | string | yes | URL hit by `deputyctl status` and the wizard chat fallback. **Validator**: must be empty, `http://...`, or `https://...`. |
| `journal_unit` | string | yes | systemd unit name `journalctl -u` reads from. Usually identical to `service.unit`. |
| `startup_grace_s` | u32 | yes | Wall-clock seconds the wizard waits before declaring the agent unhealthy. Heavier profiles (Khoj's index warmup) use 45s; lighter ones (OpenClaw) use 30s. |

## `[apparmor]` — confinement profile (optional)

| Field | Type | Required | Description |
|---|---|---|---|
| `profile` | string | yes (when section is present) | Path to the AppArmor profile. **Validator**: must live under `/etc/apparmor.d/`. |

The three shipped profiles all declare their AppArmor profile. See
[AppArmor profiles reference](../system/apparmor-profiles.md) for the
rule walkthroughs.

## `[kernel]` — sysctl prerequisites (optional)

| Field | Type | Required | Description |
|---|---|---|---|
| `required_sysctls` | map\<string, string\> | optional | Sysctls the bake pipeline drops into `/etc/sysctl.d/`. |

Used by Hermes to enable `kernel.unprivileged_userns_clone=1` for its
skill-sandbox (which calls `unshare(CLONE_NEWUSER)`). OpenClaw and Khoj
don't need it.

## `[wizard]` — first-boot prompt order (optional)

| Field | Type | Required | Description |
|---|---|---|---|
| `prompts` | array of strings | optional | Ordered list of prompt ids the wizard asks. Profiles share a small vocabulary: `model_provider`, `channels`, `gateway_allowlist`, `backup_destination`. |

The wizard is allowed to honour or override this order; in practice it
follows it. See [deputywizard HTTP API](../apis/wizard-http.md).

## `[channels]` — declared channel surface (optional)

| Field | Type | Required | Description |
|---|---|---|---|
| `supported` | array of strings | optional | Channel ids this profile *can* serve. **Validator**: profile must declare at least one. |

The wizard intersects `supported` with the device's
`limits.capabilities.channels_disabled_by_ram` to decide which checkboxes
to grey out. ufw rules and AppArmor mediations tighten to the chosen
subset at apply time. See [How-to / Add a channel](../../how-to/add-a-channel.md).

## `[memory]` — persistent state hints (optional)

| Field | Type | Required | Description |
|---|---|---|---|
| `session_db` | string | optional | Path to the agent's session DB (typically a SQLite file). Cosmetic — `deputyctl backup` honours `[upgrade].preserve_dirs`, not this hint. |
| `backup_strategy` | string | optional | One of `rclone-sync` (the default), `none`. Documents intent. |

## `[upgrade]` — update behaviour (optional)

| Field | Type | Required | Description |
|---|---|---|---|
| `preserve_dirs` | array of strings | optional | Directories `deputyctl update` keeps across image swaps. Paths can use `~/` to refer to `/home/agent/`. |
| `post_upgrade_hooks` | array of strings | optional | Commands run after an update finishes staging. The shipped profiles all run `deputyctl doctor`. |

## `[mounts]` — drive-mount defaults for the wizard (optional, M3.5)

A profile can default-suggest mounts for the wizard Drives step. The user
still confirms each mount; nothing here is a secret (share credentials live
in `/etc/deputyos/secrets.env`, never in the manifest).

| Field | Type | Required | Description |
|---|---|---|---|
| `default_mode` | `"ro"` \| `"rw"` | optional (default `"ro"`) | Default mode offered for the suggested mounts. |
| `suggested_paths` | array of strings | optional | Guest paths to pre-fill in the wizard Drives step. Each must live under `/mnt/deputyos/`; the wizard validates. |

```toml
[mounts]
default_mode    = "ro"
suggested_paths = ["/mnt/deputyos/documents", "/mnt/deputyos/code"]
```

## `[airgap]` — air-gapped build default provider (optional, M4.5)

On an `AIRGAP=1` build the wizard hides cloud API-key providers and offers
only the baked local-LLM providers (kind `local-llamacpp`, from the airgap
catalog). This section names which provider the wizard pre-selects so the
baked LFM2 is chosen automatically — no API key, no network. Non-airgap
builds ignore it.

`default_provider` is a logical alias. `local-llamacpp-airgap` resolves to
the catalog's default model (the one baked for this tier); a specific
`airgap-<model-id>` selects that exact model instead.

| Field | Type | Required | Description |
|---|---|---|---|
| `default_provider` | string | yes | Provider id to pre-select on an airgap build. Convention: `local-llamacpp-airgap`. |

```toml
[airgap]
default_provider = "local-llamacpp-airgap"
```

## Validator semantic rules (full list)

The validator (`deputyctl profile validate <path>`) enforces, in order:

1. **`profile.id`** — kebab-lowercase regex; must equal the file stem.
2. **`paths.install_root`** — absolute and prefixed with `/opt/deputyos/profiles/<id>`.
3. **`paths.data_dir`** — absolute or `~/`-rooted; must live under `/home/agent/` or `~/`.
4. **`paths.binary`** — absolute; must live inside `install_root`.
5. **`service.unit`** — must end with `.service`.
6. **`service.ports`** — must be non-empty; no port `0`.
7. **`runtime.language`** — one of `node`, `python`, `binary`. `node` requires `node_version`; `python` requires `python_version`.
8. **`health.http_check`** — empty or http(s).
9. **`apparmor.profile`** (when section present) — under `/etc/apparmor.d/`.
10. **`channels.supported`** — must declare at least one channel.

Violations are emitted as `{field, reason}` and rendered as
`<path>: <field>: <reason>` lines, or `--json` for CI consumption. See
[`deputyctl profile validate`](../cli/deputyctl.md).

## Full example: `openclaw.toml`

```toml
# OpenClaw profile manifest.
# Read by `deputyctl` at runtime; consumed by the image build pipeline at bake time.
# This file describes what is *already in the image* — there is no install step at boot.

[profile]
id              = "openclaw"
display_name    = "OpenClaw"
upstream_repo   = "openclaw/openclaw"
release_channel = "stable"        # stable | beta — controls the release-tracker, not first boot
min_ram_mb      = 4096            # wizard warns if device RAM is below this
pinned_version  = "2026.4.25"     # what is baked into the current image — bumped by the tracker bot

[paths]
install_root    = "/opt/deputyos/profiles/openclaw"
data_dir        = "/home/agent/.openclaw"
binary          = "/opt/deputyos/profiles/openclaw/node_modules/.bin/openclaw"

[runtime]
# Declared so `deputyctl doctor` can verify versions, not so anything is installed.
language        = "node"
node_version    = "24"
package_manager = "npm"
extra_apt       = []              # baked into the image, not installed at boot

[service]
unit            = "openclaw-gateway.service"
entrypoint      = "openclaw onboard --daemon"
ports           = [8080, 8443]

[health]
http_check      = "http://127.0.0.1:8080/healthz"
journal_unit    = "openclaw-gateway.service"
startup_grace_s = 30

[apparmor]
profile         = "/etc/apparmor.d/deputyos.openclaw"

[wizard]
# Order in which the wizard collects answers for this profile.
prompts = [
  "model_provider",
  "channels",
  "gateway_allowlist",
  "backup_destination",
]

[channels]
# Channels OpenClaw can serve. The wizard asks the user which to enable; ufw rules and
# AppArmor mediations are tightened to only the chosen subset.
supported = [
  "telegram", "slack", "discord", "whatsapp", "signal", "imessage", "bluebubbles",
  "irc", "matrix", "feishu", "line", "mattermost", "nextcloud-talk", "nostr",
  "synology-chat", "tlon", "twitch", "zalo", "wechat", "qq", "google-chat",
  "microsoft-teams", "webchat",
]

[upgrade]
# When the release tracker sees a new upstream tag, the build pipeline produces a new image
# rev. The user applies it via `deputyctl update`; data_dir is preserved across updates.
preserve_dirs   = ["~/.openclaw"]
post_upgrade_hooks = ["deputyctl doctor"]
```

## Full example: `hermes.toml`

```toml
# Hermes Agent profile manifest.
# Read by `deputyctl` at runtime; consumed by the image build pipeline at bake time.
# This file describes what is *already in the image* — there is no install step at boot.

[profile]
id              = "hermes"
display_name    = "Hermes Agent"
upstream_repo   = "NousResearch/hermes-agent"
release_channel = "stable"
min_ram_mb      = 4096
pinned_version  = "0.11.0"

[paths]
install_root    = "/opt/deputyos/profiles/hermes"
data_dir        = "/home/agent/.hermes"
binary          = "/opt/deputyos/profiles/hermes/.venv/bin/hermes"

[runtime]
language        = "python"
python_version  = "3.11"
package_manager = "uv"
extra_apt       = []

[service]
unit            = "hermes-gateway.service"
entrypoint      = "hermes gateway start"
ports           = [8080]

[health]
http_check      = "http://127.0.0.1:8080/healthz"
journal_unit    = "hermes-gateway.service"
startup_grace_s = 30

[apparmor]
profile         = "/etc/apparmor.d/deputyos.hermes"

[kernel]
# Hermes' command-execution sandbox uses unprivileged user namespaces.
required_sysctls = { "kernel.unprivileged_userns_clone" = "1" }

[wizard]
prompts = [
  "model_provider",
  "channels",
  "gateway_allowlist",
  "backup_destination",
]

[channels]
supported = [
  "telegram", "discord", "slack", "whatsapp", "signal", "dingtalk", "twilio-sms",
  "mattermost", "matrix", "webhook", "email-imap-smtp", "home-assistant", "feishu",
  "wecom", "imessage",
]

[memory]
# Hermes uses an FTS5 SQLite session store. Lives in data_dir; backed up by `deputyctl backup`.
session_db      = "~/.hermes/sessions.sqlite"
backup_strategy = "rclone-sync"

[upgrade]
preserve_dirs   = ["~/.hermes"]
post_upgrade_hooks = ["deputyctl doctor"]
```

## Full example: `khoj.toml`

```toml
# Khoj profile manifest.
# Read by `deputyctl` at runtime; consumed by the image build pipeline at bake time.
# This file describes what is *already in the image* — there is no install step at boot.
#
# Khoj fits the OpenClaw/Hermes profile class:
#   * multi-channel gateway     — web chat, Telegram, WhatsApp (Twilio), Obsidian, Emacs, desktop
#   * persistent memory         — SQLite + embedded vector store; chat history per conversation
#   * skill / tool system       — agent personas with tools (online search, document Q&A, code, image)

[profile]
id              = "khoj"
display_name    = "Khoj"
upstream_repo   = "khoj-ai/khoj"
release_channel = "stable"
min_ram_mb      = 4096            # honest: Khoj loads embeddings + chat models; 4GB is the floor
pinned_version  = "1.32.0"        # bumped by the release-tracker bot

[paths]
install_root    = "/opt/deputyos/profiles/khoj"
data_dir        = "/home/agent/.khoj"
binary          = "/opt/deputyos/profiles/khoj/.venv/bin/khoj"

[runtime]
language        = "python"
python_version  = "3.11"
package_manager = "pip"
extra_apt       = []

[service]
unit            = "khoj-gateway.service"
entrypoint      = "khoj --no-gui --host 127.0.0.1 --port 42110"
ports           = [42110]

[health]
http_check      = "http://127.0.0.1:42110/api/health"
journal_unit    = "khoj-gateway.service"
startup_grace_s = 45              # embeddings + index warmup is heavier than Hermes/OpenClaw

[apparmor]
profile         = "/etc/apparmor.d/deputyos.khoj"

[wizard]
prompts = [
  "model_provider",
  "channels",
  "gateway_allowlist",
  "backup_destination",
]

[channels]
supported = [
  "web", "telegram", "whatsapp-twilio", "obsidian", "emacs", "desktop",
]

[memory]
session_db      = "~/.khoj/khoj.sqlite"
backup_strategy = "rclone-sync"

[upgrade]
preserve_dirs   = ["~/.khoj"]
post_upgrade_hooks = ["deputyctl doctor"]
```

## See also

- [How-to / Add a profile](../../how-to/add-a-profile.md) — the
  step-by-step recipe to land a new profile.
- [Concepts / Profile class](../../concepts/profile-class.md) — what
  fits, what doesn't.
- [Concepts / Plugin model](../../concepts/plugin-model.md) — the
  six-file contract every profile satisfies.
- [Reference / CLI / deputyctl](../cli/deputyctl.md) — the `profile`,
  `profile validate`, `status`, `doctor`, `up`, `down` subcommands.
- [Reference / System / systemd units](../system/systemd-units.md) —
  what `[service]` renders into.
- [Reference / System / AppArmor profiles](../system/apparmor-profiles.md) —
  what `[apparmor]` binds.
- [Reference / Schemas / limits.json](limits-json.md) — what
  `limits.capabilities.channels_disabled_by_ram` intersects with.
