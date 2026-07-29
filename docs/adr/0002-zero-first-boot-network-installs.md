# ADR 0002 — Zero first-boot network installs

**Status:** Accepted (M0). Load-bearing for the entire architecture.

## Context

Both OpenClaw (Node) and Hermes Agent (Python) require a non-trivial install: `npm install`, `pip` / `uv sync`, native modules built from source (`@discordjs/opus` is the famous offender, with NEON intrinsics that fail on arm64), CMake of a specific minimum version, system packages from `apt`. The dominant failure mode for semi-technical users running these stacks on a Pi is *the install*. Search any of:

- `@discordjs/opus` NEON build error on Pi
- `npm install openclaw@latest` fails
- CMake too old on 32-bit Raspberry Pi OS
- Hermes gateway PATH is wrong because tools were installed afterwards
- `~/.zshrc` permission denied during installer

…and you find dozens of users stuck at boot-time package operations, each in their own way. Even when installs succeed, they're slow, fragile (mirror outages), and produce non-reproducible state.

## Decision

**No `apt`, no `npm`, no `pip`, no `cargo`, no `git clone` runs after flashing.** Every runtime, native module, agent binary, ClamAV signature DB, Magika model, and configuration template is already in the image. The build pipeline does the resolution; the device does configuration only.

The only network traffic on first boot is:

1. DHCP / DNS / NTP.
2. The user-configured model provider (validation + first message).
3. Optional Tailscale / Cloudflare Tunnel join, if the user opts in via the wizard.

A user with a working network and a model-provider key reaches their first chat round-trip without any package operation succeeding or failing.

## Consequences

- Image size grows. Both runtimes plus prebuilt modules plus agent code plus ClamAV DB plus Magika model is ~1.5 GB compressed (acceptable for `.img.xz` distribution; users tolerate this for the ergonomic gain).
- Updates can't be in-place package upgrades — they must be image swaps. See [ADR-0007](0007-ab-image-swap-not-package-upgrade.md).
- The build pipeline becomes the load-bearing thing. Hermetic builds, lockfiles, prebuilt native module caches, and a hard QEMU smoke gate are non-negotiable.
- Adding a new profile is more work — we have to teach the bake recipe to populate the offline cache. But this work happens once, in CI, and removes the entire class of failure for every user thereafter.

## Alternatives considered

- **Light installs at first boot, with retries.** Rejected: any retry tree still leaves the user stuck on the bad case. Users who hit a transient mirror outage see a generic "install failed" and quit.
- **Docker images that download at first boot.** Rejected: same problem, plus Docker daemon overhead on a Pi, plus the pain of ensuring the Docker daemon's storage driver works right on every supported board. See [ADR-0004](0004-systemd-not-docker-on-device.md).
- **A "bootstrap then install" two-phase image** where an initial script runs the install on first boot. Rejected: the bootstrap script *is* the failing install — moving it doesn't fix it, only relocates the failure.
