# Add a profile

## What this guide does

This guide walks through adding a **third (or fourth, or fifth) profile** to
deputyOS. A "profile" is the OpenClaw / Hermes / Khoj-shape unit: a
multi-channel personal-AI gateway with persistent memory and a tool/skill
system, packaged so that the deputyOS appliance can bake an image that runs
it as a confined, supervised, long-lived service.

The goal of the profile system is that adding a new profile is **six files
of YAML, TOML, systemd, AppArmor, and a stub script — and zero lines of
Rust**. The Rust crates (`deputyctl`, `deputywizard`, `deputypwa`,
`deputyos-track`, `deputyos-desktop`) treat profiles as data: a manifest at
`profiles/<id>.toml` is loaded at runtime, and everything else (units,
AppArmor, install paths) is bake-time scaffolding the role drops in. This
"zero Rust changes" invariant was the M7 acceptance test, and Khoj is the
landed proof.

The worked example below is **Khoj**, the third real profile.

## Prerequisites

Before starting, confirm that the upstream agent you are adding fits the
profile class:

- It speaks **multiple channels** — at least web, plus one of
  Telegram / WhatsApp / Slack / Obsidian / Emacs / desktop.
- It has **persistent memory** that survives restart — chat history,
  embeddings, vector store, document index, etc.
- It has a **skill/tool system** — agent personas, callable tools (search,
  document Q&A, code execution).
- It is **packageable as a single long-lived process** — not an IDE
  plugin, not a CLI you re-launch per query.
- Its license permits redistribution as part of an appliance image
  (Apache-2.0, MIT, BSD, GPL-with-system-libs all qualify).

If any of those is "no", the upstream agent does not belong as an deputyOS
profile. See [Concepts → Profile class](../concepts/profile-class.md) and
the [feedback note on profile class][feedback] for the canonical rule.

[feedback]: https://github.com/deputyos/deputyos/blob/main/docs/02-profiles.md

You also need a working contributor checkout — `make doctor` green, the
five workspace crates building, the existing two profiles (OpenClaw,
Hermes, Khoj) baking. See [Contributing → Overview](../contributing/overview.md).

## The recipe

Six files plus one one-line append. Numbered to match the order Khoj
landed in.

### 1. Profile manifest — `profiles/<id>.toml`

The runtime contract. Every field is documented in
[Reference → Schemas → profile.toml](../reference/schemas/profile-toml.md).

The manifest declares what the image **already contains** at boot — the
manifest is a description, never an install instruction. Required
sections:

```toml
[profile]
id              = "<id>"             # short slug; must match filename stem
display_name    = "<Display Name>"
upstream_repo   = "<owner>/<repo>"   # github slug; release-tracker uses it
release_channel = "stable"
min_ram_mb      = 4096               # honest floor, not aspirational
pinned_version  = "<x.y.z>"          # release-tracker bumps this

[paths]
install_root    = "/opt/deputyos/profiles/<id>"
data_dir        = "/home/agent/.<id>"
binary          = "/opt/deputyos/profiles/<id>/<...>/<id>"

[runtime]
language        = "python"          # or "node"
python_version  = "3.11"            # or node_version
package_manager = "pip"             # or "npm"
extra_apt       = []

[service]
unit            = "<id>-gateway.service"
entrypoint      = "<id> --no-gui --host 127.0.0.1 --port <port>"
ports           = [<port>]

[health]
http_check      = "http://127.0.0.1:<port>/api/health"
journal_unit    = "<id>-gateway.service"
startup_grace_s = 45

[apparmor]
profile         = "/etc/apparmor.d/deputyos.<id>"

[wizard]
prompts         = ["model_provider", "channels", "gateway_allowlist", "backup_destination"]

[channels]
supported       = ["web", "telegram", "..."]

[memory]
session_db      = "~/.<id>/<id>.sqlite"
backup_strategy = "rclone-sync"

[upgrade]
preserve_dirs      = ["~/.<id>"]
post_upgrade_hooks = ["deputyctl doctor"]
```

The validator runs a strict check: see
`deputyctl::validate::run` in `deputyctl/src/validate.rs`.

### 2. Bake recipe — `roles/deputyos/tasks/profile-<id>.yml`

The Ansible task list that lays down the install root, the venv, the
binary (or the stub fallback), the systemd unit, the AppArmor profile,
and the bake metadata.

The shape is identical for every Python profile (Khoj, Hermes share the
pattern). For a Node profile, replace the venv install with `npm ci --omit
dev` (OpenClaw's recipe is the canonical reference).

Required pieces, in order:

1. `set_fact` — declare the install root, data dir, unit name, pinned
   version, port, force-stub flag. All overridable via Packer
   `extra_vars` so the release-tracker bot can bump versions.
2. Create `install_root`, `data_dir`, and any subtree the upstream agent
   expects (`memory/`, `skills/`, `content/`, …) with `agent:agent`
   ownership and `0700` mode on data dirs.
3. Apt-install the language runtime + build deps + `netcat-openbsd` (the
   stub's healthz needs `nc.openbsd`).
4. Create the venv (or `npm ci`), best-effort upgrade pip/wheel.
5. Try the **real** install. Use `failed_when: false` and register the
   result so we can fall back to the stub if PyPI / npm has a hiccup.
6. Stat the resulting binary. `set_fact` to decide stub-vs-real.
7. Drop the stub binary if `use_stub` is true.
8. Render the systemd unit template; run `systemd-analyze verify` as a
   best-effort check.
9. Enable (do not start) the unit.
10. Drop the AppArmor profile; run `apparmor_parser -Q -K` as syntax
    check.
11. Write `.bake-meta` — a few `KEY=VALUE` lines that
    `deputyctl version` reads.

See the Khoj recipe at
[`roles/deputyos/tasks/profile-khoj.yml`][khoj-recipe] for the canonical
Python pattern.

[khoj-recipe]: https://github.com/deputyos/deputyos/blob/main/roles/deputyos/tasks/profile-khoj.yml

### 3. Systemd unit template — `roles/deputyos/templates/<id>-gateway.service.j2`

Hardening directives mirror the deputyOS systemd baseline. For a
**Python** profile, the Khoj unit is the reference. For **Node**, the
OpenClaw unit. The non-negotiables:

- `User=agent`, `Group=agent`.
- `EnvironmentFile=-/etc/deputyos/secrets.env` (the leading `-` makes it
  optional; secrets land at first-boot via the wizard).
- `NoNewPrivileges=true`.
- `ProtectSystem=strict` + an explicit `ReadWritePaths=` for data dir.
- `ProtectHome=read-only`, `PrivateTmp=true`, `PrivateDevices=true`.
- `RestrictNamespaces=true` unless the agent uses unshare for skill
  sandboxing (Hermes does; Khoj and OpenClaw do not).
- `RestrictRealtime=true`, `RestrictSUIDSGID=true`, `LockPersonality=true`.
- `MemoryDenyWriteExecute=true` for interpreted Python / Node (no JIT).
  Disable for V8 (OpenClaw needs it off).
- `SystemCallArchitectures=native`, `SystemCallFilter=@system-service`,
  with explicit `~@privileged @resources @mount`.
- `Restart=always`, `RestartSec=5`.

### 4. AppArmor profile — `roles/deputyos/files/apparmor/deputyos.<id>`

A real-rules confinement file, not a stub. Bind to the binary path the
unit ExecStarts. The Khoj profile at
[`roles/deputyos/files/apparmor/deputyos.khoj`][khoj-aa] is the reference.

[khoj-aa]: https://github.com/deputyos/deputyos/blob/main/roles/deputyos/files/apparmor/deputyos.khoj

Required allows:

- `#include <abstractions/base>` plus the language ones (`python`,
  `nameservice`, `openssl`).
- The interpreter (`/usr/bin/python3*` or `/usr/bin/node`).
- The install tree, `r` plus `mr` for `.so` / `.node` natives.
- The data dir, `owner` `rw` plus `rwk` (lock) for SQLite WAL.
- `/etc/deputyos/secrets.env`, `r`.
- `/etc/deputyos/profiles/<id>.toml`, `r`.
- `/etc/deputyos/<id>/`, `r` (per-profile config tree).
- `/etc/deputyos/limits.json`, `r`.
- `network inet stream / dgram / inet6 / unix / netlink raw`.

Required denies:

- `deny capability sys_admin, sys_module, sys_ptrace, sys_rawio,
  dac_override, dac_read_search, mac_admin, mac_override`.
- `deny /tmp/** wx`, `deny /var/tmp/** wx`.
- `deny /root/** rwlk`, `deny /home/[^a]*/** rwlk` (only `agent`'s home
  is allowed).
- `deny /etc/shadow rwlk`, `deny /etc/sudoers* rwlk`.

For Hermes, `sys_admin` is **allowed** (with a careful comment) so the
skill-child sandbox can `unshare`. For Khoj and OpenClaw, deny it.

### 5. Stub fallback — `roles/deputyos/files/<id>-stub.sh`

A bash + `nc.openbsd` HTTP healthz server that satisfies the smoke
harness when the real `pip install` / `npm ci` fails at bake time
(offline build, PyPI hiccup, yanked version).

The contract (Khoj's stub at
[`roles/deputyos/files/khoj-stub.sh`][khoj-stub] is the reference):

- Accept the real CLI's `--no-gui --host <addr> --port <port>` flag set,
  plus `--version` / `--help`.
- Listen on the port until SIGTERM/SIGINT.
- Serve `200 OK` with a JSON body `{"healthz":"ok","stub":true,...}` on
  every connection.
- Tolerate spurious failures (e.g. EADDRINUSE) so it doesn't crash-loop
  faster than `RestartSec`.

[khoj-stub]: https://github.com/deputyos/deputyos/blob/main/roles/deputyos/files/khoj-stub.sh

### 6. One-line import — `roles/deputyos/tasks/main.yml`

Append to the profile-dispatch block:

```yaml
- name: Apply <Id> profile bake
  ansible.builtin.import_tasks: profile-<id>.yml
  when: deputyos_profile == "<id>"
```

This is the **only** repo-wide edit. It does not require any code
changes elsewhere.

## Verification

Run these in order on a contributor laptop:

1. **Manifest validates.** `cargo run -p deputyctl -- profile validate
   profiles/<id>.toml`. Exits 0; emits no warnings.
2. **Manifest round-trips.** `cargo test -p deputyctl manifest_roundtrip`.
   The struct in `deputyctl/src/manifest.rs` deserializes every field
   without `#[serde(default)]` swallowing typos.
3. **Ansible lints.** `ansible-lint roles/`. Clean.
4. **AppArmor parses.** `apparmor_parser -Q -K
   roles/deputyos/files/apparmor/deputyos.<id>`. Best run inside a
   container or VM since the host's AppArmor namespace may already have
   a different profile loaded.
5. **systemd unit renders.** Bake a test image and run `systemd-analyze
   verify /etc/systemd/system/<id>-gateway.service` inside the booted
   guest.
6. **Bake succeeds.** `make build TARGET=qemu-aarch64 PROFILE=<id>`.
7. **Smoke passes.** `make smoke TARGET=qemu-aarch64 PROFILE=<id>
   SMOKE_LEVEL=m1`. The harness asserts the unit is `active (running)`,
   `deputyctl status` returns 0, and the healthz endpoint serves 200.
8. **CI is green.** `make ci SCAFFOLD_PHASE=1`.

## Worked example: Khoj

Concrete file paths and exact commands. Walk these on a fresh checkout
to reproduce the M7 acceptance.

### Files Khoj added

| Step | File |
| --- | --- |
| 1 | [`profiles/khoj.toml`](https://github.com/deputyos/deputyos/blob/main/profiles/khoj.toml) |
| 2 | [`roles/deputyos/tasks/profile-khoj.yml`](https://github.com/deputyos/deputyos/blob/main/roles/deputyos/tasks/profile-khoj.yml) |
| 3 | [`roles/deputyos/templates/khoj-gateway.service.j2`](https://github.com/deputyos/deputyos/blob/main/roles/deputyos/templates/khoj-gateway.service.j2) |
| 4 | [`roles/deputyos/files/apparmor/deputyos.khoj`](https://github.com/deputyos/deputyos/blob/main/roles/deputyos/files/apparmor/deputyos.khoj) |
| 5 | [`roles/deputyos/files/khoj-stub.sh`](https://github.com/deputyos/deputyos/blob/main/roles/deputyos/files/khoj-stub.sh) |
| 6 | one block in [`roles/deputyos/tasks/main.yml`](https://github.com/deputyos/deputyos/blob/main/roles/deputyos/tasks/main.yml) |

### Manifest highlights

```toml
[profile]
id              = "khoj"
display_name    = "Khoj"
upstream_repo   = "khoj-ai/khoj"
min_ram_mb      = 4096            # honest: embeddings + chat models; 4GB floor
pinned_version  = "1.32.0"

[service]
unit            = "khoj-gateway.service"
entrypoint      = "khoj --no-gui --host 127.0.0.1 --port 42110"
ports           = [42110]

[channels]
supported = ["web", "telegram", "whatsapp-twilio", "obsidian", "emacs", "desktop"]

[memory]
session_db      = "~/.khoj/khoj.sqlite"
backup_strategy = "rclone-sync"
```

Khoj's docker-compose default uses Postgres + pgvector. We pick SQLite
(`KHOJ_DB_TYPE=sqlite`) for M7 acceptance — it ships in-tree, keeps the
appliance self-contained, and matches Khoj's supported single-binary
mode. The choice is locked into both the manifest (`session_db`) and
the systemd unit (`Environment=KHOJ_DB_TYPE=sqlite`).

### Bake recipe walk-through

The four interesting sections in `profile-khoj.yml`:

```yaml
- name: Install Python runtime + Khoj build deps
  ansible.builtin.apt:
    name:
      - "python{{ khoj_python_version }}"
      - "python{{ khoj_python_version }}-venv"
      - libsqlite3-dev    # Khoj's chat session store
      - libffi-dev        # cryptography wheel build
      - netcat-openbsd    # required by the stub fallback
```

```yaml
- name: Install khoj from PyPI
  ansible.builtin.command:
    cmd: >-
      {{ khoj_install_root }}/.venv/bin/pip install
      --no-input --disable-pip-version-check
      khoj=={{ khoj_pinned_version }}
    creates: "{{ khoj_install_root }}/.venv/bin/khoj"
  failed_when: false        # never fail; stub fallback handles it
  retries: 3
  delay: 10
```

```yaml
- name: Decide stub vs real
  ansible.builtin.set_fact:
    khoj_use_stub: >-
      {{ (khoj_force_stub | bool)
         or (not _khoj_real_bin.stat.exists)
         or (khoj_install.rc | default(1) != 0) }}
```

```yaml
- name: Write Khoj bake metadata
  ansible.builtin.copy:
    dest: "{{ khoj_install_root }}/.bake-meta"
    content: |
      version={{ khoj_pinned_version }}
      python_version={{ khoj_python_version }}
      baked_at={{ khoj_baked_at }}
      stub={{ khoj_use_stub | bool | lower }}
```

`deputyctl version` reads `.bake-meta` to surface the upstream version
plus whether this image is running the stub or the real binary.

### Systemd unit highlights

```ini
[Service]
User=agent
Group=agent
WorkingDirectory={{ khoj_data_dir }}
EnvironmentFile=-/etc/deputyos/secrets.env
Environment=KHOJ_DB_TYPE=sqlite
Environment=KHOJ_DATA_DIR={{ khoj_data_dir }}
ExecStart={{ khoj_entrypoint }}
Restart=always
RestartSec=5

NoNewPrivileges=true
ProtectSystem=strict
ProtectHome=read-only
ReadWritePaths={{ khoj_data_dir }} /etc/deputyos
RestrictNamespaces=true       # Khoj has no skill-child sandbox
MemoryDenyWriteExecute=true   # interpreted Python (no JIT)
SystemCallArchitectures=native
SystemCallFilter=@system-service
SystemCallFilter=~@privileged @resources @mount
```

### AppArmor highlights

```aa
profile deputyos.khoj /opt/deputyos/profiles/khoj/.venv/bin/khoj flags=(enforce) {
  #include <abstractions/base>
  #include <abstractions/python>

  /usr/bin/python3.11                                 ix,
  /opt/deputyos/profiles/khoj/.venv/bin/python3*       ix,

  /opt/deputyos/profiles/khoj/**                       r,
  /opt/deputyos/profiles/khoj/.venv/lib/**.so*         mr,

  owner /home/agent/.khoj/                            rw,
  owner /home/agent/.khoj/**                          rwk,

  /etc/deputyos/secrets.env                            r,
  /etc/deputyos/profiles/khoj.toml                     r,
  /etc/deputyos/limits.json                            r,

  network inet stream,
  network inet6 stream,
  network unix stream,

  deny capability sys_admin,
  deny capability sys_module,
  deny capability sys_ptrace,
  deny /tmp/**           wx,
  deny /etc/shadow       rwlk,
}
```

### main.yml append

```yaml
- name: Apply Khoj profile bake
  ansible.builtin.import_tasks: profile-khoj.yml
  when: deputyos_profile == "khoj"
```

That single block is the entire dispatch wiring. No Rust imports, no
crate registration, no enum variant — `profile_switch::run("khoj", ...)`
already works because the profile is a string from the manifest.

### Reproduction commands

```sh
# from /home/<you>/Code/deputyos
cargo run -p deputyctl -- profile validate profiles/khoj.toml
cargo test -p deputyctl
ansible-lint roles/
make build TARGET=qemu-aarch64 PROFILE=khoj
make smoke TARGET=qemu-aarch64 PROFILE=khoj SMOKE_LEVEL=m1
make ci SCAFFOLD_PHASE=1
```

## Troubleshooting

!!! warning "AppArmor profile loads but the unit refuses to start"
    The profile binds to a binary path; if the venv/install layout
    differs from the path declared in the profile header, AppArmor
    rejects the exec. Run `aa-status | grep <id>` to confirm the
    profile is loaded, then `journalctl -u <id>-gateway.service` for
    the rejection reason. Fix the path in the AppArmor profile, not
    the unit.

!!! warning "`profile validate` exits 0 but `cargo test` fails"
    The validator is forgiving on unknown fields (so older deputyctl
    binaries can read newer manifests). The struct round-trip in
    `deputyctl::manifest::tests::roundtrip` is strict and will catch
    typos like `pinned_versions` (note the s) that the validator
    silently accepts. Always run `cargo test -p deputyctl` before
    committing.

!!! danger "Stub binary serves healthz but the real upstream agent does not"
    Smoke passes against the stub. The `.bake-meta` line `stub=true`
    surfaces in `deputyctl version` so this is detectable from the host.
    Investigate the bake log for the failed `pip install` step before
    publishing — a release with `stub=true` is a degraded image.

!!! tip "Force the stub for development"
    Pass `khoj_force_stub=true` (or `<id>_force_stub=true`) as a Packer
    `extra_var` to skip the real install entirely. Useful when the
    upstream package is yanked, unstable, or you are iterating on the
    AppArmor profile without waiting for a 5-minute pip install.

## Related

- [Concepts → Profile class](../concepts/profile-class.md)
- [Concepts → Plugin model](../concepts/plugin-model.md)
- [Reference → Schemas → profile.toml](../reference/schemas/profile-toml.md)
- [Reference → System → systemd units](../reference/system/systemd-units.md)
- [Reference → System → AppArmor profiles](../reference/system/apparmor-profiles.md)
- [How-to → Add a hardware target](add-a-hardware-target.md)
- [Build → Image bake internals](../build/image-bake-internals.md)
