# Add a hardware target

## What this guide does

Add a new **hardware target** to deputyOS — a Pi variant, an x86 mini-PC,
a cloud snapshot, a QEMU smoke target, or anything else that needs its
own kernel, bootloader, firmware, governor, and per-target capability
limits.

A hardware target is a tuple of `(packer template, ansible variant
recipe, limits.json, optional smoke harness)` plus a one-line dispatch
in the role's main.yml. The shared Ansible role at `roles/deputyos/`
runs the security baseline, networking baseline, wizard baseline, and
hooks baseline first; then your variant tasks tune the hardware; then
the active profile bakes its agent code on top.

The worked example below is **rpi4**, the cleanest hardware target
because it reuses rpi5's kernel toolchain and shows the per-target
tuning (governor, kernel package, on-demand ClamAV, thermal notes)
without cloud-provider noise.

## Prerequisites

- A working contributor checkout — `make doctor` green, the existing
  variants linting and (where smokeable) booting.
- Familiarity with the Ansible role layout. See
  [Build → Image bake internals](../build/image-bake-internals.md).
- For real builds (not just lint): the relevant tooling installed —
  `packer` plus `qemu-system-*` for QEMU targets, `packer-builder-arm`
  for Pi targets, cloud provider CLIs (`doctl`, `oci`) for cloud
  targets.

## The recipe

Five files (some optional). All paths relative to the repo root.

### 1. Packer template — `packer/<hw>.pkr.hcl`

Defines the source image, the SSH/chroot session Packer uses to drive
Ansible, the post-processors that compress and sha256 the artefact,
and the output filename.

For QEMU targets: source from a Debian `nocloud` cloud image, use the
`qemu` builder, point `-drive` at the cloud-init seed staged in
`build/staging/cloud-init/`. See `packer/qemu-aarch64.pkr.hcl`.

For Pi targets: use `packer-builder-arm`, source from a Raspberry Pi
OS Lite arm64 image, work inside a `chroot` (no SSH session), and post-
process to `.img.xz`. See `packer/rpi5.pkr.hcl` and `packer/rpi4.pkr.hcl`.

For cloud targets: use the provider's Packer plugin (`digitalocean`,
`oracle-arm-free`). The output is a snapshot, not a local file.

The shared invocation pattern: every template runs the role with
`deputyos_hw=<hw>` and `deputyos_profile=<PROFILE>`, both injected via
Packer `var` blocks. The shared playbook lives at
`packer/playbook.yml`.

### 2. Variant recipe — `roles/deputyos/tasks/variant-<hw>.yml`

The Ansible task list that does the per-hardware tuning. The shape
depends on the target, but every variant recipe sets at least:

- `deputyos_target_ram_mb` — best-known RAM size (drives downstream
  thresholds; the runtime queries real RAM separately).
- `deputyos_target_storage_class` — `sd` / `emmc` / `ssd` / `nvme`.
- `deputyos_clamav_daemon_enabled` — `false` for low-RAM targets that
  can't afford clamd's RSS; the recipe then drops an on-demand
  `clamscan` timer instead.

Common per-target work:

- **CPU governor.** `cpufrequtils` install + `/etc/default/cpufrequtils`
  drop. Pi 4 / Pi 5 use `ondemand`; cloud variants use `performance`
  if the provider exposes the knob.
- **Kernel package.** Pi 4 → `linux-image-rpi-2711`; Pi 5 →
  `linux-image-rpi-2712`; arm64-generic → stock Debian
  `linux-image-arm64`.
- **Bootloader cmdline.** AppArmor + cgroup flags appended to
  `/boot/firmware/cmdline.txt` on Pi targets. QEMU and cloud variants
  inherit from the cloud-image defaults.
- **Per-target hacks.** Pi 4 ships an on-demand ClamAV timer (no
  daemon). WSL2 disables tunable nf_conntrack writes. macOS-qemu
  skips voice baseline gates.

### 3. Per-target limits — `roles/deputyos/files/limits.<hw>.json`

A JSON document that lands at `/etc/deputyos/limits.json` inside the
image. Schema is documented in
[Reference → Schemas → limits.json](../reference/schemas/limits-json.md);
the canonical struct is `deputyctl::limits::Limits` in
`deputyctl/src/limits.rs`.

Required fields:

```json
{
  "target": "<hw>",
  "tier": "low|standard|high",
  "ram_mb": 4096,
  "ram_class": "low|standard|high",
  "storage_class": "sd|emmc|ssd|nvme",
  "capabilities": {
    "local_llm":               false,
    "voice_wake_word":         false,
    "voice_tts":               false,
    "clamav_daemon":           false,
    "channels_heavy":          ["telegram", "slack"],
    "channels_disabled_by_ram": ["whatsapp-cloud-webhook"]
  },
  "limitations": [
    {"id": "no-local-llm", "reason": "<honest reason>", "unblock": "<what to upgrade to>"}
  ]
}
```

The `limitations` array is consumed by `deputyctl limits`, the wizard's
device-status page, and the PWA's hardware card. **Be honest** — every
limitation surfaces to the user. See the user-awareness principle in
the deputyOS limitations doc (`docs/14-limitations.md`).

### 4. Smoke harness — `test/smoke/<hw>.sh` (optional)

Required for any target that boots under QEMU on standard CI. Skip for
real-hardware-only targets (Pi 4 / Pi 5 — the chroot bake is the
build, but you can't smoke-boot a Pi image on x86 CI without nested
emulation that's too slow to be useful).

The harness sources `test/smoke/_common.sh` and asserts (at
`SMOKE_LEVEL=m1`):

- The image boots and the kernel is up.
- The wizard service is `active (running)`.
- The active profile's gateway is `active (running)`.
- `deputyctl status` exits 0.
- The healthz endpoint serves 200.

### 5. main.yml import

Append to the variant-dispatch block in
`roles/deputyos/tasks/main.yml`:

```yaml
- name: Apply <hw> variant tuning
  ansible.builtin.import_tasks: variant-<hw>.yml
  when: deputyos_hw == "<hw>"
```

This is the only repo-wide edit besides the four file additions above.

## Verification

```sh
# Lint
ansible-lint roles/
yamllint roles/

# Packer template parses
packer validate -syntax-only packer/<hw>.pkr.hcl

# limits.json is valid JSON and parses against the schema
jq . roles/deputyos/files/limits.<hw>.json
cargo test -p deputyctl limits

# Build (host must have the right tooling — see prerequisites)
make build TARGET=<hw> PROFILE=openclaw

# Smoke (only if test/smoke/<hw>.sh exists)
make smoke TARGET=<hw> PROFILE=openclaw SMOKE_LEVEL=m1

# CI surface
make ci SCAFFOLD_PHASE=1
```

## Worked example: rpi4

The rpi4 variant landed as part of the M1 hardware matrix. It is the
cleanest example because it reuses rpi5's chroot toolchain and
introduces only the per-target deltas that matter (CPU silicon,
kernel package, on-demand ClamAV, thermal notes).

### Files rpi4 added

| Step | File |
| --- | --- |
| 1 | `packer/rpi4.pkr.hcl` |
| 2 | `roles/deputyos/tasks/variant-rpi4.yml` |
| 3 | `roles/deputyos/files/limits.rpi4.json` |
| 4 | (no smoke — real-hardware target; not in `make matrix`) |
| 5 | one block in `roles/deputyos/tasks/main.yml` |

### variant-rpi4.yml highlights

```yaml
- name: Set rpi4 facts
  ansible.builtin.set_fact:
    deputyos_clamav_daemon_enabled: false      # 4GB RAM can't afford clamd
    deputyos_target_ram_mb: 4096
    deputyos_target_storage_class: "sd"

- name: Add AppArmor + cgroup flags to /boot/firmware/cmdline.txt
  ansible.builtin.replace:
    path: /boot/firmware/cmdline.txt
    regexp: '^(.*?)(\s*)$'
    replace: '\1 apparmor=1 security=apparmor cgroup_enable=memory cgroup_memory=1\2'

- name: Install Pi-4-family kernel package
  # Pi 5's linux-image-rpi-2712 will not boot on Pi 4 silicon (bcm2712 vs bcm2711).
  ansible.builtin.apt:
    name: linux-image-rpi-2711
    state: present

- name: Set CPU governor to ondemand (A72-tuned)
  ansible.builtin.copy:
    dest: /etc/default/cpufrequtils
    content: |
      GOVERNOR="ondemand"

- name: Install ClamAV (on-demand mode — no daemon)
  ansible.builtin.apt:
    name: clamav
    state: present

- name: Drop on-demand clamscan timer
  # Daily scan + randomized 30-min jitter so a fleet doesn't stampede.
  ansible.builtin.copy:
    dest: /etc/systemd/system/deputyos-clamscan.timer
    content: |
      [Timer]
      OnCalendar=daily
      RandomizedDelaySec=30m
      Persistent=true
```

### limits.rpi4.json highlights

```json
{
  "target": "rpi4",
  "tier": "low",
  "ram_mb": 4096,
  "ram_class": "low",
  "storage_class": "sd",
  "capabilities": {
    "local_llm": false,
    "voice_wake_word": false,
    "voice_tts": false,
    "clamav_daemon": false,
    "channels_disabled_by_ram": ["whatsapp-cloud-webhook", "discord-voice"]
  },
  "limitations": [
    {"id": "no-local-llm", "reason": "Pi 4 RAM (4 GB) and CPU (Cortex-A72) below local-LLM threshold",
     "unblock": "rpi5 8GB+ or x86_64-mini-pc; or run model via cloud provider"},
    {"id": "no-voice-wake-word", "reason": "Cortex-A72 too slow for real-time wake-word detection",
     "unblock": "rpi5 8GB+ or x86_64-mini-pc"},
    {"id": "no-voice-tts", "reason": "TTS thermal-throttles on passive-cooled Pi 4",
     "unblock": "rpi5 with active cooler, or x86_64-mini-pc"},
    {"id": "no-clamav-daemon", "reason": "clamd RSS exceeds Pi 4 RAM headroom; on-demand clamscan timer used instead",
     "unblock": "rpi5 8GB+ or x86_64-mini-pc"},
    {"id": "sd-storage-slow-under-update-load", "reason": "SD card I/O thrashes under simultaneous update + backup load",
     "unblock": "USB3 SSD recommended"}
  ]
}
```

Every limitation cross-references an unblock path. This is what
`deputyctl limits` and the wizard surface to the user.

### main.yml append

```yaml
- name: Apply rpi4 variant tuning
  ansible.builtin.import_tasks: variant-rpi4.yml
  when: deputyos_hw == "rpi4"
```

### Reproduction commands

```sh
ansible-lint roles/
packer validate -syntax-only packer/rpi4.pkr.hcl
jq . roles/deputyos/files/limits.rpi4.json

# Real build needs binfmt_misc + qemu-user-static and packer-builder-arm.
make build TARGET=rpi4 PROFILE=openclaw
```

## Per-target tuning notes

| Knob | rpi4 | rpi5 | x86_64-mini-pc | wsl2 | Cloud (DO / Hetzner / Vultr / Linode) |
| --- | --- | --- | --- | --- | --- |
| Kernel | `linux-image-rpi-2711` | `linux-image-rpi-2712` | stock Debian | host kernel | provider stock |
| Governor | `ondemand` | `ondemand` | `performance` | n/a | `performance` |
| ClamAV | on-demand timer | daemon | daemon | daemon | daemon |
| Voice | disabled | enabled (8GB+) | enabled | disabled | disabled |
| Local LLM | disabled | enabled (8GB+) | enabled (16GB+) | disabled | disabled |
| Update A/B | M6: U-Boot bootcount | M6: tryboot | M6: GRUB swap | n/a | provider snapshot |

!!! note "M6"
    The "M6:" rows are deferred to milestone 6. Today every target
    ships single-slot images. `deputyctl rollback` validates the
    inactive-slot integrity but refuses to swap until M6 lands the
    bootloader plumbing. See
    [Operations → Update and rollback](../operations/update-and-rollback.md).

## Troubleshooting

!!! warning "Packer build runs but the resulting image won't boot"
    Almost always a kernel-package mismatch. Pi 4 silicon (bcm2711)
    will not boot a kernel built for Pi 5 (bcm2712). Cloud snapshots
    that copy a Pi cmdline.txt likewise fail. Check `dmesg` from a
    serial console; the kernel panic is loud.

!!! warning "AppArmor doesn't enforce on the boot kernel"
    Pi targets need `apparmor=1 security=apparmor cgroup_enable=memory
    cgroup_memory=1` appended to `/boot/firmware/cmdline.txt`. Without
    it, the AppArmor service starts but no profile actually confines
    anything. Run `aa-status` after first boot to verify.

!!! tip "Use rpi4 as your template for any RAM-constrained target"
    The on-demand ClamAV timer pattern, the conservative `ondemand`
    governor, and the explicit `deputyos_clamav_daemon_enabled: false`
    fact are reusable. arm64-generic (the Orange Pi / Rock Pi shape)
    cribs from rpi4 directly.

## Related

- [Build → Image bake internals](../build/image-bake-internals.md)
- [Build → Make targets](../build/make-targets.md)
- [Reference → Schemas → limits.json](../reference/schemas/limits-json.md)
- [Distribution → Hardware matrix](../distribution/hardware-matrix.md)
- [How-to → Add a profile](add-a-profile.md)
