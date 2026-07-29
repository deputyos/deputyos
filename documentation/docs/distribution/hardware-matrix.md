# Hardware matrix

## What this page does

Enumerate every distribution channel deputyOS ships — what gets built
locally, what gets baked as a cloud snapshot, what gets distributed as
a community template, and what gets driven from your laptop via the
desktop launcher. For each row: what the artefact is, the
acceleration shape, the RAM tier, whether you can build it locally,
whether CI smoke-tests it, and the relevant per-row notes.

The matrix is the surface answer to "where can I run deputyOS?" Today
the answer is "11 ways" — five via local Packer build, three via
cloud-init recipes, one via OCI artefact, three via community
templates, and two via the desktop launcher.

## The full matrix

| Target | Format | Build kind | Acceleration | RAM tier | Local-buildable | Smoke in CI | Notes |
| --- | --- | --- | --- | --- | --- | --- | --- |
| `qemu-aarch64` | qcow2 | Packer / qemu | KVM (host arm64) or TCG | standard | yes | yes (M1) | smoke-test target; not for production |
| `qemu-x86_64` | qcow2 | Packer / qemu | KVM (host x86) or TCG | standard | yes | yes (M1) | smoke-test target; not for production |
| `rpi5` | img.xz | Packer / pi-gen | native | low / standard / high | yes (chroot) | no | recommended Pi target; needs binfmt_misc |
| `rpi4` | img.xz | Packer / pi-gen | native | low | yes (chroot) | no | constrained; on-demand ClamAV; no voice/local-LLM |
| `arm64-generic` | img.xz | Packer | native | varies | yes (chroot) | no | Orange Pi / Rock Pi / generic arm64 SBCs |
| `x86_64-mini-pc` | img.xz | Packer / qemu | KVM | standard / high | yes | no | mini-PCs, NUCs; same template as qemu-x86_64 with raw+xz post-process |
| `digitalocean` | DO snapshot | Packer / DO API | provider-managed | standard | yes (needs `DIGITALOCEAN_TOKEN`) | no | $DIGITALOCEAN_TOKEN required |
| `oracle-arm-free` | qcow2 | Packer | TCG | standard | yes (manual upload to OCI) | no | matches Oracle's free-tier Ampere shape |
| `wsl2` | tar.gz | Packer | WSL2 | standard | yes (Linux/WSL2 host) | no | Windows users import via `wsl --import` |
| `hetzner-cloud` | cloud-init recipe | recipe-only | provider | standard | n/a | no | paste YAML into Hetzner's User-Data |
| `vultr` | cloud-init recipe | recipe-only | provider | standard | n/a | no | paste YAML into Vultr's startup script |
| `linode` | cloud-init recipe | recipe-only | provider | standard | n/a | no | paste YAML into Linode's StackScript |
| `fly-machines` | OCI image | buildah / docker | container | standard | yes (buildah/docker on PATH) | no | `flyctl deploy` to push |
| `proxmox` | template + qcow2 | community template | KVM (Proxmox host) | standard | n/a (wraps qemu-x86_64) | no | follow `templates/proxmox/README.md` |
| `unraid` | template + qcow2 | community template | KVM | standard | n/a | no | follow `templates/unraid/README.md` |
| `truenas` | template + qcow2 | community template | bhyve | standard | n/a | no | follow `templates/truenas/README.md` |
| `macos-qemu` | qcow2 | wrapper around qemu-aarch64 | TCG (host x86_64) or HVF (host arm64) | standard | yes | no | run via `macos/run-utm.sh` or `macos/run-orbstack.sh` |
| desktop launcher / Linux | bin | local cargo | native | host | yes | no | downloads + drives qemu+KVM |
| desktop launcher / Windows | bin (cross) | cross-compile | host WSL2 | host | yes | no | mandates WSL2 |
| desktop launcher / macOS | bin (cross) | cross-compile | host UTM | host | yes (on Mac) | no | mandates UTM (Apple Silicon recommended) |

## Categories

### Build targets — real Packer

`qemu-aarch64`, `qemu-x86_64`, `rpi5`, `rpi4`, `arm64-generic`,
`x86_64-mini-pc`, `digitalocean`, `oracle-arm-free`, `wsl2`.

`make build TARGET=<hw>` produces a local artefact. Lint and
`packer validate` work without provider creds; real builds for cloud
targets need creds (see
[Build → Make targets](../build/make-targets.md)).

### Cloud-init recipes — no build

`hetzner-cloud`, `vultr`, `linode`.

These are not local builds. `make build TARGET=hetzner-cloud` prints
the YAML recipe and exits 0. The user pastes the YAML into the
provider's User-Data field at instance creation. The recipes fetch the
installer from `https://cdn.deputyos.com/install-cloud.sh`.

### Container — OCI

`fly-machines`.

`make build TARGET=fly-machines` produces an OCI image via buildah
(preferred — rootless, daemonless) or Docker. The user runs `flyctl
deploy` to push to Fly's registry. The Containerfile is at
`fly/Containerfile`.

### Community templates — manual import

`proxmox`, `unraid`, `truenas`.

These wrap an existing qcow2 (built from `qemu-x86_64` or
`qemu-aarch64`). The template files at `templates/<provider>/` plus
that provider's README walk through the import. deputyOS does not
build these directly; the user follows the README.

### Desktop launcher — download + run locally

The `deputyos-desktop` crate produces a Rust binary per platform that
consumes existing image artefacts. See
[Desktop launcher internals](desktop-launcher-internals.md).

`macos-qemu` (a special-case wrap of `qemu-aarch64`) is the qcow2 the
macOS launcher boots; the launcher itself is `deputyos-desktop` built
for macOS.

## Picking a target

| If you have… | Pick |
| --- | --- |
| A Pi 5 with 8GB+ RAM | `rpi5` |
| A Pi 5 with 4GB RAM | `rpi5` (low tier; voice off) |
| A Pi 4 (any RAM) | `rpi4` |
| An Orange Pi / Rock Pi / generic arm64 SBC | `arm64-generic` |
| An Intel NUC / x86 mini-PC | `x86_64-mini-pc` |
| A laptop and you want to evaluate quickly | desktop launcher |
| A DigitalOcean droplet | `digitalocean` |
| An Oracle free-tier Ampere VM | `oracle-arm-free` |
| A Hetzner / Vultr / Linode VPS | the matching cloud-init recipe |
| A Proxmox / Unraid / TrueNAS host | the matching community template |
| Fly.io | `fly-machines` |
| WSL2 on Windows | `wsl2` |

## Per-row notes

### `rpi5`

The flagship Pi target. Three RAM tiers (4 GB, 8 GB, 16 GB); 8 GB+ is
required for voice and local-LLM features. tryboot is the planned
update mechanism (M6).

### `rpi4`

Conservative single-tier (`low`). 4 GB RAM, Cortex-A72, no daemon
ClamAV (on-demand timer instead), no voice, no local LLM, U-Boot
bootcount A/B (M6). USB3 SSD recommended over SD for sustained I/O.

### `arm64-generic`

Generic arm64 SBCs (Orange Pi 5, Rock Pi 4/5, etc.). Inherits
rpi4-class limitations except where the board has ≥8 GB RAM. Stock
Debian arm64 kernel; no per-board tuning baked in (the user picks the
right device-tree post-flash).

### `x86_64-mini-pc`

Same Packer template as `qemu-x86_64` with a `qcow2 → raw → xz`
post-processor. Targets passively-cooled or mid-range mini-PCs (Intel
N100, AMD Ryzen embedded).

### `wsl2`

The Windows path. The output is a `.tar.gz` the user imports with
`wsl --import`. systemd works under WSL2 (with `[boot]
systemd=true`); ufw is more limited because the WSL2 kernel doesn't
expose every nf_conntrack tunable. The PowerShell installer at
`wsl/Install-DeputyOS.ps1` automates the import.

### `macos-qemu`

A wrapper. The qcow2 the macOS launcher boots is the `qemu-aarch64`
qcow2 — there's no separate Packer template. `scripts/build.sh
TARGET=macos-qemu` recursively dispatches to `TARGET=qemu-aarch64`
and copies the output. Run via `macos/run-utm.sh` (recommended) or
`macos/run-orbstack.sh`.

### `fly-machines`

Single-VM persistent app on Fly.io. The Containerfile expects
`DEPUTYOS_PROFILE` and `DEPUTYOS_CHANNEL` build args. State persistence
needs a Fly volume (see `fly/fly.toml.example`). No A/B updates —
`fly deploy` is the update mechanism.

### `proxmox` / `unraid` / `truenas`

Community-contributed templates. The qcow2 ships from the QEMU build;
the per-provider XML / JSON / VM-config defines the import shape. The
deputyOS team maintains the templates but does not run a Proxmox /
Unraid / TrueNAS host for end-to-end smoke testing — community
testing is the source of truth.

## Per-target capability map (the limits)

For the per-target capability flags (`local_llm`, `voice_wake_word`,
`voice_tts`, `clamav_daemon`, …), see
[Reference → Schemas → limits.json](../reference/schemas/limits-json.md)
and the per-target `roles/deputyos/files/limits.<hw>.json`. The wizard
reads `/etc/deputyos/limits.json` at first boot and filters every
relevant choice list.

## Future targets

Tracked in `docs/11-roadmap.md`:

- **`pi3-zero-2w`** — explicitly out of scope. Pi 3 / Pi Zero 2W
  cannot run AppArmor cleanly with our gateway profiles.
- **`riscv64`** — speculative; depends on stable Debian RISC-V
  release.
- **`bare-metal-arm64-laptop`** — Lenovo X13s and friends. Possible
  with mainline-only kernel; tracked but not committed.

## Related

- [Build → Make targets](../build/make-targets.md)
- [Build → Image bake internals](../build/image-bake-internals.md)
- [Distribution → Desktop launcher internals](desktop-launcher-internals.md)
- [Reference → Schemas → limits.json](../reference/schemas/limits-json.md)
- [How-to → Add a hardware target](../how-to/add-a-hardware-target.md)
