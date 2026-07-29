# Update trust chain

## What this guide does

Document the full **trust chain** for deputyOS updates: how a release
manifest is signed, how the on-device verifier checks it, where the
public key lives, the dev-key vs release-key split, and the SLSA L3
attestation + SBOM that ride alongside every release. This page is
the reference point for anyone evaluating "can I trust the update
loop?" before deploying deputyOS.

## The trust chain in one diagram

```text
┌──────────────────────────────────┐
│ scripts/manifest.sh              │
│   build dist/manifest.json       │
└──────────────┬───────────────────┘
               │
               ▼
┌──────────────────────────────────┐
│ scripts/sign.sh --release        │
│   minisign sign manifest.json    │
│   → manifest.json.minisig        │
└──────────────┬───────────────────┘
               │
               ▼
   CDN / dist/ mirror
               │
               ▼
┌──────────────────────────────────┐
│ deputyctl update --check          │
│   1. fetch manifest + sig        │
│   2. verify sig against pubkey   │
│   3. parse, check schema_version │
│   4. compare release_version     │
└──────────────┬───────────────────┘
               │  (--apply only)
               ▼
┌──────────────────────────────────┐
│   5. download artefact           │
│   6. verify artefact sha256      │
│   7. verify artefact .minisig    │
│   8. fire update-applied hook    │
│   9. stage at /var/lib/deputyos/  │
└──────────────────────────────────┘
```

Every link is a hard fail on mismatch. There is no `--force` flag.

## Manifest signing

### Algorithm: minisign / Ed25519

deputyOS uses [minisign](https://jedisct1.github.io/minisign/) — a
small, well-audited Ed25519 signing tool. The detached signature for
`manifest.json` is `manifest.json.minisig`.

### What's signed

The **bytes** of `manifest.json`. The signature covers:

- The manifest schema version.
- The release version (Y.M.D CalVer).
- The release channel (`dev` / `beta` / `stable`).
- Every artefact entry: filename, format, size, **sha256**, signed
  URL.
- Per-artefact `.minisig` URLs for belt-and-braces verification.

So the chain is: signed manifest → contains every artefact's sha256 →
artefact bytes verifiable against that sha256.

### Public key distribution

The release pubkey is **baked into every image** at
`/etc/deputyos/pubkey.minisign`. The `update::resolve_pubkey` precedence
(see `deputyctl/src/update.rs`):

1. `DEPUTYOS_UPDATE_PUBKEY` env var (dev override).
2. `/etc/deputyos/pubkey.minisign` (the bake-time copy).
3. `<repo>/dist/pubkey.minisign` (dev fallback for contributor laptops).
4. `~/.config/deputyos/dev-keys/pubkey.minisign` (per-contributor dev key).

Production images never see paths 3 or 4 — only 1 and 2 are exercised
on a real device.

### URL distribution

The manifest URL is baked into every image at
`/etc/deputyos/update-url`. Same precedence as the pubkey (env override
→ baked file → `file://<repo>/dist/manifest.json` dev fallback).

## On-device verification

### `deputyctl update --check`

In order:

1. Resolve update URL.
2. Fetch `manifest.json` and `manifest.json.minisig` to a tmp dir.
3. Run minisign verify with the resolved pubkey. **Hard fail** on
   bad signature; non-zero exit, partial files cleaned up.
4. Parse the manifest. **Refuse** if `schema_version != 1` (semantic
   interpretation of v2 is undefined for v1 code).
5. Compare `release_version` to `/etc/deputyos/version`. Skip artefact
   verification if not newer.
6. Find the artefact for this device's `(target, profile)`. Refuse if
   none — a release that omits this device is a broken release.
7. Print the would-update info.

### `deputyctl update --apply`

Adds, after the `--check` chain:

8. Download the artefact (resolved against the manifest's URL — the
   manifest may declare relative artefact URLs).
9. Compute sha256 over the downloaded bytes; compare to the
   manifest's declared sha256. **Hard fail** on mismatch — partial
   download deleted.
10. Fetch the artefact's detached `.minisig`. Run minisign verify
    against the pubkey. **Hard fail** on mismatch — staged artefact
    deleted.
11. Fire the `update-applied` hook (failures here are advisory; the
    artefact is staged regardless).
12. Print the staged path.

The double signing (manifest + per-artefact) lets independent
verifiers attest a single image without parsing the manifest. It also
lets the manifest itself be re-signed (rotating the manifest signing
key) without re-signing every artefact.

## Dev-key vs release-key

The split lives in `scripts/sign.sh`:

- `scripts/sign.sh --dev` — auto-generates a contributor keypair
  under `~/.config/deputyos/dev-keys/` if absent. Loud about which key
  was used. Pubkey is checked into the repo at `dist/dev-pubkey/`
  for cross-contributor verification.
- `scripts/sign.sh --release` — reads the release key from
  `$DEPUTYOS_RELEASE_KEY` (a path). CI writes the corresponding GitHub secret's
  contents to a mode-0600 temporary file and exports that path. The command
  refuses to run on a dev laptop without an explicit key path.

The two paths share the signing code; only key sourcing differs. That
is the point — the release path is exercised every CI run, so a key-
loading bug never surprises a real release.

The release pubkey baked into shipped images is the **release-key
pubkey**, not the dev-key pubkey. Dev-signed artefacts will not
verify against a release image's pubkey, by design.

## SLSA L3 attestation

Every signed release artefact gets a **SLSA v1.0 in-toto provenance
attestation** alongside it, generated by `scripts/slsa-attest.sh`.

### What it contains

A valid in-toto Statement v1.0 with:

- `predicateType: https://slsa.dev/provenance/v1`.
- The artefact's sha256.
- The builder identity (`predicate.builder.id`).
- Invocation parameters (env keys only, **no values** — no secret
  leak).
- Materials (base image SHA from packer if traceable; role's git SHA).
- Build start / finish timestamps.

### What's signed

The Statement bytes are signed with `cosign attest-blob` if available,
else minisign with the same release dev key. Detached signature
shipped as `<artefact>.intoto.jsonl` plus a `.cosign.sig` or
`.minisig` next to it.

### Today's verifier story

**Generator: live.** Every release in M7 ships SLSA attestations.
**Third-party verifier: deferred to M7 close** — full SLSA L3 requires
hermetic builds (M4 bit-pinned; mostly done) plus bit-identical
reproduction across independent builders (still in progress). The
generator-side scaffold lets early consumers start consuming the
attestations even before the verifier is shrink-wrapped.

`make slsa-attest ARTEFACT=<path>` — single artefact.
`make slsa-all` — every signable artefact in `build/`.

## CycloneDX SBOM

`scripts/sbom.sh` emits a CycloneDX SBOM next to every artefact. Uses
`syft` if installed; falls back to a best-effort `dpkg -l` + `pip
freeze` enumeration otherwise.

`make sbom ARTEFACT=<path>` — single artefact.
`make sbom-all` — every signable artefact in `build/`.

The SBOM is a separate file from the SLSA attestation — both can be
consumed independently. CycloneDX is the same format Anthropic, GitHub,
and the broader supply-chain tooling use, so existing scanners
(Trivy, Grype, OSV-Scanner, Snyk) consume them out of the box.

## Local reproduction: `make verify`

The "can I rebuild this and prove the bytes match?" check.

```sh
make verify VERSION=2026.4.27 TARGET=qemu-aarch64 PROFILE=openclaw
```

Rebuilds locally with the same hermetic invariants and SHA-compares
against the published manifest. See
[Operations → Update and rollback](../operations/update-and-rollback.md)
for the operator-facing flow.

`DEPUTYOS_VERIFY_STRICT=1` makes mismatches fatal. Default: warn-only,
exit 0 — until M7 lands fully reproducible builds, mismatches are
expected on host-toolchain divergence.

## Threat model coverage

| Threat | Defense |
| --- | --- |
| Compromised CDN delivers altered manifest | Manifest signature verify (step 3) |
| Compromised CDN delivers altered artefact | Manifest sha256 + per-artefact sig (steps 9, 10) |
| Compromised dev key signs a malicious image | Release pubkey baked at image bake time; dev-signed images won't verify |
| Stolen release key | Rotate pubkey, re-bake images. The pubkey is baked, so old images don't trust the new key — they need a re-bake to update. |
| Replay of old (vulnerable) version | `release_version` comparison (step 5) — same-version is "no update", older is unsupported |
| Corrupt download | sha256 verify (step 9) |
| Manifest schema evolution | `schema_version != 1` refuses (step 4) |

## Verification

```sh
# Confirm the pubkey is baked
sudo file /etc/deputyos/pubkey.minisign

# Confirm the update URL is baked
sudo cat /etc/deputyos/update-url

# Run a verify pass
sudo deputyctl update --check --json | jq

# Local rebuild
make verify VERSION=<v> TARGET=<hw> DEPUTYOS_VERIFY_STRICT=1
```

## Troubleshooting

!!! warning "Signature verify fails on a known-good manifest"
    Common causes: clock skew (the manifest's `released_at` field is
    informational, not signed-time-authoritative, but minisign's tmp
    file handling can stumble on extreme skew), or a mismatched
    pubkey. Compare `sha256sum /etc/deputyos/pubkey.minisign` to the
    repo's `dist/release-pubkey.minisign`.

!!! warning "Manifest parses but `schema_version=2`"
    Future schema. This image's `deputyctl` doesn't know v2; re-bake
    the appliance to a newer image revision that does. The forward-
    compat strategy is "old code refuses unknown schema_version",
    which is the right default — silent v2-to-v1 coercion would be a
    correctness footgun.

!!! danger "Setting DEPUTYOS_UPDATE_PUBKEY to a contributor's dev key on a production device"
    Don't. The env override exists for contributor laptops. Setting
    it on production circumvents the bake-time trust anchor — every
    release signed by that contributor's dev key would then "verify."
    This is a footgun the doc surfaces deliberately so it's not used
    by accident.

!!! tip "Re-baking after a key rotation"
    The release pubkey is baked into images. A pubkey rotation
    requires re-baking every supported image revision (so they trust
    the new pubkey going forward). Until then, rotated-out keys must
    keep signing alongside the new key — a brief overlap window.
    The release-tracker bot handles this with a "next-key + cur-key"
    co-sign mode in M7.

## Related

- [Reference → Schemas → release manifest](../reference/schemas/release-manifest.md)
- [Operations → Update and rollback](../operations/update-and-rollback.md)
- [Build → Make targets](../build/make-targets.md) (`manifest`, `sign-dev`, `sign-release`, `slsa-attest`, `sbom`, `verify`)
- [Concepts → Threat model overview](../concepts/threat-model-overview.md)
- [Security → Default-on controls](default-on-controls.md)
