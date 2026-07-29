# ADR 0004 — systemd, not Docker, on the device

**Status:** Accepted (M0)

## Context

A common pattern for "deploy this thing on a server" is to ship a Docker image and tell the user to `docker run` it. Both OpenClaw and Hermes Agent provide working Docker images. We considered baking the Docker daemon into deputyOS and running each profile as a container.

## Decision

**No Docker on the device.** Profiles run as systemd services as the `agent` user, with AppArmor profiles enforcing filesystem and capability limits.

We *use* OCI artefacts in one specific place — the Fly.io target packages the appliance as an OCI image because Fly's Machines API consumes OCI. That's a packaging format choice, not a runtime choice; the device itself doesn't run Docker in any target.

## Why

1. **The data-volume bug**. The single most common Hermes self-hosting failure is "I forgot to mount the data volume; my agent's memory is gone after `docker rm`." systemd binds the data dir directly via the unit; there's nothing to forget.
2. **PATH and exec model**. Hermes' gateway captures shell PATH at install time. Inside Docker that PATH is the container's PATH, set once at image build. With systemd, the unit's `Environment=` and `EnvironmentFile=` are the source of truth — no surprises from a shell that wasn't sourced.
3. **AppArmor and capability confinement**. We get tighter and more predictable confinement applying AppArmor profiles to systemd-managed processes than layering Docker's seccomp on top of an already complex containerd graph.
4. **Resource overhead**. The Docker daemon plus a containerd plus runc adds RAM and disk overhead that's noticeable on a Pi 4 4GB. systemd is already running and free.
5. **Update model**. We swap entire image partitions (see [ADR-0007](0007-ab-image-swap-not-package-upgrade.md)). Containers add a layer of indirection that's irrelevant when the whole rootfs gets replaced.
6. **Storage driver flakiness**. Docker's overlayfs/btrfs/zfs storage drivers each have known issues on different SBC distros. We sidestep this entirely.

## Alternatives considered

- **Docker on device.** Rejected for the reasons above. Also: the user-facing model "I just installed something" gets confused if there's a daemon they have to think about (when does it start? who manages it?).
- **Podman on device.** Rejected for similar reasons. Lower overhead than Docker but the data-volume + PATH problems remain because they're container-shape problems, not Docker-specific.
- **systemd-nspawn.** Considered as a middle path. Rejected for now: we'd inherit some of the confinement benefits without the Docker tax, but at cost of extra build complexity (we'd need to bundle a separate rootfs per profile). If we ever need stronger isolation between profiles than AppArmor gives us, nspawn is the natural next step.

## Consequences

- Adding a profile means writing a real systemd unit template, not adopting the upstream Docker image as-is. We've spec'd this in [02-profiles.md](../02-profiles.md).
- Inter-profile isolation rests on AppArmor, not container namespaces. The profiles in `apparmor/` are the load-bearing isolation control.
- Users used to `docker logs` should reach for `deputyctl logs` (which wraps `journalctl`). The PWA dashboard and `deputyctl logs` give them the same experience.
