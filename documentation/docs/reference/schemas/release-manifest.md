# Release manifest (`manifest.json`)

A **release manifest** is the signed JSON file that describes one
deputyOS release. It lives at the channel root of the deputyOS CDN
(`https://cdn.deputyos.com/<channel>/manifest.json` in production;
under `dist/` during development) and is the entry point for
`deputyctl update --check`, the desktop launcher, and any third party
that wants to verify, mirror, or audit a release.

The manifest is **schema-versioned** (`schema_version: 1`), strict on
schema version, and forgiving on unknown fields (older `deputyctl`
binaries can read newer manifests for informational purposes; they
just refuse `schema_version != 1` so semantic interpretation never
silently drifts).

The Rust struct is in
[`deputyctl/src/release.rs`](https://github.com/deputyos/deputyos/blob/main/deputyctl/src/release.rs);
the on-disk JSON Schema is at `docs/schemas/manifest-v1.json` (also
quoted below — this site is self-contained).

[TOC]

## Top-level shape

| Field | Type | Required | Description |
|---|---|---|---|
| `schema_version` | const `1` | yes | Hard-pinned. Any other value causes parsers to refuse the manifest with a "upgrade deputyctl" error. |
| `release_version` | string | yes | CalVer Y.M.D, optionally suffixed (e.g. `2026.4.27`, `2026.4.27-rc1`). Validated against `^\d{4}\.\d{1,2}\.\d{1,2}(-[a-z0-9.-]+)?$`. |
| `channel` | enum | yes | One of `dev`, `beta`, `stable`. |
| `released_at` | string (RFC 3339) | yes | Timestamp the manifest was generated. |
| `tracker` | object\<string,string\> | optional | Map of profile id → upstream tag baked into this release (e.g. `{"openclaw": "0.31.4"}`). Free-form; consumers may ignore unknown keys. |
| `artefacts` | array | yes | One entry per signed image / OCI ref / cloud snapshot. Must be non-empty. |
| `wizard_version` | string | optional | Version of the bundled `deputywizard` binary. Informational. |
| `chat_ui_version` | string | optional | Version of the bundled chat UI assets. Informational. |
| `mounts_policy_schema_version` | integer (≥1, default 1) | optional | Schema version of `/etc/deputyos/mounts-policy.json` this release ships (M3.5). Bumped when the on-disk policy schema changes; consumers detect mismatches and migrate. Older manifests omit it and read as `1`. |

## `artefacts[]` — per-image entry

Each artefact is one signed deliverable for one (target, profile) pair.

| Field | Type | Required | Description |
|---|---|---|---|
| `target` | string | yes | Hardware target id (matches the `TARGET=` matrix; e.g. `qemu-aarch64`, `rpi5`, `digitalocean`, `fly-machines`). |
| `profile` | string | yes | Profile id (matches `profiles/<id>.toml`; e.g. `openclaw`, `hermes`, `khoj`). |
| `filename` | string | yes | Basename of the artefact, per the deputyOS naming convention `deputyos-<profile>-<target>-<version>-<channel>.<ext>`. |
| `format` | enum | yes | One of `img.xz`, `qcow2`, `tar.gz`, `do-snapshot`, `oci`. |
| `size_bytes` | integer ≥ 0 | yes | Size of the artefact in bytes. |
| `sha256` | hex string | yes | Lowercase hex-encoded SHA-256 of the artefact bytes. Pattern `^[a-f0-9]{64}$`. |
| `minisig_url` | string | yes | URL of the detached minisign signature (`<filename>.minisig`). May be relative or absolute. |
| `url` | string | optional | URL of the artefact itself. May be relative; resolved against the manifest's own URL. |

## URL resolution

Artefact and signature URLs may be **relative**. They resolve against
the parent of the manifest's URL:

| Manifest origin | Relative `url` | Resolved |
|---|---|---|
| `https://cdn.deputyos.com/dev/manifest.json` | `2026.4.27/deputyos-openclaw-qemu-aarch64-2026.4.27-dev.qcow2` | `https://cdn.deputyos.com/dev/2026.4.27/deputyos-openclaw-qemu-aarch64-2026.4.27-dev.qcow2` |
| `file:///home/me/dist/manifest.json` | `2026.4.27/foo.qcow2` | `file:///home/me/dist/2026.4.27/foo.qcow2` |
| anywhere | `https://elsewhere/foo` (absolute) | `https://elsewhere/foo` (passes through) |

Resolution is implemented in `release::resolve_url`.

## CalVer release-version rules

`release_version` is **CalVer**, not SemVer. The validator
(`release::is_valid_release_version`) enforces:

- Exactly three numeric parts separated by `.`.
- Year (first part) must be 4 digits.
- Month and day must be 1–2 digits.
- Optional pre-release suffix after a single `-`. Empty suffix is rejected.

Examples accepted: `2026.4.27`, `2026.04.27`, `2026.4.27-rc1`,
`2026.4.27-pre.1`.

Examples rejected: `2026`, `2026.4`, `v2026.4.27`, `2026.4.27-`.

Newer-than comparison (`release::is_newer`) parses each part numerically
and falls back to lexicographic compare for the suffix. A version with
no suffix sorts **greater** than the same version with any suffix
(`2026.4.27 > 2026.4.27-rc1`).

## Signature scheme

Manifests are signed with **minisign** (Ed25519 over BLAKE2b). The
companion signature lives next to the manifest as `manifest.json.minisig`
and is fetched + verified by `deputyctl update --check`. Verification
shells out to the system `minisign` binary so the trust surface is
small and reuses the same code path the build pipeline uses to sign.

```
manifest.json
manifest.json.minisig    ← detached signature
```

The verification command (run via `release::verify_manifest_signature`):

```sh
minisign -V -p <pubkey> -m <manifest> -x <sig>
```

Each artefact also has its own `.minisig`; `deputyctl update --apply`
verifies both the manifest signature *and* the per-artefact signature
before staging. See [Security / Update trust chain](../../security/update-trust-chain.md)
for the end-to-end flow.

## Full real example

```json
{
  "schema_version": 1,
  "release_version": "2026.4.27",
  "channel": "stable",
  "released_at": "2026-04-27T08:00:00Z",
  "tracker": {
    "openclaw": "0.31.4",
    "hermes": "0.11.0",
    "khoj": "1.32.0"
  },
  "wizard_version": "2026.4.27",
  "chat_ui_version": "2026.4.27",
  "artefacts": [
    {
      "target": "rpi5",
      "profile": "openclaw",
      "filename": "deputyos-openclaw-rpi5-2026.4.27-stable.img.xz",
      "format": "img.xz",
      "size_bytes": 1395261440,
      "sha256": "4b3c2a1f0e9d8c7b6a5f4e3d2c1b0a9f8e7d6c5b4a3f2e1d0c9b8a7f6e5d4c3b",
      "minisig_url": "2026.4.27/deputyos-openclaw-rpi5-2026.4.27-stable.img.xz.minisig",
      "url": "2026.4.27/deputyos-openclaw-rpi5-2026.4.27-stable.img.xz"
    },
    {
      "target": "x86_64-mini-pc",
      "profile": "openclaw",
      "filename": "deputyos-openclaw-x86_64-mini-pc-2026.4.27-stable.img.xz",
      "format": "img.xz",
      "size_bytes": 1612342272,
      "sha256": "9a8b7c6d5e4f3a2b1c0d9e8f7a6b5c4d3e2f1a0b9c8d7e6f5a4b3c2d1e0f9a8b",
      "minisig_url": "2026.4.27/deputyos-openclaw-x86_64-mini-pc-2026.4.27-stable.img.xz.minisig",
      "url": "2026.4.27/deputyos-openclaw-x86_64-mini-pc-2026.4.27-stable.img.xz"
    },
    {
      "target": "digitalocean",
      "profile": "openclaw",
      "filename": "deputyos-openclaw-digitalocean-2026.4.27-stable.do-snapshot",
      "format": "do-snapshot",
      "size_bytes": 0,
      "sha256": "0000000000000000000000000000000000000000000000000000000000000000",
      "minisig_url": "2026.4.27/deputyos-openclaw-digitalocean-2026.4.27-stable.do-snapshot.minisig",
      "url": "do://snapshots/deputyos-openclaw-2026.4.27-stable"
    },
    {
      "target": "fly-machines",
      "profile": "hermes",
      "filename": "deputyos-hermes-fly-machines-2026.4.27-stable.oci",
      "format": "oci",
      "size_bytes": 524288000,
      "sha256": "1c2d3e4f5a6b7c8d9e0f1a2b3c4d5e6f7a8b9c0d1e2f3a4b5c6d7e8f9a0b1c2d",
      "minisig_url": "2026.4.27/deputyos-hermes-fly-machines-2026.4.27-stable.oci.minisig",
      "url": "registry.fly.io/deputyos/hermes:2026.4.27-stable"
    }
  ]
}
```

## Companion files

Per release, the CDN serves:

| File | Description |
|---|---|
| `<channel>/manifest.json` | The manifest itself. |
| `<channel>/manifest.json.minisig` | Detached minisign signature for the manifest. |
| `<channel>/<release_version>/<filename>` | Each artefact. |
| `<channel>/<release_version>/<filename>.minisig` | Per-artefact signature. |

The CDN root is the deputyOS infrastructure concern; manifest consumers
only need the URL of `manifest.json`.

## JSON Schema (`docs/schemas/manifest-v1.json`)

The single source of truth for the schema. Quoted verbatim:

```json
{
  "$schema": "http://json-schema.org/draft-07/schema#",
  "$id": "https://www.deputyos.com/schemas/manifest-v1.json",
  "title": "deputyOS release manifest",
  "description": "Single-source-of-truth schema for the signed manifest.json that lives at the channel root of the deputyOS CDN (or, in dev, under dist/). Generated by scripts/manifest.sh and consumed by deputyctl::release. Schema version 1; bump on breaking changes.",
  "type": "object",
  "required": [
    "schema_version",
    "release_version",
    "channel",
    "released_at",
    "artefacts"
  ],
  "properties": {
    "schema_version": { "const": 1 },
    "release_version": {
      "type": "string",
      "description": "CalVer Y.M.D, optionally with a pre-release suffix (e.g. 2026.4.27, 2026.4.27-rc1).",
      "pattern": "^\\d{4}\\.\\d{1,2}\\.\\d{1,2}(-[a-z0-9.-]+)?$"
    },
    "channel": { "enum": ["dev", "beta", "stable"] },
    "released_at": {
      "type": "string",
      "format": "date-time",
      "description": "RFC 3339 / ISO 8601 timestamp of when this manifest was generated."
    },
    "tracker": {
      "type": "object",
      "description": "Upstream agent versions baked into this release (profile id -> upstream tag). Free-form; consumers may ignore unknown keys.",
      "additionalProperties": { "type": "string" }
    },
    "artefacts": {
      "type": "array",
      "minItems": 1,
      "description": "Every signed image, OCI ref, or cloud snapshot in this release.",
      "items": {
        "type": "object",
        "required": [
          "target",
          "profile",
          "filename",
          "format",
          "size_bytes",
          "sha256",
          "minisig_url"
        ],
        "properties": {
          "target": {
            "type": "string",
            "description": "Hardware target id matching the TARGET= matrix (e.g. qemu-aarch64, rpi5, digitalocean)."
          },
          "profile": {
            "type": "string",
            "description": "Profile id matching profiles/<id>.toml (e.g. openclaw, hermes)."
          },
          "filename": {
            "type": "string",
            "description": "Basename of the artefact, per docs/03-image-builds.md naming: deputyos-<profile>-<target>-<version>-<channel>.<ext>"
          },
          "format": {
            "enum": ["img.xz", "qcow2", "tar.gz", "do-snapshot", "oci"]
          },
          "size_bytes": { "type": "integer", "minimum": 0 },
          "sha256": {
            "type": "string",
            "pattern": "^[0-9a-f]{64}$"
          },
          "minisig_url": {
            "type": "string",
            "description": "URL (https:// or relative or file://) of the detached minisign signature for this artefact."
          },
          "url": {
            "type": "string",
            "description": "Absolute or relative URL of the artefact. Relative URLs resolve against the manifest's own URL."
          }
        }
      }
    },
    "wizard_version": { "type": "string" },
    "chat_ui_version": { "type": "string" }
  }
}
```

## Generator

`scripts/manifest.sh` produces a manifest from the artefacts in
`dist/`. It computes SHA-256s, fills the per-artefact entries, and
either signs (when `MINISIGN_SECRET_KEY` is in the environment) or
emits an unsigned manifest with a `*.unsigned` suffix for diff review
before signing. Details: [Build / Make targets](../../build/make-targets.md).

## See also

- [Security / Update trust chain](../../security/update-trust-chain.md) —
  end-to-end signature verification, who holds the signing key, key
  rotation policy.
- [Reference / CLI / deputyctl](../cli/deputyctl.md) — `update --check`,
  `update --apply` reference.
- [Operations / Update and rollback](../../operations/update-and-rollback.md) —
  the operator-facing workflow.
- [Distribution / Hardware matrix](../../distribution/hardware-matrix.md) —
  the targets that appear in `artefacts[].target`.
- [Reference / Schemas / Profile manifest](profile-toml.md) — the
  `profile` field references one of these.
