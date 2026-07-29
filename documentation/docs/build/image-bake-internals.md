# Image bake internals

## What this page does

Walk through how an deputyOS image is **baked** end to end — the
shared Ansible role, the variant gates, the per-Packer-template
invocation, the pre-build staging in `scripts/build.sh`, and the
hermetic-build invariants. This page is for contributors who want to
understand the build system enough to extend it (new hardware
target, new profile, new policy baseline).

## The shared Ansible role + variant gates pattern

Every shipped image is the same Debian 12 (bookworm) base plus the
deputyOS Ansible role at `roles/deputyos/`. The role applies in a
fixed order:

1. **`security-baseline.yml`** — sysctls, ufw, fail2ban, SSH
   hardening, AppArmor framework, zram, ClamAV, Magika, the agent
   user, `/etc/deputyos/` layout, `deputyctl` binary install.
2. **`networking-baseline.yml`** — mDNS via avahi (LAN targets only),
   Cloudflare Tunnel + Tailscale binaries (installed but not started).
3. **`wizard-baseline.yml`** — `deputywizard` binary install +
   first-boot systemd unit + qr-on-tty unit (LAN targets).
4. **`hooks-baseline.yml`** — `/etc/deputyos/hooks.d/<kind>/`
   directory tree creation.
5. **`voice-baseline.yml`** (gated) — whisper.cpp + Piper + voice-relay
   shell bridge + `deputyos-voice-relay.service` + AppArmor profile.
   Self-gates on `deputyos_voice_enabled` and the `deputyos_no_voice_hw`
   list.
6. **`variant-<hw>.yml`** — per-target tuning (kernel package,
   bootloader cmdline, governor, on-demand vs daemon ClamAV, voice
   gates, …).
7. **`profile-<id>.yml`** — agent code install, gateway systemd unit,
   AppArmor profile, `.bake-meta`.

Each step is idempotent. The role never starts gateways at bake time
— first boot of the image is what runs the first-boot unit, which
in turn runs the wizard, which on apply enables and starts the
gateway.

## Layout

```
roles/deputyos/
├── defaults/
│   └── main.yml              ← role-wide facts (no_voice_hw, cloudish_hw, ...)
├── meta/
│   └── main.yml              ← role metadata (Ansible compatibility)
├── handlers/
│   └── main.yml              ← Reload sysctl / ufw / Avahi / AppArmor / systemd
├── tasks/
│   ├── main.yml              ← entry point; dispatches to the rest
│   ├── security-baseline.yml
│   ├── networking-baseline.yml
│   ├── wizard-baseline.yml
│   ├── hooks-baseline.yml
│   ├── voice-baseline.yml    ← gated
│   ├── variant-rpi5.yml
│   ├── variant-rpi4.yml
│   ├── variant-arm64-generic.yml
│   ├── variant-qemu-aarch64.yml
│   ├── variant-qemu-x86_64.yml
│   ├── variant-x86_64-mini-pc.yml
│   ├── variant-wsl2.yml
│   ├── variant-macos-qemu.yml
│   ├── variant-digitalocean.yml
│   ├── variant-oracle-arm-free.yml
│   ├── variant-hetzner-cloud.yml
│   ├── variant-vultr.yml
│   ├── variant-linode.yml
│   ├── variant-fly-machines.yml
│   ├── profile-openclaw.yml
│   ├── profile-hermes.yml
│   └── profile-khoj.yml
├── templates/                ← *.j2 — systemd units + per-profile config
└── files/                    ← AppArmor profiles, stub binaries, limits.<hw>.json
```

The shared invocation pattern: every Packer template runs the role
with `deputyos_hw=<hw>` and `deputyos_profile=<id>`, both injected via
Packer `var` blocks. The shared playbook lives at
`packer/playbook.yml`.

## Entry point: `tasks/main.yml`

The `main.yml` is a flat `import_tasks` chain:

```yaml
- name: Assert required role variables are set
  ansible.builtin.assert:
    that:
      - deputyos_hw | length > 0
      - deputyos_profile | length > 0

- name: Apply security baseline
  ansible.builtin.import_tasks: security-baseline.yml

- name: Apply networking baseline
  ansible.builtin.import_tasks: networking-baseline.yml

- name: Apply wizard baseline
  ansible.builtin.import_tasks: wizard-baseline.yml

- name: Apply hooks baseline
  ansible.builtin.import_tasks: hooks-baseline.yml

- name: Apply voice baseline (gated)
  ansible.builtin.import_tasks: voice-baseline.yml

# 14 variant dispatches, when: deputyos_hw == "<hw>"
- name: Apply rpi5 variant tuning
  ansible.builtin.import_tasks: variant-rpi5.yml
  when: deputyos_hw == "rpi5"
# ... rpi4, arm64-generic, qemu-aarch64, qemu-x86_64,
#     x86_64-mini-pc, wsl2, macos-qemu,
#     digitalocean, oracle-arm-free, hetzner-cloud, vultr,
#     linode, fly-machines

# 3 profile dispatches, when: deputyos_profile == "<id>"
- name: Apply OpenClaw profile bake
  ansible.builtin.import_tasks: profile-openclaw.yml
  when: deputyos_profile == "openclaw"
# ... hermes, khoj
```

## Pre-build staging: `scripts/build.sh`

Before invoking Packer, `scripts/build.sh` populates
`build/staging/` with the host-built artefacts the role copies into
the guest. Pre-stage steps:

1. **`deputyctl` release binary** — `cargo build --release --bin
   deputyctl`; copied to `build/staging/deputyctl`.
2. **`deputywizard` release binary** — same; `build/staging/deputywizard`.
3. **Profile manifest** — `cp profiles/<id>.toml
   build/staging/profiles/<id>.toml`.
4. **`limits.json`** — per-target. Falls back to a synthesized
   inline default if neither `deputyctl/etc/limits.<hw>.json` nor
   `roles/deputyos/files/limits.<hw>.json` exists.
5. **Voice assets** — `whisper.cpp` model + Piper binary + Piper
   voice. Skipped if `DEPUTYOS_VOICE_OFFLINE=1` or the target is in
   the no-voice set. Failures are non-fatal — the role tolerates
   missing assets and refuses to start the unit.
6. **Cloud-init seed** — for QEMU-style targets that need an SSH
   session for Packer. A throwaway Ed25519 keypair is generated under
   `build/staging/ssh-key`.

Then Packer is invoked:

```sh
packer init <template>
packer validate -var "profile=<id>" -var "channel=<channel>" \
                -var "tier=<tier>"  -var "deputyos_staging_dir=<staging>" \
                <template>
packer build  ... <template>
```

## Per-Packer-template anatomy

Each template at `packer/<hw>.pkr.hcl` does the same shape with
target-specific blocks:

```hcl
packer {
  required_plugins { ... }
}

variable "profile"               { type = string }
variable "channel"               { type = string }
variable "tier"                  { type = string }
variable "deputyos_staging_dir"   { type = string }

source "<builder>" "<name>" {
  # ... source image, accelerator, output path
}

build {
  sources = ["source.<builder>.<name>"]

  provisioner "file" {
    source      = "${var.deputyos_staging_dir}/deputyctl"
    destination = "/tmp/deputyctl"
  }
  # ... more file copies for limits.json, profiles, voice assets

  provisioner "ansible" {
    playbook_file = "packer/playbook.yml"
    extra_arguments = [
      "-e", "deputyos_hw=<hw>",
      "-e", "deputyos_profile=${var.profile}",
      "-e", "deputyos_channel=${var.channel}",
      "-e", "deputyos_tier=${var.tier}",
    ]
  }

  post-processor "compress" { ... }   # img.xz / qcow2 stay as-is
  post-processor "shell-local" {
    inline = ["sha256sum build/<output>.<format> > build/<output>.<format>.sha256"]
  }
}
```

For QEMU targets the source is the Debian `nocloud` cloud image. For
Pi targets, packer-builder-arm chroot-mounts a Pi OS Lite image. For
cloud targets (`digitalocean`, `oracle-arm-free`), the builder is the
provider plugin; the output is a snapshot, not a local file.

## Hermetic-build invariants

deputyOS aims at SLSA L3 (M7). The hermetic invariants today:

1. **Pinned base SHAs.** Every Packer template pins
   `iso_url`/`source_image` by checksum. The Debian `nocloud` and
   Raspberry Pi OS Lite image SHAs are checked into the templates.
2. **Pinned package versions.** Critical packages
   (`linux-image-rpi-2711`, `clamav`, `apparmor`, …) lock to apt's
   version-pinning shape via the role's `defaults/main.yml`. The base
   distro is Debian 12 stable, which is itself pin-shaped.
3. **Pinned npm/pip versions.** Every profile bake recipe uses
   `pinned_version` from the manifest. The release-tracker bot bumps
   it; humans don't edit the venv install command.
4. **Pinned voice asset URLs + SHAs.** `scripts/build.sh` records
   them in `build/staging/voice/MANIFEST` and the role compares
   against the same record at install time.
5. **Determinism markers.** `SOURCE_DATE_EPOCH` is honored where
   Debian tooling supports it. Pi images use `pi-gen`'s deterministic
   shape.

What's not yet hermetic, by design:

- `freshclam` — ClamAV's signature DB pulls from a CDN at bake time.
  Best-effort; a fresh-DB-failed image is still functional (signatures
  update on first boot).
- `apt update`'s mirror selection. Debian's archive is by date, but
  individual mirrors can lag.
- Voice-asset CDN. Hugging Face + GitHub Releases serve the upstream
  whisper / Piper artefacts. SHA-pinning in MANIFEST gates the
  per-file integrity, but the URL itself is mutable.

M7 closes the remaining gaps with vendored mirrors for the unstable
upstreams.

## QEMU smoke harness

Smoke uses cloud-init seed files plus SSH-based assertions. The
shared scaffolding is `test/smoke/_common.sh`; per-target harnesses
(`test/smoke/qemu-aarch64.sh`, `qemu-x86_64.sh`, `rpi5.sh`) source
it.

`SMOKE_LEVEL` controls assertion depth:

- `scaffold` — boot succeeds, kernel up, ssh reachable.
- `m1` (default) — wizard service active, gateway service active,
  `deputyctl status` exits 0, healthz endpoint serves 200.
- `full` — adds M2+ assertions: cost ledger format, hooks dispatcher,
  apparmor enforce confirmation.

## End-to-end timeline of `make build TARGET=qemu-aarch64 PROFILE=khoj`

1. `make build TARGET=qemu-aarch64 PROFILE=khoj`
2. `scripts/build.sh` runs.
3. `cargo build --release --bin deputyctl` → `build/staging/deputyctl`.
4. `cargo build --release --bin deputywizard` → `build/staging/deputywizard`.
5. `cp profiles/khoj.toml build/staging/profiles/khoj.toml`.
6. limits.json fallback (qemu-aarch64 has a Lane-A sample at
   `deputyctl/etc/limits.qemu-aarch64.json`).
7. Voice assets: skipped if `DEPUTYOS_VOICE_OFFLINE=1` or target is
   in no-voice set; otherwise downloaded.
8. Throwaway SSH keypair generated.
9. `packer init packer/qemu-aarch64.pkr.hcl`.
10. `packer validate ...`.
11. `packer build ...` — boots the Debian cloud image under qemu,
    SSHs in, runs the role with `deputyos_hw=qemu-aarch64
    deputyos_profile=khoj`.
12. The role applies all 7 phases above. Khoj profile bake installs
    `khoj==1.32.0` from PyPI (or the stub on failure).
13. Packer powers off, post-processes the qcow2.
14. Output: `build/qemu-aarch64-khoj.qcow2` plus
    `build/qemu-aarch64-khoj.qcow2.sha256`.

Total time: ~6-9 minutes on a recent Intel laptop.

## Troubleshooting

!!! warning "`packer build` hangs at 'Waiting for SSH'"
    The cloud-init seed didn't land or the throwaway SSH key wasn't
    accepted. Check `build/staging/cloud-init/user-data` is well-formed
    YAML and the pubkey ended up in `users.[0].ssh_authorized_keys`.

!!! warning "Pi 4 build fails with 'no binfmt_misc handler'"
    `packer-builder-arm` needs `qemu-user-static` registered. On
    Debian: `sudo apt install qemu-user-static binfmt-support`.
    `make doctor` flags this.

!!! tip "Iterate without re-baking"
    For role-only changes, run the playbook directly against a
    booted dev VM: `ansible-playbook -i <ip>, packer/playbook.yml -e
    'deputyos_hw=qemu-aarch64 deputyos_profile=khoj'`. 30 seconds vs.
    9 minutes.

## Related

- [Build → Make targets](make-targets.md)
- [Distribution → Hardware matrix](../distribution/hardware-matrix.md)
- [How-to → Add a profile](../how-to/add-a-profile.md)
- [How-to → Add a hardware target](../how-to/add-a-hardware-target.md)
- [Reference → Schemas → release manifest](../reference/schemas/release-manifest.md)
