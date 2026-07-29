# deputyOS on WSL2

Two paths:

1. **Pre-built tarball** (M4+, when the CDN is live) — Windows users
   run [`Install-DeputyOS.ps1`](Install-DeputyOS.ps1) from an elevated
   PowerShell.
2. **Self-build** (works today) — a Linux or WSL2 host runs
   `make build TARGET=wsl2 PROFILE=<id>`, then the resulting
   `build/wsl2-<profile>.tar.gz` is copied to a Windows machine and
   imported with `Install-DeputyOS.ps1 -LocalTarball <path>`.

## Prerequisites (Windows)

- Windows 10 21H2+ or Windows 11.
- WSL2 enabled. From an admin PowerShell:
  ```powershell
  wsl --install
  wsl --set-default-version 2
  ```
  Reboot if prompted.
- ~3 GB free disk space at `$env:LOCALAPPDATA\deputyos`.
- PowerShell 5.1 (built-in) or PowerShell 7+.

## Quick install (pre-built — once M4 ships)

```powershell
# From this repo's wsl/ directory:
.\Install-DeputyOS.ps1
# or, with explicit profile + channel:
.\Install-DeputyOS.ps1 -Profile hermes -Channel stable
```

The script:

1. Confirms WSL2 is enabled and there's no existing `deputyos` distro.
2. Downloads `deputyos-<profile>-wsl2-<version>-<channel>.tar.gz` plus
   the matching `.sha256` from the deputyOS dist URL.
3. Verifies the SHA256 (refuses to import on mismatch).
4. Runs `wsl --import deputyos $env:LOCALAPPDATA\deputyos <tarball>
   --version 2`.
5. Prints next-step instructions.

After install:

```powershell
wsl -d deputyos
# inside the distro:
deputyctl init
```

The wizard listens on `http://localhost:8088` on the Windows host.

## Self-build (today)

On a Linux or WSL2 host with the deputyOS repo:

```bash
make doctor                       # confirm tooling
make build TARGET=wsl2 PROFILE=openclaw
ls build/wsl2-openclaw.tar.gz
```

Copy the tarball to the Windows machine (Drag-and-drop, scp, or
write directly into `\\wsl$\<distro>\home\you\` from File Explorer),
then:

```powershell
.\Install-DeputyOS.ps1 -LocalTarball C:\path\to\wsl2-openclaw.tar.gz
```

## What you can't do on WSL2

See [docs/14-limitations.md §wsl2](../docs/14-limitations.md#wsl2):

- **No audio.** WSL2 doesn't pass `/dev/snd`. Voice features are off.
- **mDNS doesn't reach the LAN.** `deputyos.local` resolves only from
  the Windows host. From a phone on the same Wi-Fi, use the host's
  Windows IP plus port 8088.
- **No A/B updates.** Updates are re-imports of the new tarball.
- **No persistent clamd.** On-demand `clamscan` + Magika hint runs
  daily.
- **Microsoft owns the kernel.** AppArmor cmdline flags, kernel-module
  tuning, etc. cannot be set at bake time.

If any of these are deal-breakers, run deputyOS on a Pi 5, x86 mini-PC,
or one of the cloud targets instead.

## Updates

```powershell
# stop the distro
wsl --shutdown

# back up your /etc/deputyos and home dir if you care:
wsl --export deputyos C:\backup\deputyos-pre-update.tar

# unregister the old distro
wsl --unregister deputyos

# re-run the installer with the new tarball
.\Install-DeputyOS.ps1
```

`deputyctl init` on the new distro restores from the previous backup if
one exists in your B2/R2 bucket.

## Troubleshooting

- **`wsl.exe not found`** — run `wsl --install` from an elevated
  PowerShell, reboot, retry.
- **`Default Version: 1` warning** — `wsl --set-default-version 2`.
- **`A WSL distro named deputyos already exists`** —
  `wsl --unregister deputyos` (or pick a different `-InstallDir`).
- **Slow first-boot** — first launch runs `deputyctl` migrations and
  warms ClamAV signatures; expect 2-3 minutes.

For Linux-side issues (build failures), see
[../docs/15-local-build.md](../docs/15-local-build.md).
