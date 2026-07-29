# 08 — Update and rollback

Updates ship as full image swaps, never as in-place package upgrades. This is the only way the "zero first-boot network installs" invariant survives the lifetime of the device. This doc describes the partition layout, the swap mechanic, and the watchdog that keeps a bad update from bricking the box.

## Partition layout

```
┌──────────────────────────────────────────────────────────────────┐
│  /boot/firmware  (FAT, small)                                     │
│   - bootloader config (tryboot.txt / U-Boot env / systemd-boot)  │
│   - deputyos.yaml (cloud-init userdata for first boot)            │
│   - kernel + initrd for the active slot                          │
├──────────────────────────────────────────────────────────────────┤
│  slotA  (ext4, read-only at runtime, ~3 GB)                      │
│   - /opt/deputyos/profiles/<id>/  (pinned agent versions)         │
│   - /var/cache/deputyos/          (npm + pip + skill caches)      │
│   - /var/lib/clamav/             (packed signature DB)           │
│   - /opt/deputyos/magika/                                         │
│   - /usr/local/bin/deputyctl                                      │
│   - all OS files for slotA's image rev                           │
├──────────────────────────────────────────────────────────────────┤
│  slotB  (ext4, ~3 GB)                                            │
│   - same shape; either current+1 or empty until first update     │
├──────────────────────────────────────────────────────────────────┤
│  data   (ext4, remainder of disk)                                │
│   - /home/agent/.openclaw/   ⎫                                   │
│   - /home/agent/.hermes/     ⎬ never touched by image swap       │
│   - /etc/deputyos/secrets.env ⎪                                   │
│   - /etc/deputyos/backup.env  ⎭                                   │
│   - swapfile (2 GB)                                              │
└──────────────────────────────────────────────────────────────────┘
```

Slot partitions mount read-only at runtime, with `overlayfs` over `tmpfs` for the few paths that need to be writable during a session (e.g. `/var/run`, `/tmp`). This means the running system can never accidentally mutate a slot — useful both as a hardening measure and as a guarantee that A/B comparison works.

## The swap mechanism per platform

| Platform | Mechanism |
|---|---|
| **Pi 5** | Pi firmware `tryboot`. We update the inactive slot's kernel + image, set `tryboot.txt` to point at the new slot for one boot, reboot. The new slot must call `deputyctl confirm` (which the watchdog issues on green health) within 5 minutes or the firmware reverts to the previous slot on next boot. |
| **Pi 4** | U-Boot with `bootcount`/`bootlimit`. Same idea: write new image, set `bootenv` to one-shot the new slot, reset `bootcount`. If `bootcount` exceeds `bootlimit` without the watchdog clearing it, U-Boot falls back. |
| **arm64-generic** | U-Boot with `bootcount`. Same. |
| **x86 mini-PC** | systemd-boot with [boot-counting](https://systemd.io/AUTOMATIC_BOOT_ASSESSMENT/). Same. |
| **DigitalOcean / cloud VPS** | DO snapshots aren't directly A/B'd, so we apply the new image as an in-place root-fs swap (still read-only afterwards) and lean on cloud snapshot rollback as the fallback. `deputyctl restore-image --snapshot <id>` calls the DO API. |
| **WSL2 / qemu / OCI / UTM** | The desktop downloads and verifies the new immutable base image, then replaces it through the platform driver. Full host-distribution images are never written from inside the guest. |

In every case the data partition mounts unchanged across a swap. User state never lives on a slot.

## The flow

```
deputyctl update --apply
  │
  ▼
download artefact + verify sha256 + minisign + cosign
  │
  ▼
decompress into the inactive slot (using fallocate + dd)
  │
  ▼
stage bootloader pointer to one-shot the new slot
  │
  ▼
backup --before-update (snapshot to user bucket)
  │
  ▼
reboot
  │
  ▼ (new slot boots)
systemd target multi-user.target
  │
  ▼
deputyctl-watchdog.service starts
  │
  ▼ within 5 min:
  ├── deputyctl doctor   — passes?
  │     yes → deputyctl confirm → bootloader pointer made permanent
  │     no  → reboot (one-shot pointer expires → falls back to previous slot)
  ▼
deputyctl logs records the outcome; manifest event uploaded if telemetry is opted-in
```

Every image runs `deputyos-update-cycle.timer` daily. Immutable VM/container
targets use `check` mode and are upgraded by the desktop application. A native
A/B image may opt into `apply` mode only after its `slots.json` declares
explicit slot A/B destinations and the release supplies a signed
`raw`, `img`, or `rootfs` payload. Formats such as qcow2 are rejected by the
guest updater.

After a new slot boots, `deputyos-watchdog-confirm.service` confirms it only
when the active profile health endpoint responds. Until then,
`update_pending` remains set and boot attempts are counted. The previous slot
is selected automatically at the rollback threshold.

Independently, `deputyos-reconcile.timer` checks the resident agent, active
profile, terminal, and provisioned tunnel every two minutes. It waits for two
consecutive failures before performing an allow-listed service start/restart.
Operators can inspect or trigger this path with:

```sh
deputyctl repair
deputyctl repair --json
deputyctl repair --force
```

## What the watchdog checks

Inside `deputyctl confirm` the criteria are intentionally narrow:

1. systemd's `default.target` reached cleanly.
2. `deputyctl doctor` exits zero (this checks the security baseline, not just "is the agent up").
3. The active profile's healthcheck URL returns 200.
4. ClamAV daemon is running.
5. `ufw` is active with the expected rule set.

If any check fails within the 5-minute grace window, the device reboots; the bootloader's "one-shot new slot" expires, and the previous slot becomes active. The user observes a single ~90-second downtime and lands back where they were.

## Manual rollback

```sh
deputyctl rollback           # confirm prompt; reboots into the other slot
deputyctl rollback --yes     # immediate
```

Rollback is symmetric — both slots are valid runtime targets. The data partition is unchanged, so memory and conversations are preserved.

For DigitalOcean and other clouds where slot swap is in-place, `deputyctl restore-image --snapshot <id>` uses the cloud's snapshot API rather than a local A/B.

## Recovery cases

| Failure | What happens | What the user does |
|---|---|---|
| New image fails to boot at all | Bootloader's one-shot pointer expires; falls back automatically | Nothing |
| New image boots but `doctor` fails | Watchdog reboots; falls back automatically | Read `deputyctl logs --since "1h ago"` |
| Updated agent has a regression | Manual `deputyctl rollback` | Run rollback |
| Both slots somehow broken | Re-flash from picker page; data partition survives | Flash and the wizard offers to restore from latest backup |
| Disk full on data partition | Watchdog flags via doctor; agent enters read-only mode | `deputyctl prune` (rotates old logs and backups) |

## ClamAV signatures and out-of-band fixes

ClamAV signatures travel inside the slot image — nothing fetches signatures at runtime. Out-of-date signatures are addressed by cutting a new image rev. For urgent CVEs that don't bump the agent version, we publish a "patch only" image rev that bumps the OS package set and ClamAV DB only; manifest schema accommodates a profile staying on the same `agent_version` across two `deputyos_version`s.

## Telemetry on update outcomes

Off by default. If the user opts in (via the wizard or `deputyctl telemetry enable`), the watchdog reports a small JSON event to the project's telemetry endpoint:

```json
{
  "ts": "2026-04-27T03:12:00Z",
  "device_id": "<hashed install token>",
  "from_version": "2026.04.20",
  "to_version": "2026.04.27",
  "result": "confirmed | rolled-back | timeout",
  "doctor": "pass | fail:<which-check>"
}
```

No device IP, no provider keys, no agent state. Used to catch update regressions early.
