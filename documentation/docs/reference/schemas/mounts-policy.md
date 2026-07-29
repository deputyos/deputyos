# Mounts policy (`mounts-policy.json`)

`/etc/deputyos/mounts-policy.json` is the **single source of truth** for which
host folders, removable drives, and network shares the agent can see. It is
read and mutated by `deputyctl mounts`, materialised at boot by
`deputyos-mounts.service`, and surfaced in the wizard Drives step and the PWA
`/app/mounts` page.

The on-disk JSON Schema is at
[`docs/schemas/mounts-policy-v1.json`](https://github.com/deputyos/deputyos/blob/main/docs/schemas/mounts-policy-v1.json)
(also served at `https://www.deputyos.com/schemas/mounts-policy-v1.json`,
the `$schema` URL written into every policy file). The Rust struct is in
[`deputyctl/src/mounts.rs`](https://github.com/deputyos/deputyos/blob/main/deputyctl/src/mounts.rs).

!!! warning "Credentials never live here"
    SMB/CIFS credentials are **not** stored in this file. Put
    `<KEY>=username:password` (or separate `username` / `password` lines) in
    `/etc/deputyos/secrets.env` (mode 0600) and reference the key by name via
    `credentials_env`. The policy file only ever sees the key *name*.

[TOC]

## Resolution order

`deputyctl/src/paths.rs::mounts_policy_file()`:

1. `$DEPUTYOS_MOUNTS_POLICY` env var if set (dev / hermetic tests).
2. `/etc/deputyos/mounts-policy.json`.

Unlike `limits.json` there is **no dev fallback** — absence is meaningful
(an empty allow-list is the default policy). The file is created on first
`deputyctl mounts add` / `network-add`.

## Top-level shape

| Field | Type | Required | Default | Description |
|---|---|---|---|---|
| `$schema` | string | no | `https://www.deputyos.com/schemas/mounts-policy-v1.json` | JSON Schema URL. Written so editors + future consumers can validate. |
| `version` | integer | yes | `1` | Policy schema version. Currently `1`; mirrors `release::Manifest::mounts_policy_schema_version`. |
| `host_fs` | array | no | `[]` | Host-filesystem bind mounts. See below. |
| `removable` | object | no | see below | Removable-drive (USB/SD) policy. |
| `network` | array | no | `[]` | Network shares (SMB/CIFS, NFS). See below. |

Every `guest_path` across both arrays **must** live under `/mnt/deputyos/`
so AppArmor's per-profile rules can confine it.

## `host_fs[]` entries

| Field | Type | Required | Description |
|---|---|---|---|
| `id` | string (`[a-zA-Z0-9_-]+`) | yes | Stable id (`documents`, `code`). Unique across `host_fs` + `network`. |
| `host_path` | string | yes | Path on the host (informational; the materialiser bind-mounts it). |
| `guest_path` | string (`/mnt/deputyos/.*`) | yes | Path inside the appliance. |
| `mode` | `ro` \| `rw` | yes | Read-only or read-write. |

## `removable` object

| Field | Type | Default | Description |
|---|---|---|---|
| `enabled` | bool | `false` | Whether the udev rule reacts to removable-media events at all. |
| `auto_mount` | bool | `false` | Mount on insert (`true`) vs. wait for an explicit action (`false`). |
| `default_mode` | `ro` \| `rw` | `ro` | Mount mode for removable media. |
| `mount_options_unknown_fs` | string | `nosuid,nodev,noexec` | Options forced for filesystems the appliance does not recognise. |

Removable handling lives in the udev rule `99-deputyos-removable.rules` +
`deputyos-mount-removable.sh`, not the boot materialiser. `crypto_LUKS` and
empty `FSTYPE` devices are **refused outright**; unknown filesystems are
mounted with the forced hardening options above.

## `network[]` entries

| Field | Type | Required | Description |
|---|---|---|---|
| `id` | string (`[a-zA-Z0-9_-]+`) | yes | Stable id (`nas-photos`). Unique across `host_fs` + `network`. |
| `kind` | `cifs` \| `nfs` | yes | SMB/CIFS or NFS. |
| `source` | string | yes | Server path. `//nas.lan/photos` for cifs, `nas.lan:/srv/photos` for nfs. |
| `guest_path` | string (`/mnt/deputyos/.*`) | yes | Path inside the appliance. |
| `mode` | `ro` \| `rw` | yes | Read-only or read-write. |
| `credentials_env` | string \| null | no | Name of the env var in `/etc/deputyos/secrets.env` holding CIFS credentials. NFS usually omits this. |

## Example

```json
{
  "$schema": "https://www.deputyos.com/schemas/mounts-policy-v1.json",
  "version": 1,
  "host_fs": [
    {
      "id": "documents",
      "host_path": "/home/operator/Documents",
      "guest_path": "/mnt/deputyos/documents",
      "mode": "ro"
    }
  ],
  "removable": {
    "enabled": true,
    "auto_mount": true,
    "default_mode": "ro",
    "mount_options_unknown_fs": "nosuid,nodev,noexec"
  },
  "network": [
    {
      "id": "nas-photos",
      "kind": "cifs",
      "source": "//nas.lan/photos",
      "guest_path": "/mnt/deputyos/nas-photos",
      "mode": "ro",
      "credentials_env": "NAS_PHOTOS_CREDS"
    }
  ]
}
```

## See also

- [How-to: mount drives](../../how-to/operate/mount-drives.md) — operator walkthrough (wizard + CLI).
- [Systemd units](../system/systemd-units.md) — `deputyos-mounts.service`.
- [Filesystem layout](../system/filesystem-layout.md) — `/etc/deputyos/mounts-policy.json` + `/mnt/deputyos/`.