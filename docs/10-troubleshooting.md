# 10 — Troubleshooting

deputyOS is designed so the documented failure modes from running OpenClaw and Hermes manually on a Pi don't recur. This doc enumerates them, says how each is prevented in the image, and gives the recovery command for the rare case the prevention slips.

`deputyctl doctor` is the first thing to run for any "something seems off" report. It checks every item in this table.

## Pain-point map

| Pain point (observed in the wild) | How deputyOS prevents it | If it does happen |
|---|---|---|
| `@discordjs/opus` NEON build fails on arm64 (issue #23909) | Prebuilt arm64 native module baked into the image; npm cache populated at build time. | `deputyctl doctor` reports the module path and SHA. Reflash if mismatched. |
| `npm install openclaw@latest` fails (issue #23861) | OpenClaw is already installed in the slot partition. npm not used at boot. | Should be impossible. If reported, file a bug; we shipped a broken image. |
| CMake too old | CMake 3.28+ baked in. arm64-only target avoids the 32-bit-OS problem. | `cmake --version` >= 3.28 is a doctor check. |
| OOM on small-RAM devices | zram + 2 GB swapfile baked in. Wizard warns if RAM<8 GB and offers to disable RAM-heavy channels. | `deputyctl resources` shows RAM/zram/swap; consider switching to a lighter channel set or upgrading hardware. |
| WiFi drops every few minutes | `iwconfig wlan0 power off` enforced via systemd oneshot at boot. | `deputyctl doctor` checks. To force: `sudo iwconfig wlan0 power off`. |
| `systemd --user` lingering disabled (service won't start headless) | `loginctl enable-linger agent` baked in. | `loginctl show-user agent` should show `Linger=yes`. |
| Hermes gateway captures stale PATH because tools were installed after gateway | All tools and the gateway are installed at build time, in the right order. PATH captured then is correct forever. | If somehow stale, `systemctl restart hermes-gateway.service` re-reads. |
| `~/.zshrc` permission errors | `agent` user owns its rc files with mode 0644 from build time. | `ls -la ~agent/.zshrc` should show `agent agent`. |
| Docker data volume forgotten | We don't use Docker on device. systemd unit binds data dir directly. | N/A |
| Wrong API key format silently accepted | Wizard validates with a real round-trip before writing `secrets.env`. | `deputyctl model test` repeats the validation. |
| Gateway exposed to the world | Default allowlist mode = `allowlist`. ufw blocks until configured. "open" mode requires `--i-know-what-im-doing`. | `deputyctl gateway status` shows current mode. |
| Pi 5 SSH/WiFi setup confusion | `cloud-init` userdata read from `/boot/firmware/deputyos.yaml` on the FAT partition — visible to the user before flashing. | Edit `deputyos.yaml`, reboot, wizard re-reads. |
| SD card slowness | Image detects SD-only boot and prints a single nudge to flash to USB SSD. Doctor reports `storage_class=sd`. | `deputyctl resources --io` shows iostat-like numbers. |
| Mirror outage at install time | Not applicable — no installs occur on device. | N/A |

## Recipes by symptom

### "I can't reach `deputyos.local`"

Most likely your network blocks mDNS. Look at the HDMI screen — the device prints its IP on first boot. If headless, check your router's DHCP leases for a host named `deputyos-<your-hostname>`. Direct IP works the same as `deputyos.local`.

```sh
# from another machine on the LAN
nmap -sn 192.168.1.0/24 | grep -A1 deputyos
```

If you find the IP but `:8088` doesn't answer, SSH in and run:

```sh
deputyctl status
sudo journalctl -u deputyctl-wizard.service -n 200
```

### "The wizard refuses to enable a channel"

It's enforcing the rule that channels can't come up while the security baseline is broken. Run:

```sh
deputyctl doctor --verbose
```

It will print which check failed and the one-line fix.

### "The agent isn't replying to my Telegram message"

```sh
deputyctl logs --follow
```

Common causes:

- API key revoked or out of credit. `deputyctl model test` confirms.
- Telegram bot token wrong. Reconfigure with `deputyctl init --reconfigure-channels`.
- User ID not in the allowlist. `deputyctl gateway allowlist add <user-id>`.

### "Update applied, now the agent won't start"

The watchdog should have rolled you back automatically. If you're still on the failing slot:

```sh
deputyctl rollback --yes
```

This is non-destructive and the data partition is preserved.

### "I want to start completely fresh"

```sh
deputyctl factory-reset
```

Wipes the data partition, keeps the slot images. Wizard re-runs on next boot. Take a backup first if you might want anything back:

```sh
deputyctl backup now --name pre-reset
```

### "Disk is full"

```sh
deputyctl prune              # rotates old logs and old backups
deputyctl resources --disk   # see what's eating space
```

If `~/.<profile>/sessions.sqlite` has grown enormous, the agent's been chatty. Hermes has built-in compaction; OpenClaw doesn't. Consider `deputyctl backup now` then a selective DB vacuum.

### "I think a file the agent received was malicious"

Check the quarantine:

```sh
sudo ls /var/quarantine/
sudo cat /var/log/clamav/clamav.log
sudo journalctl -u magika-prefilter.service -n 100
```

Magika logs catch content-type spoofing; ClamAV catches signature hits. Both report to `deputyctl logs`.

## When to ask for help

If `deputyctl doctor` is green but something's still off, run:

```sh
deputyctl support-bundle
```

This produces a redacted tarball (no secrets, no message bodies) you can attach to a GitHub issue. The bundle contains: doctor output, last 200 lines of journals for `deputyctl-*` and the active profile, partition layout, build manifest, ufw rules.

Issue tracker: `github.com/deputyos/deputyos/issues`. Security-sensitive reports go to `security@deputyos.com` per [09-security.md](09-security.md).
