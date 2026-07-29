# 06 — Storage and backup

deputyOS follows an object-storage-first model. PostgreSQL is reserved for
transactional account concerns: users, credential hashes, subscriptions,
payments and the entitlements derived from them. Backup payloads, catalogs,
verification records and lifecycle history never belong in PostgreSQL.

## Backup products

| Product | Availability | Storage owner | Encryption |
|---|---|---|---|
| Self-managed | All editions | User B2, R2 or S3 bucket | User-controlled bucket; encrypted bundle migration is in progress |
| Managed | Business and Enterprise | deputyOS object storage | Mandatory client-side age encryption |

Downgrading an account stops new managed uploads but does not prevent listing,
downloading or deleting existing snapshots.

## Managed object layout

The managed API treats object storage as the source of truth:

```text
backups/account_id=<account>/
├── catalog/<snapshot>.json.zst
├── device_id=<device>/profile=<profile>/snapshots/<snapshot>/
│   └── bundle-<sha256>.age
└── events/
    ├── verified/<timestamp>-<uuid>.json.zst
    └── deleted/<timestamp>-<uuid>.json.zst
```

The content-addressed encrypted bundle is written first. Its catalog is written
last and is the commit point. Readers discover snapshots by listing catalogs.
Retention removes the catalog and bundle, then appends a lifecycle event.
Orphaned payloads can therefore be garbage-collected after a grace period
without requiring a database transaction.

Legacy `backups/<account>/<snapshot>.age` objects remain readable during
migration. New uploads never use that layout or write `backup_snapshots` rows.

## Consistency and contents

Before copying data, `deputyctl` asks the resident agent to:

1. freeze the active workload slice;
2. issue a filesystem `sync`;
3. report the deputy as quiesced.

The workload is thawed immediately after the snapshot inputs have been copied,
including on failure. Encryption and upload happen after the workload resumes.

A schema-v3 bundle contains:

- `BUNDLE.json` with snapshot, device, profile, key and consistency metadata;
- the active profile data directory;
- user hooks;
- `secrets.env`;
- the profile session database when it is outside the data directory.

The reproducible OS slot, caches, logs and journal are not included.

## Recovery key

Encryption never derives from the backup API token. Tokens are revocable
credentials; using one as a key would make token rotation destructive.

The wizard creates `/etc/deputyos/backup-recovery-key` with mode `0600`.
The user must export it and store it offline:

```sh
deputyctl backup recovery-key export > deputyos-recovery-key.txt
```

On a replacement deputy:

```sh
deputyctl backup recovery-key import deputyos-recovery-key.txt
deputyctl restore from-cloud --snapshot <snapshot> --yes
```

The CLI retains a token-derived decryption fallback only for old schema-v2
bundles.

## Verification

Before upload, the client decrypts the newly encrypted object to a temporary
archive and asks `tar` to read its full table of contents. Only then is it
uploaded. After a successful upload it records a verification event through
the managed API. The local plaintext and verification archive are removed.

The server never receives the recovery key or plaintext.

## Scheduling

The wizard installs a persistent systemd timer:

- managed: `deputyctl backup schedule --at 03:00 --to-cloud`;
- self-managed: `deputyctl backup schedule --at 03:00`.

Manual configuration can use `--every 6h`, `--every 30m`, or another daily
`--at HH:MM`. The resident command surface exposes `backup.status` and
`backup.run`; the desktop console uses the same allow-listed operation.

Managed retention is enforced from object catalogs:

| Plan | Snapshot count | Total bytes | Age |
|---|---:|---:|---:|
| Business | 100 | 100 GiB | 365 days |
| Enterprise | 500 | 1 TiB | 2555 days |

These are entitlement defaults in the API and can be changed centrally.

## Self-managed buckets

The wizard supports Backblaze B2, Cloudflare R2 and custom S3-compatible
storage. It writes:

- `/etc/deputyos/rclone.conf` — scoped bucket credentials, mode `0600`;
- `/etc/deputyos/backup.toml` — `remote:bucket` and retention configuration;
- `/etc/deputyos/backup.env` — compatibility values, mode `0600`.

Self-managed data remains in the user's object store and never traverses the
deputyOS managed API.

## Restore safety

Restore downloads and decrypts into staging first. It then stops the workload,
moves each current component aside, places the restored component and restarts
the workload. If any placement fails, already placed components are rolled
back. The moved-aside copies are retained for manual recovery.

Managed catalogs are account-scoped rather than device-scoped, so a registered
replacement deputy can list and download snapshots made by another device. It
still needs the matching exported recovery key.
