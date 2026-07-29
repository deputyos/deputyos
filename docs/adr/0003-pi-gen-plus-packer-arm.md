# ADR 0003 — pi-gen + packer-builder-arm for image builds

**Status:** Accepted (M0)

## Context

We need to build appliance images for many hardware targets (Pi 5, Pi 4, generic arm64 SBCs, x86 mini-PCs, WSL2, qcow2, DigitalOcean snapshots, Oracle Cloud, Fly.io). Each target wants different bootloader/kernel/firmware bits but identical agent/security/baseline contents.

## Decision

- **Raspberry Pi targets:** [`pi-gen`](https://github.com/RPi-Distro/pi-gen) (the official tool) provides the base image with the correct firmware and kernel package set per board. We use the `arm64` branch.
- **Overlay across targets:** [`mkaczanowski/packer-builder-arm`](https://github.com/mkaczanowski/packer-builder-arm) plugin for HashiCorp Packer overlays our shared Ansible role on top of the base. Builds run in a privileged container with `qemu-user-static` providing arm64 emulation on x86 builders.
- **DigitalOcean and OCI:** Packer's first-party DigitalOcean builder and standard OCI builders consume the same Ansible role.
- **One Ansible role** (`roles/deputyos/`) is the single source of truth. Hardware variation is gated by `when: hw == "..."` conditionals on tasks, not by separate roles.

## Why this combination

- **pi-gen** is what Raspberry Pi Foundation uses to build Raspberry Pi OS itself. Choosing it inherits firmware-update timeliness, `tryboot` compatibility on Pi 5, and the maintainers' ongoing kernel work. Anything else means re-implementing what they already do well.
- **packer-builder-arm** is the most mature open-source Packer plugin for arm64 image builds. It's used in production by many SBC distros. Its image-creation model (start from an existing base, apply provisioners, output `.img`) maps cleanly to our needs.
- The combination keeps **one provisioning role** across Pi (`packer-builder-arm`), x86 (`packer-builder-arm` with the x86 base, or Packer QEMU), and cloud (Packer DO / OCI). That one role is where all the engineering value accumulates.

## Alternatives considered

- **Yocto / Buildroot.** Rejected: more flexible than we need and a steeper learning curve. We'd be building our own distro instead of riding Debian/Pi OS upstream. Worth revisiting if we ever ship truly minimal images, but the agent stacks are big enough that the extra surface from a stock Debian doesn't matter.
- **Ubuntu Core / snaps.** Rejected: locks us into Snap and the Canonical channel model. We want flexibility on what runtimes we bake.
- **Custom shell-script image build.** Rejected: that's what `pi-gen` already is, but ours wouldn't have the maintainer base behind it.
- **Docker images on host OS (we install our own minimal OS, then run agents in containers).** Rejected: see [ADR-0004](0004-systemd-not-docker-on-device.md). Containers also break our offline-package guarantees because container layers tend to imply network pulls at boot if not careful.

## Consequences

- We're one community PR away if `packer-builder-arm` ever stagnates. Mitigation: pin a known-good fork in our build CI.
- pi-gen is Debian-specific. Our `arm64-generic` SBC target rides Debian 12 arm64; users on RHEL-derived SBC distros are not first-class.
- The shared Ansible role is the load-bearing artefact. ADRs 0001 (one binary, many profiles), 0007 (A/B updates), and 0008 (security baseline) all assume this role exists and is the only place provisioning happens.
