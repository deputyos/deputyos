# Update and rollback

## What this guide does

Walk through the update and rollback flow on deputyOS: what `deputyctl
update --check` and `deputyctl update --apply` do today, what `deputyctl
rollback` does today, what's deferred to milestone M6 (the actual A/B
slot swap and watchdog auto-rollback), and how `make verify` lets you
**rebuild** a published release locally and SHA-compare against the
signed manifest.

The update trust chain is detailed in
[Security → Update trust chain](../security/update-trust-chain.md);
this page is the operator-facing flow.

## The update flow

### `deputyctl update --check`

```sh
sudo deputyctl update --check
sudo deputyctl update --check --json
```

Performs:

1. Read the update URL from `/etc/deputyos/update-url` (or
   `DEPUTYOS_UPDATE_URL` env override; or dev fallback to
   `file://<repo>/dist/manifest.json`).
2. Fetch `manifest.json` and `manifest.json.minisig` to a tmp dir.
3. **Verify the minisign signature** against
   `/etc/deputyos/pubkey.minisign`. Hard fail on mismatch.
4. Parse the manifest. Refuse if `schema_version != 1`.
5. Compare `release_version` to the contents of
   `/etc/deputyos/version` (the running image's version).
6. If newer, find the artefact entry matching this device's
   `(target, profile)` pair.
7. Print: `info: update available: <version>` plus the artefact's
   sha256 and signed URL.

`--json` emits the same content as a structured object suitable for
piping to `jq`.

### `deputyctl update --apply`

```sh
sudo deputyctl update --apply --yes
```

Performs everything `--check` does, **plus**:

1. Download the artefact to `/var/lib/deputyos/staging/<filename>`.
2. SHA-256 verify against the manifest entry. Hard fail on mismatch
   (delete the partial download).
3. Verify the artefact's detached `.minisig` against the manifest's
   declared signature URL. (Belt-and-braces: the manifest itself is
   signed, but per-artefact sigs let independent verifiers confirm
   any one image without parsing the manifest.)
4. Fire the `update-applied` hook with payload:

    ```json
    {"kind":"update-applied",
     "staged_at":"/var/lib/deputyos/staging/<filename>",
     "filename":"<filename>","sha256":"<hex>","release_version":"Y.M.D"}
    ```

5. Print: `info: staged at <path>; A/B swap on next boot is M6`.

!!! note "M6 deferral"
    Today, `--apply` ends at "staged." The actual slot swap requires
    bootloader plumbing (tryboot on Pi 5, U-Boot bootcount on Pi 4,
    GRUB swap on x86) that lands in milestone M6. The hook **does**
    fire on the staging step so admin tooling can refresh dashboards
    eagerly — the staged artefact is the proof an update is ready,
    even if the swap hasn't happened.

### `deputyctl rollback`

```sh
sudo deputyctl rollback
```

The contract today (per `deputyctl/src/rollback.rs`):

1. Identify the inactive A/B slot.
2. Validate its integrity — sha256 against the manifest that produced
   it, plus an "is the unit file there" probe.
3. **Refuse to swap** with a clear "M6 deferral" message and a
   non-zero exit.

The validation step proves the rollback path is wired and the inactive
slot is intact. The actual `reboot --bootloader-set inactive-slot`
lands in M6.

## `make verify` — local reproducibility

```sh
make verify VERSION=2026.4.27 TARGET=qemu-aarch64 PROFILE=openclaw
```

Performs:

1. Read `dist/manifest.json` (or `DEPUTYOS_UPDATE_URL`).
2. Find the artefact for `(VERSION, TARGET, PROFILE)`.
3. Run `make build TARGET=$TARGET PROFILE=$PROFILE` to **rebuild**
   the image locally with the same hermetic-build invariants.
4. SHA-256 the rebuilt artefact.
5. Compare to the manifest's published sha256.

Set `DEPUTYOS_VERIFY_STRICT=1` to make a mismatch fatal (default:
warning, exit 0). The non-strict default exists so contributors with
slightly different host toolchains can see "yes, this rebuilds, but
the bytes diverge here" rather than a hard fail — which is the right
shape until M7 lands fully reproducible builds.

The bigger story (SLSA L3 attestation) is in
[Security → Update trust chain](../security/update-trust-chain.md).

## Watchdog auto-rollback

Deferred to **M6**. The plan: a systemd watchdog on the active
profile's gateway that, on N consecutive crashes within a window,
flips the boot-slot pointer to the inactive slot and reboots. The
inactive-slot validity check from `deputyctl rollback` is the
foundation; the watchdog plumbing is the piece M6 still owes.

For now, manual rollback (when M6 lands) plus `deputyctl doctor` in a
systemd timer is the substitute. A timer-driven `deputyctl doctor`
exit-non-zero plus an alert hook gives you the same "something is
broken, page someone" signal without auto-recovery.

## Failure modes

| Failure | Where | Recovery |
| --- | --- | --- |
| Manifest signature invalid | `update --check` | Refuses, exit 2. Don't override; investigate the signing key path. |
| Artefact sha256 mismatch | `update --apply` | Refuses, deletes partial. Rerun — likely transient CDN. |
| Artefact `.minisig` invalid | `update --apply` | Refuses. Same as above. |
| Network down | `update --check` | Reports DNS / connect timeout. Retry when network returns. |
| Missing artefact for `(target, profile)` | `update --check` | Reports "no artefact in manifest for this device." Indicates a broken release. |
| Disk full at staging | `update --apply` | Reports `ENOSPC`. Free space, retry. |
| `update-applied` hook script fails | post-staging | Logs; staging is already complete. Hook failures don't roll back staging. |

## Verification

```sh
# Doc-driven local loop
make publish-local                           # mirror dist/ for file:// CDN
sudo deputyctl update --check                 # should detect the local mirror
sudo deputyctl update --apply --yes           # stages
ls /var/lib/deputyos/staging/                 # verify
sudo deputyctl rollback                       # validates inactive, refuses M6 swap
```

## Troubleshooting

!!! warning "`update --check` says 'no update' but the manifest has a newer version"
    Compare `cat /etc/deputyos/version` to the manifest's
    `release_version`. The on-disk version is what the bake wrote;
    if they're equal, you're already on the latest. If they differ,
    look for cached manifest data and `rm -rf
    /var/lib/deputyos/staging/*` before retrying.

!!! warning "`update --apply` succeeds but next reboot doesn't pick up the new image"
    Expected today (M6 deferral). The staged artefact is in
    `/var/lib/deputyos/staging/`; the bootloader still points at the
    old slot. M6 lands the swap.

!!! danger "Bypassing signature verification"
    Don't. The trust chain is the only thing that prevents a
    compromised CDN from delivering a malicious image. There is no
    `--force` flag, by design. If you have a release-key incident,
    rotate the pubkey baked at `/etc/deputyos/pubkey.minisign` and
    re-bake the appliance image. See
    [Security → Update trust chain](../security/update-trust-chain.md).

!!! tip "Use `make verify` before pulling an update onto production"
    On a separate test rig, run
    `make verify VERSION=<v> TARGET=<hw>` before any production roll.
    Even with `DEPUTYOS_VERIFY_STRICT=1`, you get a fast "yes, this
    rebuilds bit-identically" signal.

## Related

- [Reference → CLI → deputyctl](../reference/cli/deputyctl.md) (`update`, `rollback` subcommands)
- [Reference → Schemas → release manifest](../reference/schemas/release-manifest.md)
- [Reference → Schemas → hook payloads](../reference/schemas/hook-payloads.md) (the `update-applied` schema)
- [Security → Update trust chain](../security/update-trust-chain.md)
- [Build → Make targets](../build/make-targets.md) (the `manifest` / `publish-local` / `verify` targets)
