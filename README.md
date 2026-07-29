# deputyOS

A batteries-included appliance image for running open-source personal AI assistants — **OpenClaw** and **Hermes Agent** today, more later — on a Raspberry Pi, an x86 mini-PC, or a $5 cloud VPS.

You flash an image. You boot it. A wizard asks for your model provider key and which messaging channels to wire up. Five minutes later you're chatting with your own assistant from Telegram. There is no `npm install`, no `pip`, no `apt`, no Docker. The image already has everything inside it.

## Open source and official images

This public repository owns the cross-platform desktop/CLI, image construction
interfaces, security baseline, wizard, updates, and local backup. It does not
publish the source of the privileged deputyOS resident agent.

Every official downloadable deputyOS image nevertheless includes and starts
that resident agent, including its external tunnel, native agent WebUI proxy,
advanced terminal, update, recovery, and self-healing capabilities. The private
`deputyos/deputyos-core` pipeline supplies those binaries through a local,
fail-closed staging contract. The public builder never downloads or discovers
private code, and it cannot produce an official image without the private
payload.

Contributors can explicitly build an `agentless-dev` base to work on the open
source image layer. Such output is development-only, visibly marked, and
blocked from the signing and release path. Stripe entitlements govern paid
cloud, business, and enterprise capabilities at runtime; they do not govern
whether the resident agent is present.

> **Public preview**: marketing + curated docs + blog at <https://www.deputyos.com> (separate repo: [`deputyos/www-deputyos-com`](https://github.com/deputyos/www-deputyos-com)) · full technical reference at <https://docs.deputyos.com> (built from `documentation/` in this repo) · build status at <https://api.deputyos.com> · signed artefacts at <https://cdn.deputyos.com>. Security: see [`SECURITY.md`](SECURITY.md).
>
> **New tier**: `make build … AIRGAP=1` bakes [LFM2](documentation/docs/concepts/airgap.md) via `llama.cpp` so the image works with `-net none`. Per-tier defaults: lean → LFM2-350M, standard → LFM2-1.2B, rich → LFM2-2.6B + Qwen-Coder-1.5B.

## Why this exists

Both OpenClaw and Hermes Agent are excellent — but installing them today is a string of failure points that lose semi-technical users:

- `@discordjs/opus` won't compile on arm64
- `npm install openclaw@latest` fails on a fresh Pi
- CMake is too old, mirrors are down, swap isn't configured
- Service won't start because `loginctl enable-linger` was never run
- The gateway captures a stale PATH and silently misroutes commands
- The agent gets exposed to the world because nobody set the allowlist

deputyOS makes the entire class of "first-boot install problem" disappear by **never doing first-boot installs**. Every runtime, native module, agent binary, ClamAV signature DB, and configuration template is already in the image. The only network traffic on first boot is DHCP/DNS/NTP and the model provider you chose.

## What you get

- **Pre-tuned kernel** with hardened sysctls, BBR, zram, the right swappiness for an embedded box.
- **Security baseline on by default**: AppArmor enforce, `ufw` default-deny, `fail2ban`, ClamAV running with packed signatures, Google Magika wired into the agent's file-upload path, key-only SSH, no default password.
- **One Rust manager binary** — `deputyctl` — for everything you'd want to do: bring the agent up, switch profiles, rotate model keys, schedule backups, run a health check, apply an update.
- **A/B image updates** signed with `minisign` + `cosign`. If the new image fails to come up, the bootloader rolls back automatically. You don't lose data — your home directory lives on a separate partition.
- **First-boot wizard** on `deputyos.local` (mDNS) plus a QR code on the TTY/HDMI for phone-driven setup. No SSH required.
- **Built-in private web chat** at `deputyos.local/chat` so you can talk to your agent immediately, even before wiring up Telegram or Slack.
- **Backups to your own Backblaze B2 or Cloudflare R2 bucket** on a schedule, via rclone. Free egress through the Bandwidth Alliance.
- **Audit evidence spool** via `deputyctl audit`: device events are written locally as JSONL, then flushed to the cloud API for zstd-compressed object-storage retention and compliance queries.

## Pick your hardware

deputyOS publishes pre-optimised images for every supported target. You download the one for your device — no post-flash tuning step.

| You have | Download |
|---|---|
| Raspberry Pi 5 (8 / 16 GB) | `deputyos-<profile>-rpi5-<version>-stable.img.xz` |
| Raspberry Pi 4 (4 / 8 GB) | `deputyos-<profile>-rpi4-<version>-stable.img.xz` |
| Other arm64 SBC (Radxa, Orange Pi, Khadas, Le Potato…) | `deputyos-<profile>-arm64-generic-<version>-stable.img.xz` |
| x86 mini-PC (Beelink / MeLE / NUC) | `deputyos-<profile>-x86_64-mini-pc-<version>-stable.img.xz` |
| Windows (WSL2) | `wsl --install -d deputyos` |
| macOS (UTM / OrbStack) | `deputyos-<profile>-macos-qemu.qcow2` |
| DigitalOcean | 1-Click Marketplace listing |
| Oracle Cloud Always-Free arm | `deputyos-<profile>-oracle-arm-free.img` |
| Hetzner / Vultr / Linode | `cloud-init` recipe |
| Fly.io | OCI artefact + `fly launch` recipe |
| Proxmox / Unraid / TrueNAS | deployment templates wrapping the official qcow2 |

The picker page at [deputyos.com](https://www.deputyos.com) (target domain) reads the latest signed manifest and gives you the right artefact.

## Quickstart (Raspberry Pi 5)

1. Download `deputyos-openclaw-rpi5-<version>-stable.img.xz` and verify the `minisig` signature.
2. Flash to an SD card or NVMe with Raspberry Pi Imager (or `dd`).
3. Edit `/boot/firmware/deputyos.yaml` on the FAT partition: set WiFi SSID/PSK, hostname, and your SSH public key (optional).
4. Boot the Pi. Within ~60 seconds the wizard publishes itself at `http://deputyos.local`.
5. Open the URL on any device on your LAN (or scan the QR shown on HDMI). The wizard asks for your model provider key and validates it with a real round-trip.
6. Start chatting at `http://deputyos.local/chat`, or wire up Telegram/Slack/Discord/etc. from the wizard.

## Pick your agent

| Profile | What it is | Upstream |
|---|---|---|
| `openclaw` | Personal AI assistant with broad messaging-channel support; lobster-themed. | [openclaw/openclaw](https://github.com/openclaw/openclaw) |
| `hermes` | Self-improving agent with FTS5 memory, skill creation, and 17+ gateways. | [NousResearch/hermes-agent](https://github.com/NousResearch/hermes-agent) |
| _more profiles_ | Any project in the same class as the two above (multi-channel personal assistant + persistent memory + skills). Marketplace lands in M7. | community-maintained |

Want a different agent? See [docs/02-profiles.md](docs/02-profiles.md) for the manifest format. Note the [profile-class rule](CONTRIBUTING.md#profile-class) — IDE coding tools and agent frameworks belong elsewhere.

## Pick your model provider

Wizard supports OpenRouter, Anthropic, OpenAI, Google AI Studio, AWS Bedrock, Nous Portal, NVIDIA NIM, Ollama (cloud or local), z.ai/GLM, Kimi/Moonshot, MiniMax, Xiaomi MiMo, Hugging Face Inference, and any custom OpenAI-compatible endpoint. Full list and key formats in [docs/05-model-providers.md](docs/05-model-providers.md).

## Documentation

- [00 — Architecture](docs/00-architecture.md)
- [01 — Getting started](docs/01-getting-started.md)
- [02 — Profiles](docs/02-profiles.md)
- [03 — Image builds](docs/03-image-builds.md)
- [04 — Release tracking](docs/04-release-tracking.md)
- [05 — Model providers](docs/05-model-providers.md)
- [06 — Storage and backup](docs/06-storage-and-backup.md)
- [07 — Networking](docs/07-networking.md)
- [08 — Update and rollback](docs/08-update-rollback.md)
- [09 — Security](docs/09-security.md)
- [10 — Troubleshooting](docs/10-troubleshooting.md)
- [11 — Roadmap](docs/11-roadmap.md)
- [12 — Bundled software (per-target deep analysis)](docs/12-bundled-software.md)
- [13 — Memory pressure (designing for beyond-RAM operation)](docs/13-memory-pressure.md)
- [14 — Limitations (the awareness map)](docs/14-limitations.md)
- [15 — Local build (any contributor, any laptop)](docs/15-local-build.md)
- [Architecture Decision Records](docs/adr/)

## Limitations (read this before flashing)

We treat **awareness of limitations as more important than capability**. A semi-technical user who hits an unexplained failure thinks the project is broken; a user told upfront what their device cannot do feels respected and trusts the rest.

Per-target highlights — the full map lives in [docs/14-limitations.md](docs/14-limitations.md):

| Target | What it cannot do |
|---|---|
| Pi 4 (4 GB) | No local LLM; no voice; no persistent ClamAV daemon (on-demand `clamscan` instead); heavy channels (WhatsApp Cloud webhook, Discord+voice) default off. |
| Pi 4 (8 GB) | No local LLM (CPU too slow). Voice limited to Whisper `tiny.en`. |
| Pi 5 (8 GB) | Local LLM limited to 1B Q4. Voice up to `base.en`. mmap'd LLM on SD card thrashes — NVMe / USB SSD strongly recommended. |
| Pi 5 (16 GB) | `small.en` voice thermal-throttles. No verified boot on Pi firmware. |
| `arm64-generic` | Best-effort; per-board quirks the user inherits. No NPU runtimes bundled. |
| `x86_64-mini-pc` | OpenVINO acceleration only — no NVIDIA/AMD discrete-GPU tooling unless you build your own. |
| `wsl2` | No audio (Windows doesn't pass `/dev/snd` to WSL2 by default); voice features disabled. |
| `macos-qemu` | Demo-only; voice and NPU disabled; performance throttled by qemu. |
| `digitalocean`, `hetzner`, `vultr`, `linode`, `oracle-arm-free` | No audio → no voice. No A/B partitions on most cloud targets — rollback is via snapshot. Oracle's "Always Free" tier may reclaim idle instances. |
| `fly-machines` | Ephemeral; data partition is a Fly volume that can be deleted. Cold-start latency. |
| All targets | No OAuth model providers (API keys only — see [ADR-0006](docs/adr/0006-no-oauth-in-wizard.md)). Whole-snapshot restore only — no granular recovery. Updates are full image swaps (1.5–4 GB), not deltas. ClamAV is signature-based; Magika is heuristic. No full-disk encryption by default (opt-in at M6+). |

The picker page, wizard, `deputyctl limits`, and PWA "Your device" card all surface these limits in context — you should not need to read this README to find out something doesn't work.

## Try it without buying a Pi

**Easiest — download the deputyOS Console** from
[GitHub Releases](https://github.com/deputyos/deputyos/releases/latest). The
Console uses your platform's native virtualization:

| Platform | Prereq | Installer |
|---|---|---|
| Windows 10 21H2+ / Windows 11 | WSL2 (`wsl --install`) | `.msi` or setup `.exe` |
| macOS (Apple Silicon) | UTM ([free in App Store](https://mac.getutm.app/)) | `.dmg` |
| Linux x86_64 | qemu + KVM (`apt install qemu-system-x86 cpu-checker`) | `.AppImage` or `.deb` |

Install and open it. The Console checks the prerequisite, downloads the latest
deputyOS image (~2 GB, one time), boots it locally, and opens the wizard in your
browser. Pre-release installers are not yet code-signed; the
[Console guide](documentation/docs/how-to/operate/desktop-console.md) explains
the one-time OS warning and the CLI alternative.

The launcher mandates the platform-native hypervisor — no bundled QEMU. If the prereq is missing, the launcher prints the exact one-liner to install it. This keeps the binary tiny (~5 MB instead of ~80 MB) and your machine clean. See [docs/11-roadmap.md § M2.5](docs/11-roadmap.md) for the full architecture rationale.

**Alternative — script** (for CI / SSH / terminal-only environments):

```sh
curl -fsSL https://www.deputyos.com/try.sh | bash
```

Forwards the wizard to `http://localhost:8088`. Works in **UTM on Apple Silicon**, **qemu on Linux**, and **WSL2 on Windows**.

**Alternative — build from source**:

```sh
git clone https://github.com/deputyos/deputyos.git
cd deputyos
make doctor                    # checks Packer / Ansible / qemu / Docker
DEPUTYOS_IMAGE_KIND=agentless-dev DEPUTYOS_ALLOW_AGENTLESS_DEV=1 \
  make try TARGET=qemu-aarch64 # build public development base + boot it
```

The same public Ansible role used by the private release pipeline runs on your
laptop. Official images also contain the proprietary resident payload and are
verified through signatures, SBOMs, provenance, and payload checksums. Full
guide: [docs/15-local-build.md](docs/15-local-build.md).

## Local Control-Plane Docker

For end-to-end API + website testing from this repo, keep the three sibling
repos checked out next to each other:

```sh
Github/
  deputyOS/
  api-deputyos-com/
  www-deputyos-com/
```

Then run:

```sh
docker compose -f docker-compose.dev.yml up --build
```

This starts the Rust API at `http://localhost:3000` with a local compressed
object-lake volume, and the Astro website/dashboard at `http://localhost:4321`
pointed at that API. This Docker path is for validating the cloud control
plane; deputyOS devices still run profiles directly under systemd, not Docker.

## Status

**Production hardening in progress.** The Rust workspace, first-boot wizard, PWA, desktop launcher, release tooling, and image-bake scaffolding are implemented and under active validation. Public release is blocked on full matrix image builds, operator-provisioned signing/CDN secrets, signed CDN publication, and live API tests. See [docs/11-roadmap.md](docs/11-roadmap.md) for milestone detail.

## Licence

Apache-2.0. Compatible with the MIT licences of OpenClaw and Hermes Agent.

## Credits

deputyOS is not affiliated with the OpenClaw or NousResearch teams. We track their releases, package them well, and stand on their shoulders.
