# 00 — Architecture

## The single load-bearing invariant

> **Zero first-boot network installs.** No `apt`, `npm`, `pip`, `cargo`, or `git clone` runs after flashing. Every runtime, native module, model file, AV signature DB, and configuration template is already inside the image. The only network traffic on first boot is (1) DHCP/DNS/NTP, (2) the model provider you choose, and (3) optional Tailscale/Cloudflare Tunnel join if you opt in.

Everything else in this document follows from that constraint. Every install failure observed on these stacks (`@discordjs/opus` NEON build, `npm install openclaw@latest` failure, CMake too old, mirror outage, gateway PATH captured before tools install, `~/.zshrc` permission errors, missing data volumes) is a *first-boot install* problem. We make the class of problem disappear by not doing first-boot installs.

## System view

```
                     ┌────────────────────────────────────────────────────┐
                     │  Image partitions (read-only / r-w)                │
                     │  ┌────────────┐ ┌────────────┐ ┌────────────────┐  │
   flash → boot →    │  │ /boot/     │ │ slotA (ro) │ │ data (rw)      │  │
                     │  │ firmware   │ │ slotB (ro) │ │  /home/agent/  │  │
                     │  └────────────┘ └────────────┘ └────────────────┘  │
                     └────────────────────────────────────────────────────┘

                     ┌────────────────────────────────────────────────────┐
                     │  Runtime processes (under systemd, AppArmor enf.)  │
                     │                                                    │
                     │   deputyctl ──▶ openclaw-gateway.service            │
                     │       │                or                          │
                     │       │       hermes-gateway.service               │
                     │       │                                            │
                     │       ├──▶ wizard (axum :8088, mDNS deputyos.local) │
                     │       └──▶ doctor / update / backup / model        │
                     │                                                    │
                     │   clamd  (fanotify watch on uploads dir)           │
                     │   magika (called by gateway pre-write)             │
                     │   ufw    (default-deny, allow channels + LAN)      │
                     │   sshd   (key-only, no root, no password)          │
                     └────────────────────────────────────────────────────┘
```

`deputyctl` is the only management surface. It is a single Rust binary that reads a TOML profile manifest and drives a systemd user unit, a wizard server, and an update client.

## What's baked into every image

**Runtimes & toolchains** (version-pinned, never re-fetched)
- Node 24 LTS, with a populated offline npm cache for the pinned OpenClaw release.
- Python 3.11 + `uv`, with a populated wheel cache for the pinned Hermes release.
- CMake 3.28+, build-essential, git, jq, rclone, minisign, age.
- Prebuilt arm64 (or x86_64) native modules: `@discordjs/opus`, `better-sqlite3`, `node-canvas`, etc.

**Agent code** — the pinned profile versions are laid down at `/opt/deputyos/profiles/<id>/`. `deputyctl up` starts a systemd unit; nothing is installed on boot.

**Models & signatures**
- ClamAV with a packed signature DB at `/var/lib/clamav/`. `freshclam` is disabled at first boot — new signatures arrive in the next image rev or on user opt-in.
- Magika model weights at `/opt/deputyos/magika/`.
- (offline build only) `llama3.2:3b` for local Ollama.

**Kernel & system tuning** (pre-applied in `/etc/sysctl.d/90-deputyos.conf` and friends)
- `vm.swappiness=10`, `vm.vfs_cache_pressure=50`, `vm.dirty_ratio=10`
- `net.ipv4.tcp_congestion_control=bbr`, `net.core.default_qdisc=fq`
- `kernel.kptr_restrict=2`, `kernel.dmesg_restrict=1`, `kernel.yama.ptrace_scope=2`
- `kernel.unprivileged_userns_clone=1` (Hermes sandbox needs it)
- `net.ipv4.conf.all.rp_filter=1`, `net.ipv4.tcp_syncookies=1`
- `zram-tools` enabled at 50% RAM; 2 GB swapfile fallback when zram is unavailable.
- `iwconfig wlan0 power off` enforced via systemd oneshot.
- `loginctl enable-linger agent` baked in.

**Security baseline** — see [09-security.md](09-security.md) for the full enumeration.

**Reproducibility** — every image carries `/etc/deputyos/sbom.json` (CycloneDX) and `/etc/deputyos/build.json` (commit, builder, profile versions, ClamAV DB date, Magika model version).

**More-than-baseline tiers** — performance-sensitive add-ons (voice, local LLM, browser automation, NPU runtimes) ship per-target according to RAM and CPU envelope. Pi 5 16GB and Oracle ARM 24GB carry a 3B local LLM; Pi 5 8GB carries a 1B; Pi 4 stays lean. See [12-bundled-software.md](12-bundled-software.md) for the full per-target inclusion matrix and rationale.

**Memory pressure is a design point, not an afterthought.** On every limited-RAM target the working set will exceed physical RAM in normal use. The OS is configured (zram, cgroup limits per service, systemd-oomd with PSI thresholds, lazy-loaded tool processes, mmap-aware local-LLM service) to make overshoot graceful — the user sees a small latency spike at worst, never a kernel livelock or a random OOM kill of the agent. See [13-memory-pressure.md](13-memory-pressure.md) for component-level RSS budgets, per-target tuning, and the failure modes we guard against.

## Why this layout

| Choice | Reason |
|---|---|
| Single Rust binary `deputyctl` | One thing to ship, one thing to harden, one thing to verify against `cosign`. Profile-driven means new agents = manifest, not recompile. See [ADR-0001](adr/0001-one-binary-many-profiles.md). |
| systemd, not Docker, on the device | Containers add a layer of indirection, networking complexity, and a familiar set of "data volume forgotten" bugs. systemd already handles user-units, restarts, journals, and AppArmor. See [ADR-0004](adr/0004-systemd-not-docker-on-device.md). |
| A/B image swap, not package upgrade | Upgrading on-device means re-running `apt`/`npm`/`pip`, which is the whole class of problem we eliminated. A/B keeps the invariant intact. See [ADR-0007](adr/0007-ab-image-swap-not-package-upgrade.md). |
| pi-gen + packer-builder-arm | pi-gen is the official Pi imaging tool; packer-builder-arm extends it cleanly for hardware variants. See [ADR-0003](adr/0003-pi-gen-plus-packer-arm.md). |
| Backblaze B2 fronted by Cloudflare | Free egress through the Bandwidth Alliance; B2 storage is among the cheapest credible options. Same code paths work with R2. See [ADR-0005](adr/0005-b2-with-cloudflare-egress.md). |
| API-key-only wizard, no OAuth | OAuth flows on a headless device are hostile. API-key + base-URL is universal across every supported provider and validates with one round-trip. See [ADR-0006](adr/0006-no-oauth-in-wizard.md). |
| ClamAV + Magika as default-on baseline | ClamAV gives signature-based on-write scanning; Magika exposes content-type spoofing in agent file uploads. Together they cover two distinct threat shapes. See [ADR-0008](adr/0008-clamav-plus-magika-baseline.md). |

## Where data lives

| Path | What | Lifetime |
|---|---|---|
| `/opt/deputyos/profiles/<id>/` | Agent install, read-only on slot partition | per image rev |
| `/var/cache/deputyos/` | npm + pip + skill caches, read-only | per image rev |
| `/var/lib/clamav/` | Packed signature DB, read-only | per image rev |
| `/etc/deputyos/secrets.env` | Provider keys, mode 0600, root-owned | persists across updates |
| `/home/agent/.<profile>/` | User data: config, memory SQLite, skills | persists, backed up to B2/R2 |
| `/var/log/deputyos/` | Journal subset for the dashboard | rotated weekly |

The data partition is mounted separately and is never overwritten by an A/B image swap.

## Build pipeline at a glance

The shared Ansible role `roles/deputyos/` is the single source of truth for what goes into an image. Each Packer template (per hardware target) just picks a builder; the provisioning is identical.

```
                    ┌──────────────────────────────────┐
                    │  roles/deputyos/  (Ansible)       │
                    │  ─ runtimes & caches             │
                    │  ─ agent code at pinned version  │
                    │  ─ kernel tuning, security base  │
                    │  ─ ClamAV sig DB, Magika model   │
                    │  ─ deputyctl + profile manifests  │
                    └──────────────────────────────────┘
                              │
            ┌────────┬────────┼────────┬─────────┬────────┐
            ▼        ▼        ▼        ▼         ▼        ▼
         pi-gen  packer-  packer-  packer-    Fly.io  cloud-init
         (rpi5/  arm-img  arm-img  digital-   (OCI)   recipes
          rpi4)  (arm64-  (x86_64- ocean             (Hetzner,
                 generic) mini-pc)                    Vultr,
                                                      Linode,…)
            │        │        │        │         │        │
            └────────┴────────┴───────.img.xz / qcow2 / snapshot / OCI
```

CI runs QEMU smoke-tests against every artefact before publishing. See [03-image-builds.md](03-image-builds.md).

## Update flow

1. Release-tracker GH Action polls upstreams every 30 min.
2. New upstream tag → PR bumps `profiles/<id>.toml`.
3. Merge → matrix build → QEMU smoke gate → upload to B2 → signed `manifest.json`.
4. `deputyctl update --check` (run on a real device) reads the manifest, verifies signature, prompts.
5. `deputyctl update --apply` writes the new image into the inactive slot, sets one-shot boot pointer, reboots.
6. Watchdog: if `deputyctl health` fails to go green within 5 min of boot, the bootloader rolls back. See [08-update-rollback.md](08-update-rollback.md).
