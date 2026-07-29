# Plugin Model

deputyOS's plugin model is a contract: a third profile lands in the
appliance as **a manifest plus a bake recipe plus a systemd unit template
plus an AppArmor profile plus an offline-bake fallback** — and zero
changes to the Rust code. This page explains the contract and what
makes it possible.

## The promise

A new profile (in the [profile class](profile-class.md)) needs the
following six artefacts in the repo, and *nothing else*:

1. **`profiles/<id>.toml`** — the manifest.
2. **`roles/deputyos/tasks/profile-<id>.yml`** — the Ansible bake recipe.
3. **`roles/deputyos/templates/<id>-gateway.service.j2`** — the systemd
   unit template.
4. **`roles/deputyos/files/apparmor/deputyos.<id>`** — the AppArmor
   confinement profile.
5. **`roles/deputyos/files/<id>-stub.sh`** — the offline-bake fallback
   stub (used in offline / sandboxed builds where the upstream cannot
   be fetched).
6. **One line in `roles/deputyos/tasks/main.yml`** — `import_tasks:
   profile-<id>.yml` to make the build pipeline pick the new recipe up.

That is the entire contract. No Rust changes. No schema migrations.
No new wizard step types. No new HookKind variants.

!!! tip "If you find yourself editing Rust"
    If a profile PR needs to change `deputyctl`, the wizard, the PWA,
    the tracker, or the desktop launcher, that is a signal that either
    (a) the profile does not fit the [profile class](profile-class.md)
    and is asking for a special case, or (b) deputyOS itself needs an
    extension that *every* profile would benefit from. The right move
    is to split the PR — propose the deputyOS extension separately,
    with its own ADR. See [Contributing → Overview](../contributing/overview.md)
    for the ADR process.

## What makes this work

Three properties of deputyOS combine to keep the surface profile-agnostic:

### Profile-agnostic operator surface

[`deputyctl`](../reference/cli/deputyctl.md), [`deputywizard`](../reference/cli/deputywizard.md),
[`deputypwa`](../reference/cli/deputypwa.md),
[`deputyos-track`](../reference/cli/deputyos-track.md), and
[`deputyos-desktop`](../reference/cli/deputyos-desktop.md) — every Rust
crate — read profile data, never profile-specific logic. Switching the
active profile is reading a different TOML file at
`/etc/deputyos/profiles/<id>.toml` and starting a different systemd unit;
no command in the operator surface has an `if profile == "openclaw"`
branch.

The manifest schema is documented in
[Reference → Schemas → profile.toml](../reference/schemas/profile-toml.md).
The wizard reads `[wizard].prompts` to decide which questions to ask;
the doctor reads `[health]` and `[runtime]`; the up/down commands read
`[service]`; the AppArmor verifier reads `[apparmor]`. Everything is
data-driven.

### Profile-agnostic IPC

The [message relay](../reference/apis/message-relay.md) is a
line-delimited JSON socket protocol with four hook kinds — and that's
the entire IPC vocabulary between a profile and deputyOS. A profile that
emits hooks in the [documented payload schemas](../reference/schemas/hook-payloads.md)
is a first-class citizen of the appliance. The PWA, the cost ledger,
the doctor, and the channel-toggle surface all read from the relay and
do not care which profile produced the hook.

### Profile-agnostic build

The Ansible role at `roles/deputyos/` is the single source of truth for
what goes into an image. A new profile adds *one* `import_tasks` line
to `roles/deputyos/tasks/main.yml` and contributes its own task file.
Everything else — the runtime baseline, the security baseline, the
A/B partitioning, the systemd hardening, the offline cache — is shared
across every profile.

The variant gate (`when: hw == "<target>"`) is per-hardware-target, not
per-profile. A profile is built into every image; what differs across
hardware is the limits the profile runs under.

## The 6-file recipe at a glance

This is the inventory; the [How-to → Add a profile](../how-to/add-a-profile.md)
page walks each file step-by-step with a worked example (Khoj).

### 1. `profiles/<id>.toml` — the manifest

The schema is in [Reference → Schemas → profile.toml](../reference/schemas/profile-toml.md).
Required sections: `[profile]`, `[paths]`, `[runtime]`, `[service]`,
`[health]`, `[apparmor]`, `[wizard]`, `[channels]`, `[upgrade]`. The
`[memory]` and `[kernel]` sections are optional.

The manifest declares what is *already in the image*. It does not cause
anything to be installed — see [the architecture page](architecture.md)
for the zero-first-boot-network-installs invariant.

### 2. `roles/deputyos/tasks/profile-<id>.yml` — the bake recipe

This is where the agent actually lands on disk. The recipe runs at
build time inside the Ansible role. It must:

- Lay the agent down at `[paths].install_root` on the slot partition.
- Pre-resolve all native modules and Python wheels into the offline
  cache under `/var/cache/deputyos/`.
- Be reproducible — same inputs, same outputs.
- Be idempotent — running it twice produces the same image.
- Honour the offline-bake mode (use the stub fallback when the network
  is unavailable; details below).

The recipe is full Ansible — `pip`, `npm`, `git`, `unarchive`,
`get_url`, `command` — but everything happens at *build* time, never
at boot.

### 3. `roles/deputyos/templates/<id>-gateway.service.j2` — the systemd unit

A Jinja2 template rendered into `/etc/systemd/system/<id>-gateway.service`
at bake time. Required hardening directives:

- `User=agent`, `Group=agent` — never root.
- `ExecStart=` references `[paths].binary` from the manifest.
- `EnvironmentFile=/etc/deputyos/secrets.env` — provider keys live there
  at mode 0600, owned by root, readable by the gateway via supplementary
  group or systemd `LoadCredential`.
- `Restart=always`, `RestartSec=` matching the manifest's
  `[service].restart_policy`.
- `ProtectSystem=strict`, `ProtectHome=true`, `PrivateTmp=true`,
  `NoNewPrivileges=true`, `CapabilityBoundingSet=` (drop everything not
  needed), `SystemCallFilter=` (deny obvious bad classes).
- `ConditionPathExists=/etc/deputyos/profiles/<id>.toml` — the unit
  refuses to start if the profile manifest is not present.

The full hardening list is in [Reference → System → systemd units](../reference/system/systemd-units.md).

### 4. `roles/deputyos/files/apparmor/deputyos.<id>` — the AppArmor profile

The profile lives at `/etc/apparmor.d/deputyos.<id>` after bake and is
loaded into enforce mode at boot. It must allow:

- Read on `[paths].install_root` (the agent's read-only bake).
- Read/write on `[paths].data_dir` (the agent's persistent state).
- Read on `/etc/deputyos/secrets.env` (or via systemd credential
  forwarding).
- Network egress on the channels the profile actually uses.
- Whatever runtime-specific syscalls the agent's language stack needs.

It must deny everything else. AppArmor profiles are
[documented in detail](../reference/system/apparmor-profiles.md) with
per-rule rationale.

### 5. `roles/deputyos/files/<id>-stub.sh` — the offline-bake fallback

When CI builds in offline mode (no network, sandboxed runners), the
real bake recipe cannot fetch upstream. The stub is a small shell
script that is laid down at `[paths].binary` instead and prints a
clear message: "this profile was offline-baked; the agent binary is
not present; you cannot start this profile." That keeps the image
testable end-to-end (every other deputyOS surface still works) while
making the missing payload obvious.

The stub matters for the local-build-as-first-class workstream — a
contributor on a flaky cafe Wi-Fi can still reproduce an image and
exercise the appliance shape.

### 6. One-line addition to `roles/deputyos/tasks/main.yml`

```yaml
- name: Bake the <id> profile
  import_tasks: profile-<id>.yml
```

That's the only edit to a shared file. Everything else is new, profile-owned
artefacts.

## What M7 acceptance proved

Khoj was added to deputyOS as the M7 acceptance test of the plugin model.
The acceptance criteria were:

1. Khoj fits the [profile class](profile-class.md) (multi-channel gateway,
   persistent memory, skill/tool system) — yes.
2. The PR adds exactly the six artefacts above and modifies *only*
   `roles/deputyos/tasks/main.yml` (one new `import_tasks` line) and
   `README.md` / supporting docs. No Rust changes — confirmed.
3. `cargo test --all` is still green at 221 tests — confirmed; the
   number didn't move because no Rust code was touched.
4. The Khoj image boots in QEMU, the wizard runs, the gateway starts on
   port 42110, the PWA shows Khoj-branded chat — confirmed by the smoke
   harness.
5. `deputyctl profile switch khoj` and `deputyctl profile switch openclaw`
   both work on a multi-profile image — confirmed.

The Khoj manifest, recipe, AppArmor profile, systemd unit, and stub
remain in the repo as the reference example for any subsequent profile.

## What plugins *cannot* do

The plugin model is wide — a profile can pick its language, its
package manager, its channels, its memory backend, its required
sysctls — but it has hard edges. The following changes are *not*
plugin-extendable; they require Rust changes to deputyOS itself,
generally with an ADR:

- **New HookKind variants.** The relay's hook vocabulary is fixed; a
  profile that wants a fundamentally new event type is asking for an
  deputyOS-wide extension. The four current kinds are documented in
  [Reference → Schemas → hook payloads](../reference/schemas/hook-payloads.md).
- **New manifest schema fields.** Adding a field to `profile.toml`
  changes a Rust struct in `deputyctl/src/manifest.rs` and ripples to
  the wizard, the doctor, and (potentially) the tracker. That is an
  deputyOS extension, not a profile contribution.
- **New wizard step types.** The wizard renders steps from a fixed set
  of step kinds (`model_provider`, `channels`, `gateway_allowlist`,
  `backup_destination`). A profile that needs a brand-new step type
  needs an deputyOS extension.
- **New systemd hardening defaults, new AppArmor abstractions, new
  ufw chains.** These are appliance-wide invariants; per-profile
  loosening is fine inside an existing profile's AppArmor file, but
  changing the *defaults* is a security review, not a profile review.
- **New CLI commands on `deputyctl`.** Per-profile CLI helpers are not a
  thing — the operator surface is shared. If a profile needs to do
  something new, it does so via its own gateway's CLI (which
  [`deputyctl shell`](../reference/cli/deputyctl.md) drops into).

The bright line is: a profile contributes *what runs* and *how it is
confined*, not *how the appliance is operated*.

## Where to go next

- [How-to → Add a profile](../how-to/add-a-profile.md) — the worked
  step-by-step for the six files, using Khoj as the reference example.
- [Concepts → Profile class](profile-class.md) — the rule for what
  qualifies before you start.
- [Reference → Schemas → profile.toml](../reference/schemas/profile-toml.md)
  — every field of the manifest.
- [Reference → System → systemd units](../reference/system/systemd-units.md)
  — the hardening directives a unit template must include.
- [Reference → System → AppArmor profiles](../reference/system/apparmor-profiles.md)
  — the structure of the per-profile confinement file.
- [Reference → Schemas → hook payloads](../reference/schemas/hook-payloads.md)
  — the events a profile emits to integrate with the rest of the appliance.
