# Back up and restore with deputyOS managed storage

Managed backup is available on Business and Enterprise. The device creates an
opaque age-encrypted bundle; the API and object store never receive plaintext
or the recovery key.

## Set up

Complete the wizard's Account step, then choose **deputyOS managed backup**.
The wizard:

- writes the revocable API credential to `/etc/deputyos/backup-token`;
- creates `/etc/deputyos/backup-recovery-key` with mode `0600`;
- schedules a persistent daily backup at 03:00.

Export the recovery key immediately:

```sh
sudo deputyctl backup recovery-key export > deputyos-recovery-key.txt
chmod 600 deputyos-recovery-key.txt
```

Keep this file in a password manager or offline recovery store. The API token
authorizes upload/download; it is deliberately not the encryption key.

`age`, `age-keygen` and `rclone` are baked into deputyOS images. Managed backup
fails closed if age or the recovery key is unavailable.

## Create a snapshot

```sh
sudo deputyctl backup now --to-cloud
```

The device:

1. asks `deputyd` to freeze the active workload and issue `sync`;
2. copies profile data, hooks, secrets and any external session database;
3. thaws the workload, including on copy failure;
4. writes a schema-v3 `BUNDLE.json`;
5. creates and age-encrypts the tar archive with the recovery secret;
6. decrypts it locally and reads the archive table to verify it;
7. uploads the ciphertext with profile, key-id and schema headers;
8. records verification and removes temporary files.

Preview prerequisites and the generated snapshot identity without changing
state:

```sh
sudo deputyctl backup now --to-cloud --dry-run
```

## Restore

On a replacement deputy, register it to the same account and import the saved
key:

```sh
sudo deputyctl backup recovery-key import deputyos-recovery-key.txt
sudo deputyctl restore from-cloud --snapshot <snapshot-id> --yes
```

Restore downloads the ciphertext, decrypts into staging, stops the workload,
moves current components aside and replaces them. A placement failure rolls
back components already changed. Old schema-v2 snapshots retain a
token-derived compatibility fallback.

## List or delete

The account dashboard is the normal surface. The API can also be called with a
device backup token:

```sh
BASE=${DEPUTYOS_API_BASE:-https://api.deputyos.com}
TOK=$(sudo cat /etc/deputyos/backup-token)

curl -fsSL -H "Authorization: Bearer $TOK" "$BASE/api/v1/backup/list"
curl -fsSL -X DELETE -H "Authorization: Bearer $TOK" \
  "$BASE/api/v1/backup/<snapshot-id>"
```

Catalogs, ciphertext, verification records and deletion events are all stored
in the account's object-storage namespace. No backup metadata is written to
PostgreSQL. Downgrading stops new managed uploads but existing snapshots remain
listable, downloadable and deletable.

## Failure modes

| Failure | Recovery |
|---|---|
| Business/Enterprise required (402) | Upgrade or use a self-managed B2/R2/S3 bucket. |
| Backup token rejected (401) | Re-register the device; this does not change the recovery key. |
| Recovery key missing | Import the exported key. Do not create a replacement if old snapshots are needed. |
| Local verification fails | The upload is not attempted; inspect disk space and `age`/`tar`. |
| Object storage unavailable | The persistent systemd timer retries on its next run; run manually after recovery. |
| Wrong recovery key | Import the key whose `key_id` matches the snapshot catalog. |

## Related

- [Accounts threat model](../concepts/threat-model-accounts.md)
- [Back up to your own bucket](backup-and-restore.md)
