# Verify a Published Release

Reproduce a build and verify its SHA256 matches the published manifest.

## Prerequisites

- Linux x86_64 build host with `make doctor` green
- Git checkout at the release tag

## Steps

```bash
# 1. Checkout the release tag.
git checkout v<version>

# 2. Rebuild the image.
make build TARGET=qemu-aarch64 PROFILE=openclaw

# 3. Verify the SHA256 matches the published manifest.
make verify VERSION=<version> TARGET=qemu-aarch64

# Expected output:
#   verify: deputyos-openclaw-qemu-aarch64-<version>-dev.qcow2
#   verify: local SHA256   abc123...
#   verify: manifest SHA256 abc123...
#   verify: OK — SHA256 match
```

## Verifying from CDN

If the CDN is available, you can also spot-check a published artefact:

```bash
make verify-cdn CDN_URL=https://cdn.deputyos.com VERSION=<version>
```

This fetches the signed manifest from CDN, downloads one artefact, and
compares its SHA256 against the manifest entry.

## SLSA Provenance

Each release includes an in-toto SLSA v1 provenance attestation
(`<artefact>.intoto.jsonl`). To inspect it:

```bash
make slsa-verify ARTEFACT=build/<artefact>.qcow2
```

**Note:** Full SLSA L3 requires bit-identical reproduction across two
independent builders, which is deferred until a second CI build host is
available. The current `make verify` and `make slsa-verify` paths validate
structural correctness and single-builder reproducibility.
