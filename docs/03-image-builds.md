# 03 — Image builds

This doc describes how an `.img.xz` (or `.qcow2`, or DigitalOcean snapshot, or OCI artefact) is produced from this repository, why we use the toolchain we do, and how to add a new hardware target.

## Architectural lever

**One Ansible role provisions every image.** Hardware-specific tuning lives in variant tasks gated by Ansible `when:` conditions on a `hw` and a `cloud` fact. Per-target Packer templates differ only in their `source` block. This is the lever that prevents drift between Pi, x86, cloud, and QEMU outputs.

```
roles/deputyos/
├── defaults/main.yml             # role-wide defaults (hw=, cloud=, profile=)
├── tasks/
│   ├── main.yml                  # always-run: agent code, deputyctl, security baseline
│   ├── variant-rpi5.yml          # when: hw == "rpi5"
│   ├── variant-rpi4.yml          # when: hw == "rpi4"
│   ├── variant-arm64-generic.yml # when: hw == "arm64-generic"
│   ├── variant-x86_64-mini-pc.yml
│   ├── variant-wsl2.yml
│   ├── variant-digitalocean.yml  # when: cloud == "digitalocean"
│   ├── variant-oracle-arm.yml
│   ├── variant-hetzner.yml
│   ├── variant-fly.yml
│   ├── profile-openclaw.yml      # when: profile == "openclaw"
│   └── profile-hermes.yml
├── templates/
│   ├── openclaw-gateway.service.j2
│   ├── hermes-gateway.service.j2
│   ├── 90-deputyos.conf.j2        # sysctl
│   └── ufw.rules.j2
└── files/
    ├── apparmor/deputyos.openclaw
    ├── apparmor/deputyos.hermes
    └── clamav/<packed signature DB at build time>
```

A new hardware target = one new `variant-<hw>.yml` + one new `packer/<hw>.pkr.hcl` + one new CI matrix row. The shared role does not change.

## The build matrix

| Target | Image format | Tooling | Status (M0) |
|---|---|---|---|
| `rpi5` (8 / 16 GB) | `.img.xz` | pi-gen arm64 base + packer-builder-arm | designed, builds in M1 |
| `rpi5+hailo`, `rpi5+coral` | `.img.xz` (NPU variants) | same + accelerator userspace | designed, builds in M4 |
| `rpi4` (4 / 8 GB) | `.img.xz` | pi-gen arm64 base + packer-builder-arm | designed, builds in M2 |
| `arm64-generic` | `.img.xz` | Debian 12 arm64 base + packer-builder-arm | designed, builds in M2 |
| `x86_64-mini-pc` | `.img.xz` | Debian 12 amd64 + Packer QEMU + UEFI | designed, builds in M2 |
| `wsl2` | distro tarball | Packer null + `wsl --import` | designed, builds in M2 |
| `macos-qemu` | `.qcow2` + launcher script | Packer QEMU aarch64 | designed, builds in M2 |
| `digitalocean` | DO snapshot → 1-Click | Packer DigitalOcean builder | designed, builds in M2; submission in M5 |
| `oracle-arm-free` | bootable cloud image | Packer QEMU aarch64 → Oracle import | designed, builds in M2 |
| `hetzner-cloud` | cloud-init recipe | published YAML, no custom snapshot | designed, builds in M2 |
| `fly-machines` | OCI artefact | `flyctl` + Buildah | designed, builds in M2 |
| `vultr` / `linode` | cloud-init recipe | published YAML | designed, builds in M2 |
| `proxmox` / `unraid` / `truenas` | deployment templates wrapping qcow2 | reuse `macos-qemu` qcow2 | designed, builds in M2 |
| `qemu-aarch64` / `qemu-x86_64` | `.qcow2` (CI only) | Packer QEMU | designed, builds in M1 |

## Per-target tuning highlights

| Setting | rpi5 | rpi4 | arm64-generic | x86_64-mini-pc | digitalocean |
|---|---|---|---|---|---|
| Kernel | `linux-image-rpi-2712` 6.6+ | `linux-image-rpi-2711` 6.6+ | mainline arm64 | mainline amd64 | DO Ubuntu 24.04 stock |
| Bootloader / A-B | `tryboot` (Pi 5 firmware) | U-Boot + `bootcount` | U-Boot + `bootcount` | systemd-boot + `boot-counting` | snapshot rollback |
| Storage path | NVMe via PCIe HAT preferred; SD fallback | USB3 SSD preferred; SD fallback | depends on board | M.2 NVMe / SATA | DO block storage |
| zram | 50% RAM, lz4 | 50% RAM, zstd | 50% RAM, lz4 | 50% RAM, lz4 | disabled |
| Swapfile | 2 GB on data partition | 2 GB on data partition | 2 GB on data partition | 2 GB on data partition | none |
| CPU governor | ondemand, A76-tuned | ondemand, A72-tuned | ondemand | schedutil | n/a |
| Crypto build flags | `-march=armv8.2-a+crypto` | `-march=armv8-a+crypto` | `-march=armv8-a` | `-march=x86-64-v3` | `-march=x86-64-v3` |
| WiFi power-save | force-off oneshot | force-off oneshot | force-off oneshot | force-off oneshot | n/a |
| Thermal | Pi 5 active-cooler PWM curve | passive only; warn on throttle | per-board | mini-PC default | n/a |
| Firmware blobs | `bcm2712` | `bcm2711` | none | none | none |

## Why pi-gen + packer-builder-arm

- `pi-gen` is the official tool for producing Raspberry Pi OS images. Using it instead of building from scratch means we inherit Pi-firmware updates and kernel packaging maintained by the upstream, and our images stay compatible with `rpi-eeprom`, `rpi-imager`, and `tryboot`.
- `packer-builder-arm` (`mkaczanowski/packer-builder-arm`) lets us overlay our role on top of a pi-gen base or a Debian/Ubuntu arm64 base in a hermetic, reproducible way. It runs in a privileged container with `qemu-user-static` providing arm64 emulation on x86 builders.
- The DigitalOcean Packer builder takes a similar form — start from a stock Ubuntu base, apply our role, snapshot. Same provisioner, different builder block.

This combination is the lowest-overhead way to support both "boot a Pi" and "click Deploy on DO" from a single source. See [ADR-0003](adr/0003-pi-gen-plus-packer-arm.md) for alternatives we considered.

## Hermetic build properties

To make A/B updates and SLSA L3 attestations work, builds must reproduce. We pin:

- Base image SHA (pi-gen tag, Ubuntu point release).
- Node, Python, CMake versions (and their Debian package SHAs).
- npm and pip lockfiles for the agent's pinned version.
- ClamAV signature DB snapshot at build time.
- Magika model version.
- All native modules, prebuilt at build time and cached.

CI rejects builds that don't reproduce. A second builder must produce a byte-identical artefact (modulo timestamps captured by the build's `SOURCE_DATE_EPOCH`).

## QEMU smoke test (the gate)

Every produced artefact is booted in QEMU before publish. The smoke test asserts:

1. Kernel boots, `deputyctl` is on PATH.
2. The proprietary `deputyd` resident is active and its owner-only socket
   reports the expected protocol.
3. The resident can perform a cooperative pause/resume cycle and advertises
   backup, tunnel, terminal, reconciliation, and recovery capabilities.
4. systemd reaches `multi-user.target` within 90 s.
5. `deputyctl doctor` exits zero.
6. `ufw status` is `active`, default `deny`.
7. Wizard is reachable on `:8088`; `/healthz` returns 200.
8. After a non-interactive cloud-init userdata run, the gateway port responds
   and a synthetic Telegram-like message round-trips against a stub provider.

A failing smoke test blocks publish, full stop.

## Picker page

The picker at `deputyos.com` is a static page on Cloudflare Pages. It reads the latest signed `manifest.json` from the project B2 bucket and renders the right artefact link for the user's chosen device + agent + channel. This means the picker is always in sync with what's actually published — there's no separate database to drift from the bucket.

**The picker also surfaces per-target limitations** from [14-limitations.md](14-limitations.md) before download. Picking "Pi 4 4GB" shows a "What this can't do" panel listing no-local-LLM, no-voice, on-demand-clamscan, etc., with one-line "unblock" hints (upgrade to Pi 4 8GB, Pi 5, etc.). The panel is collapsible but not hideable — users see the constraints once before they commit to an artefact.

## Local builds (every contributor, every laptop)

The same shared Ansible role and Packer templates can build an explicitly
marked open-source development base on a contributor's laptop. Set
`DEPUTYOS_IMAGE_KIND=agentless-dev DEPUTYOS_ALLOW_AGENTLESS_DEV=1`; signing and
manifest tooling deliberately rejects that output. Official images are built
only by the private `deputyos-core` pipeline, which injects the mandatory
resident payload before running these templates. Full details are in
[15-local-build.md](15-local-build.md).

## Build outputs (naming)

```
deputyos-<profile>-<hw>-<version>-<channel>.img.xz
deputyos-<profile>-<hw>-<version>-<channel>.img.xz.sha256
deputyos-<profile>-<hw>-<version>-<channel>.img.xz.minisig
deputyos-<profile>-<hw>-<version>-<channel>.img.xz.cosign.bundle
manifest.json                        # one per release; lists every artefact
manifest.json.minisig                # detached signature
```

## Adding a new hardware target

1. Add `roles/deputyos/tasks/variant-<hw>.yml` with `when: hw == "<hw>"` gates on the imports.
2. Add `packer/<hw>.pkr.hcl` (or a cloud-init recipe under `cloud-init/<hw>.yaml`).
3. Add a CI matrix row in `.github/workflows/build.yml`.
4. Add a smoke-test fixture under `test/qemu/<hw>.cloudinit.yaml`.
5. Document the per-target install path in [01-getting-started.md](01-getting-started.md).

There is no Rust change, no new manifest schema, and no fork of the Ansible role.
