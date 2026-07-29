# Glossary

Every term used across this site, defined. Cross-links go to the page
where the term is treated in depth. If a term you expect is missing,
[open an issue](https://github.com/deputyos/deputyos/issues) — the
glossary is meant to be exhaustive for the published vocabulary.

## A

### A/B slot

The two read-only system partitions on a baked image — `slotA` and
`slotB`. One is active; the other is the rollback target. An update
writes the new image into the inactive slot, sets a one-shot boot
pointer, and reboots; if the new slot fails the watchdog, the
bootloader returns to the previous slot. See
[Operations → Update and rollback](operations/update-and-rollback.md).
Not every hardware target supports A/B; cloud targets use platform
snapshots or re-deploy instead.

### active profile

The profile the device is currently running. Stored as a symlink at
`/etc/deputyos/active-profile.toml` pointing into
`/etc/deputyos/profiles/<id>.toml`. Switched with
[`deputyctl profile switch <id>`](reference/cli/deputyctl.md). See
[Concepts → Profile class](concepts/profile-class.md).

### AppArmor profile

A kernel-enforced confinement description. deputyOS ships one per
profile (e.g. `deputyos.openclaw`, `deputyos.hermes`, `deputyos.khoj`)
plus one for the voice relay. Lives at `/etc/apparmor.d/deputyos.<id>`
on the device; loaded into enforce mode at boot. See
[Reference → System → AppArmor profiles](reference/system/apparmor-profiles.md).

## B

### baked image

An deputyOS image after the build pipeline has finished. It contains
every runtime, native module, signature DB, configuration template,
and profile bake the system needs. Booting a baked image is
deterministic and offline-capable. See
[Concepts → Architecture](concepts/architecture.md#the-image-as-the-unit-of-release).

### bake recipe

The Ansible task file at `roles/deputyos/tasks/profile-<id>.yml` that
lays a profile down at `/opt/deputyos/profiles/<id>/` *at build time*.
One of the [six artefacts](concepts/plugin-model.md) a new profile
contributes.

## C

### channel

A messaging surface a profile gateway speaks — Telegram, Slack,
Discord, Matrix, WhatsApp, Signal, IRC, email, web chat, etc. Each
profile's manifest declares a `[channels].supported` list; the wizard
asks which subset to enable; ufw and AppArmor are tightened to the
chosen subset. See
[How-to → Add a channel](how-to/add-a-channel.md).

### channel allowlist

The subset of a profile's supported channels that have been enabled by
the user. Persisted in the profile's data directory and surfaced in
the [PWA dashboard](reference/apis/pwa-http.md).

### CDN

The content-delivery network from which `deputyctl update` and the
desktop launcher fetch images and the signed manifest. deputyOS uses
Backblaze B2 fronted by Cloudflare; the URL pattern is documented in
[Security → Update trust chain](security/update-trust-chain.md).

### ClamAV

The signature-based antivirus daemon baked into every image. The
signature database is packed into the image at build time
(`/var/lib/clamav/`); `freshclam` is disabled at first boot — new
signatures arrive in the next image rev or via user opt-in. On
RAM-constrained targets (Pi 4 4GB) `clamd` is replaced by on-demand
`clamscan`. See
[Security → Default-on controls](security/default-on-controls.md).

### CostAlert hook

The hook kind emitted when cost ledger entries cross a configured
threshold or trip a guardrail. Payload schema in
[Reference → Schemas → hook payloads](reference/schemas/hook-payloads.md).
Behaviour and configuration in
[Operations → Cost guardrails](operations/cost-guardrails.md).

## D

### data partition

The read-write partition mounted as `/home/agent/` (and a few
adjacent paths) on a booted device. Persists across A/B image swaps
and across factory re-flashes of the system slots. Backed up to the
user's bucket via [`deputyctl backup`](reference/cli/deputyctl.md).

### default-deny

The posture for the inbound network chain (ufw) and for AppArmor:
deny everything, then allow the specific surfaces the configuration
calls out. See
[Security → Default-on controls](security/default-on-controls.md).

## F

### factory reset

The operation that wipes the data partition while preserving the
system slots. Triggered by [`deputyctl factory-reset`](reference/cli/deputyctl.md);
prompts unless `--yes`. See
[Security → Secrets storage](security/secrets-storage.md) for the
secrets-storage implications.

## G

### gateway

The active profile's running process — `openclaw onboard --daemon`,
`hermes gateway start`, `khoj --no-gui --host 127.0.0.1 --port 42110`.
Runs under its own systemd unit and AppArmor profile. The user-facing
agent lives inside the gateway process.

## H

### HookKind

The fixed enum of hook variants the [message relay](reference/apis/message-relay.md)
understands. The four kinds plus their JSON Schemas are in
[Reference → Schemas → hook payloads](reference/schemas/hook-payloads.md).
New variants require a Rust change to deputyOS and an ADR; they are
not plugin-extendable.

### host fwd

The qemu / launcher mechanism that forwards a host port to the guest's
`:8088` so the wizard and PWA are reachable from the laptop's browser.
Used by the desktop launcher and `make try`. See
[Distribution → Desktop launcher internals](distribution/desktop-launcher-internals.md).

## I

### image manifest

Two things live under this name; context disambiguates:

1. The **profile manifest** — the TOML at `profiles/<id>.toml`; see
   [Reference → Schemas → profile.toml](reference/schemas/profile-toml.md).
2. The **release manifest** — the JSON at
   `<cdn-base>/manifest-v1.json`; see
   [Reference → Schemas → release manifest](reference/schemas/release-manifest.md).

When this site says "manifest" without a qualifier, it's usually the
release manifest, because the trust chain centres on it.

## K

### KVM

The Linux Kernel Virtual Machine. The desktop launcher on Linux uses
qemu+KVM; the launcher's `make doctor` checks for `/dev/kvm` access.
See [Distribution → Desktop launcher internals](distribution/desktop-launcher-internals.md).

## L

### limits awareness (principle)

The deputyOS principle that **awareness of what the device cannot do
matters more than capability**. Every refusal includes a reason,
shown in the same UI surface where the user clicked. Surfaced in the
picker, the wizard, [`deputyctl limits`](reference/cli/deputyctl.md),
the [PWA "Your device" card](reference/apis/pwa-http.md), the
update flow, and the docs. See
[Concepts → Architecture](concepts/architecture.md) for the system view
and [Distribution → Hardware matrix](distribution/hardware-matrix.md)
for the per-target limits.

### limits.json

The authoritative per-target capability file at
`deputyctl/etc/limits.json`. One entry per supported hardware target;
each entry declares `target`, `tier`, `ram_mb`, `capabilities`,
`limitations`. Read by [`deputyctl limits`](reference/cli/deputyctl.md)
and the picker page. Schema in
[Reference → Schemas → limits.json](reference/schemas/limits-json.md).

## M

### Magika

Google's heuristic content-type identifier. Wired into the gateway's
file-upload path to expose content-type spoofing — a `.png` whose
bytes are an executable, for example. Model weights at
`/opt/deputyos/magika/`. See
[Security → Default-on controls](security/default-on-controls.md).

### manifest signature

The detached `.minisig` file accompanying the release manifest at the
CDN. Verified by `deputyctl update --check` and the desktop launcher
against the public key baked into every image. See
[Security → Update trust chain](security/update-trust-chain.md).

### mDNS

Multicast DNS — the protocol Avahi uses to advertise `deputyos.local`
on the LAN. Lets a phone or laptop find the wizard / PWA without
configuring DNS. Some enterprise / locked-down Wi-Fi blocks it; the
QR-on-TTY service prints an IP-form fallback URL.

### message relay

The Unix-socket service that brokers hooks between the gateway and
the rest of deputyOS (the PWA, the cost ledger, the doctor). Wire
format in [Reference → APIs → message relay](reference/apis/message-relay.md).

### minisign

The signature scheme deputyOS uses for release manifests and images.
Public key baked into every image; private key is held offline. See
[Security → Update trust chain](security/update-trust-chain.md).

## P

### profile (lowercase)

A general term for "an installable configuration" — including AppArmor
profiles. Disambiguated by context. When this site means the deputyOS
sense, it is capitalised or qualified ("the OpenClaw profile").

### Profile (capital P, deputyOS sense)

A personal AI assistant of the OpenClaw / Hermes / Khoj shape, packaged
for deputyOS as a six-artefact contribution. See
[Concepts → Profile class](concepts/profile-class.md) for the rule.

### profile class

The rule that defines what counts as a Profile in deputyOS:
multi-channel gateway + persistent memory + skill/tool system. PRs
that propose anything else get a redirection. See
[Concepts → Profile class](concepts/profile-class.md).

### providers.json

The authoritative file at `deputyctl/etc/providers.json` describing
every model provider the wizard knows about. Each entry declares
`id`, `display_name`, `kind`, `endpoint_default`, `key_env_var`,
`key_format`, `supported_models_hint`. Schema in
[Reference → Schemas → providers.json](reference/schemas/providers-json.md).

### PWA

The Progressive Web App at `/` on `:8088` after the wizard finishes —
the always-on dashboard plus the built-in private chat client. Served
by the [`deputypwa`](reference/cli/deputypwa.md) crate. Routes in
[Reference → APIs → PWA HTTP](reference/apis/pwa-http.md).

## Q

### qcow2

The QEMU Copy-On-Write disk format used for the macOS-qemu and
Proxmox/Unraid/TrueNAS targets. The desktop launcher on macOS expects
UTM (which speaks qcow2 natively).

### quiet hours

A user-configurable window during which cost guardrails are tighter
and notifications are deferred. Configured via [`deputyctl quiet-hours`](reference/cli/deputyctl.md).
See [Operations → Cost guardrails](operations/cost-guardrails.md).

## R

### relay socket

The Unix domain socket the [message relay](reference/apis/message-relay.md)
listens on. Path is `/run/deputyos/relay.sock`; permissions are 0660,
owned by `root:agent`.

### release-tracker

The [`deputyos-track`](reference/cli/deputyos-track.md) crate. Polls
upstream profile repositories every 30 minutes; opens a PR bumping
`profiles/<id>.toml`'s `pinned_version` when a new tag is observed.
Runs as a GitHub Action.

### runtime

The language runtime each profile depends on — Node 24 LTS for
OpenClaw, Python 3.11 for Hermes and Khoj. Declared in the profile
manifest's `[runtime]` section so [`deputyctl doctor`](reference/cli/deputyctl.md)
can verify versions. Never installed at boot; baked into the image.

## S

### secrets.env

The file at `/etc/deputyos/secrets.env` carrying provider API keys,
tunnel tokens, and backup credentials. Mode 0600, owned by root,
forwarded to the gateway via systemd credential mechanisms. Persists
across image updates. See
[Security → Secrets storage](security/secrets-storage.md).

### signed manifest

See [manifest signature](#manifest-signature) and
[release manifest](reference/schemas/release-manifest.md).

### signed update

An update where the release manifest has been verified against the
baked-in minisign public key before any new image is written to the
inactive slot. Signature verification is non-skippable. See
[Security → Update trust chain](security/update-trust-chain.md).

### SLSA

[Supply-chain Levels for Software Artifacts](https://slsa.dev/) — the
build-provenance framework. deputyOS targets SLSA L3 attestations as a
deferred milestone; the generator is in place, third-party
verification is the next step. See
[Security → Update trust chain](security/update-trust-chain.md).

### smoke test

A QEMU-based test that boots a freshly built image, runs the wizard
through a scripted sequence of answers, validates the gateway comes
up healthy, and tears down. One per supported hardware target;
fixtures under `test/qemu/`. See
[Contributing → Overview](contributing/overview.md).

### systemd unit

A systemd service description. deputyOS ships several:
`openclaw-gateway`, `hermes-gateway`, `khoj-gateway`, `deputywizard`,
`deputypwa`, `deputyos-qr-on-tty`, `deputyos-voice-relay`,
`agent-relay`, plus the platform's `avahi-daemon`. Hardening
directives and per-unit details in
[Reference → System → systemd units](reference/system/systemd-units.md).

## T

### target (build sense)

A supported hardware platform — `rpi5`, `rpi4`, `arm64-generic`,
`x86_64-mini-pc`, `wsl2`, `macos-qemu`, `digitalocean`, `hetzner`,
`vultr`, `linode`, `oracle-arm-free`, `fly-machines`,
`fly-machines+gpu`, plus accelerator variants like `rpi5+hailo`,
`rpi5+coral`. Each has a Packer template (or a cloud-init recipe), a
variant task file, and a smoke fixture. See
[Distribution → Hardware matrix](distribution/hardware-matrix.md).

### tier (capability sense)

A coarse capability level for a target — roughly, what local-LLM and
voice features it can run. Declared in
[`limits.json`](reference/schemas/limits-json.md) per target. Used
by the picker to filter, by the wizard to pre-disable toggles, and by
[`deputyctl limits`](reference/cli/deputyctl.md) to explain refusals.

### token (wizard token)

The bearer-style token the wizard prints (or shows on the QR code) so
the user's first request from another device on the LAN is
authenticated. Single-use, rotated when the wizard finishes. See
[Reference → APIs → wizard HTTP](reference/apis/wizard-http.md).

### tunnel

An optional outbound-initiated path that lets inbound channel webhooks
reach the agent across NAT or CGNAT. deputyOS supports Cloudflare Tunnel
and Tailscale Funnel. Configured via the wizard or
[`deputyctl tunnel`](reference/cli/deputyctl.md). See
[How-to → Set up a tunnel](how-to/set-up-tunnel.md).

## U

### UTM

The macOS-native virtualization frontend the desktop launcher mandates.
Free in the App Store. Speaks qcow2. See
[Distribution → Desktop launcher internals](distribution/desktop-launcher-internals.md).

## V

### variant gate

The `when: hw == "<target>"` condition on hardware-specific Ansible
tasks. Lets the shared role ship a baseline that is the same
everywhere and a per-target task file that diverges only where it
must. See [Build → Image bake internals](build/image-bake-internals.md).

## W

### watchdog

The boot-time process that gives a freshly booted slot five minutes to
reach `deputyctl health` green; if it fails, the bootloader rolls back
to the previous slot. See
[Operations → Update and rollback](operations/update-and-rollback.md).

### WSL2

Windows Subsystem for Linux 2 — the Windows hypervisor the desktop
launcher and the WSL2 image target both rely on. See
[Distribution → Hardware matrix](distribution/hardware-matrix.md) for
the WSL2 limitations (no audio, no LAN-mDNS).

## Z

### ZRAM

A compressed in-memory swap device. Enabled via `zram-tools` at 50%
of RAM on every deputyOS image; gives RAM-constrained targets graceful
overshoot under memory pressure without thrashing the SD card. See
[Concepts → Architecture](concepts/architecture.md) and
[Operations → Monitoring and logs](operations/monitoring-and-logs.md).
