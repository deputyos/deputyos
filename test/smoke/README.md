# Smoke harness

The QEMU smoke harness is the publish gate: every produced artefact must
pass before the release pipeline will publish it. See
`docs/03-image-builds.md` §"QEMU smoke test (the gate)" for the eight
assertions.

## SMOKE_LEVEL

Not every assertion is meaningful at every milestone. `SMOKE_LEVEL`
controls which subset runs:

| Level      | Asserts                                                  | When to use                                           |
|------------|----------------------------------------------------------|-------------------------------------------------------|
| `scaffold` | kernel boots, `deputyctl` and resident `deputyd` work      | Default. Fast image/control-plane confidence.         |
| `m1`       | + `deputyctl doctor`, firewall, deputyd pause/resume       | Normal image validation.                              |
| `full`     | + clamd, Magika, wizard `:8088` healthz, telegram stub   | M3 onwards. CI gates publishes on this level.         |

## Running locally

```sh
# Official image (requires the private deputyos-core build contract)
make smoke TARGET=qemu-aarch64

# After M1 lane work lands:
SMOKE_LEVEL=m1 make smoke TARGET=qemu-aarch64

# Full gate (matches CI):
SMOKE_LEVEL=full make smoke TARGET=qemu-aarch64

# Public-layer development only; never a release candidate
DEPUTYOS_IMAGE_KIND=agentless-dev \
DEPUTYOS_ALLOW_AGENTLESS_DEV=1 \
  make smoke TARGET=qemu-aarch64
```

If the build artefact (`build/<target>-<profile>.qcow2` or `.img.xz`) is
missing, the harness invokes `make build` first.
