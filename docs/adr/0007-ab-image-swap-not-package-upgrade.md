# ADR 0007 — A/B image swap, not in-place package upgrade

**Status:** Accepted (M0)

## Context

Once the device is running, the agent and its dependencies will need updates: upstream releases, security patches, new ClamAV signatures, kernel CVEs. The traditional path on a Debian-flavoured device is `unattended-upgrades` plus `apt`, plus `npm install -g openclaw@latest`, plus `pip install --upgrade hermes-agent`. We considered that.

## Decision

**Updates ship as full image swaps using A/B partitions.** Never as in-place package upgrades. `unattended-upgrades` is disabled in our images; `apt`, `npm`, and `pip` are not invoked at runtime.

The flow is:

1. `deputyctl update --apply` downloads a new image artefact from the manifest.
2. Verifies signatures (`minisign` + `cosign`).
3. Writes the new image into the inactive A/B slot partition.
4. Sets the bootloader to one-shot the new slot.
5. Reboots.
6. Watchdog asserts `deputyctl doctor` is green within 5 minutes; if not, the bootloader rolls back automatically.

The data partition (containing `~/.<profile>/`, `secrets.env`, `backup.env`) is mounted unchanged across the swap.

## Why

This decision exists primarily to **preserve [ADR-0002](0002-zero-first-boot-network-installs.md)** — zero first-boot network installs. If updates were in-place package upgrades, every update would re-introduce the entire class of failure that motivated deputyOS in the first place: native modules that won't compile, mirror outages, version skew between npm/pip/apt, configuration drift.

Secondary benefits:

1. **Atomicity.** The image is either fully the new version or fully the old. No "half-upgraded" state where systemctl says everything's fine but a config file is from the old release.
2. **Reproducibility.** A device on `deputyos-2026.04.27` is byte-identical to every other device on `deputyos-2026.04.27` (modulo data partition). Bug reports become tractable.
3. **Rollback.** Going back is "boot the other slot". No state migration, no data loss, no need to backup-restore.
4. **Security.** A compromised package upstream doesn't propagate to the device until the next image rev. (We can pull a release if needed; in the apt model the user's already running the bad package.)
5. **CVE response without changing agent versions.** We can ship a "patch only" image rev that bumps the OS package set and ClamAV DB without touching `pinned_version` for any profile.

## Costs

- **Bandwidth.** Updates download the whole image (~1.5 GB compressed) instead of a few MB of package deltas. We mitigate with Cloudflare-fronted CDN (free egress) and resumable downloads.
- **Disk.** Two slot partitions plus the data partition. Sized for 8 GB SD as the floor; 16 GB+ recommended in docs.
- **Build cadence pressure.** Every CVE that would normally be a 2 KB `apt` update is now a full image rebuild. M4's release-tracker pipeline addresses this with automation; we accept the build minutes as the price.

## Alternatives considered

- **Hybrid: A/B for OS, in-place for agent code.** Rejected. The most fragile updates *are* the agent code (npm/pip native modules). Hybrid still has the failure surface we're trying to eliminate.
- **systemd-sysext / portable services.** Considered. They achieve some of the same atomicity benefits without partition swap. Rejected for now because the firmware-level rollback story on the Pi (`tryboot`) and on x86 (`systemd-boot` boot-counting) is cleaner end-to-end with full slot partitions. Worth revisiting if disk pressure becomes a real complaint.
- **OSTree / rpm-ostree.** Considered. Mature tech but adds a dependency we'd be the only consumers of in our user base; the ergonomics of teaching users `rpm-ostree status` aren't great. Slot-image is simpler.

## Consequences

- **`docs/08-update-rollback.md`** is the user-facing companion to this ADR.
- **The build pipeline must produce reproducible images** for SLSA L3 attestations to be meaningful. M7 commits to this.
- **The data partition's schema is load-bearing.** Anything that moves out of `~/.<profile>/` into a slot path becomes ephemeral across updates. We document this constraint clearly.
