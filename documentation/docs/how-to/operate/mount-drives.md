# Mount drives so the agent can read/write your files

deputyOS gives the agent access to user files via three surfaces — all gated
through a single policy file at `/etc/deputyos/mounts-policy.json`. Every
mount lives under `/mnt/deputyos/` so AppArmor's per-profile rules can confine
it. Every mount is reviewable + revocable from `deputyctl mounts` and from
the PWA "Mounts" card.

## Surface 1: host-FS passthrough (desktop launcher / WSL2)

When deputyOS runs as a VM on your laptop (via the desktop launcher → WSL2 / UTM / qemu),
share a host folder into the appliance.

### Linux / macOS host (qemu / UTM, virtiofs)

```bash
make try TARGET=qemu-aarch64 VIRTIOFS_SHARE=/home/me/Documents
deputyctl mounts add \
  --id documents \
  --host-path /home/me/Documents \
  --guest-path /mnt/deputyos/documents \
  --mode rw
```

### Windows host (WSL2, DrvFs)

The desktop launcher detects `/mnt/c` and `/mnt/wslg` automatically and
offers a per-folder picker. Behind the scenes it generates the same
`deputyctl mounts add` invocation.

## Surface 2: removable drives on bare-metal

Plug a USB stick or external SSD into a Pi 5 / NUC / mini-PC. With the
default policy, nothing happens — auto-mount is opt-in:

```bash
sudo jq '.removable.enabled = true | .removable.auto_mount = true' \
  /etc/deputyos/mounts-policy.json | sudo tee /etc/deputyos/mounts-policy.json
```

Now plugging a USB stick auto-creates `/mnt/deputyos/<label-or-uuid>`. The
helper enforces:

- `nosuid,nodev,noexec` for unknown filesystems.
- `ro` by default; flip to `rw` per-device in the policy.
- LUKS-encrypted volumes are refused until you `cryptsetup luksOpen` them
  manually.

`deputyctl limits` adds a "Connected drives" section listing
detected/mounted/refused devices with reasons.

## Surface 3: SMB / NFS network shares

For NAS users (TrueNAS, Synology, Unraid). Network shares use a separate
`network-add` subcommand (and a separate form in the wizard) because they
carry a `kind` (`cifs` | `nfs`) and reference credentials by name rather
than by value.

```bash
# 1. Put the SMB credentials in secrets.env (mode 0600) — never in the policy.
sudo tee -a /etc/deputyos/secrets.env >/dev/null <<'EOF'
NAS_PHOTOS_CREDS=username:password
EOF
sudo chmod 600 /etc/deputyos/secrets.env

# 2. Add the share, naming the credentials key.
deputyctl mounts network-add \
  --id nas-photos \
  --kind cifs \
  --source "//nas.lan/photos" \
  --guest-path /mnt/deputyos/nas-photos \
  --mode ro \
  --creds-env NAS_PHOTOS_CREDS

# NFS usually needs no credentials — omit --creds-env:
deputyctl mounts network-add \
  --id nas-videos \
  --kind nfs \
  --source nas.lan:/srv/videos \
  --guest-path /mnt/deputyos/nas-videos \
  --mode ro

# 3. Apply.
deputyctl mounts apply
```

You can do the same from the wizard: the **Drives** step links to the
standalone `/mounts` page, whose "Add a network share (SMB / NFS)" form
posts the same fields to `/mounts/network-add`. The form collects the
credentials *key name* only — you still add the secret itself to
`secrets.env` by hand, so it never passes through the wizard or the
policy file.

`deputyctl doctor mounts-health` pings each share and reports failures (DNS,
auth, kernel-module-missing).

## When mounts actually appear

`deputyos-mounts.service` is **enabled at boot** and runs the materialiser
(`deputyos-mount-materialise.sh`) before the first-boot wizard, so any
mounts already in the policy are present from power-on. After you edit the
policy — via `deputyctl mounts add` / `network-add` / `remove`, the wizard
`/mounts` page, or the PWA — re-materialise with:

```bash
deputyctl mounts apply          # restarts deputyos-mounts.service
```

Removable USB/SD media are handled out-of-band by the udev rule
(`99-deputyos-removable.rules` → `deputyos-mount-removable.sh`) on insert,
not by the boot unit.

## Revoking access

```bash
deputyctl mounts list                  # see what's configured
deputyctl mounts remove documents      # by id
deputyctl mounts apply                 # re-materialise
```

The PWA "Mounts" card (`/app/mounts`) and the wizard `/mounts` page both
have a one-click revoke button per mount. The PWA rewrites the policy file
in-place; run `deputyctl mounts apply` (or reboot) to un-mount it.

## Why this is safe

- Every mount is under `/mnt/deputyos/`, which AppArmor per-profile rules
  confine.
- Mounts are explicit: an empty policy means the agent sees nothing.
- Credentials live in `secrets.env`, never in the policy file (so the
  policy is safe to commit to backups).
- Removable auto-mount is opt-in and applies safe options for unknown
  filesystems.
