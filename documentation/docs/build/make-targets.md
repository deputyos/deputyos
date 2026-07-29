# Make targets

## What this page does

A reference for every `make` target in the deputyOS Makefile — what it
does, what host dependencies it needs, what it produces, and the
common gotchas. The Makefile is **the** API: CI calls `make ci`,
contributors call `make doctor` then `make build`, then `make try` or
`make smoke`. macOS, WSL2, and Linux all use the same surface.

The hard rule (see the
[local-build-first feedback note][localbuild]): every check, build,
smoke, and signing step must work locally. CI is a thin wrapper around
`make ci`.

[localbuild]: https://github.com/deputyos/deputyos/blob/main/docs/15-local-build.md

## Top-level help

```sh
make           # alias for `make help`
make help      # prints every target with one-line descriptions + variables
```

## Variable reference

| Variable | Default | Meaning |
| --- | --- | --- |
| `TARGET` | `qemu-aarch64` | hardware/cloud target (qemu-aarch64, rpi5, rpi4, …) |
| `PROFILE` | `openclaw` | profile id (`openclaw`, `hermes`, `khoj`) |
| `CHANNEL` | `dev` | release channel (`dev`, `beta`, `stable`) |
| `TIER` | `standard` | RAM tier (`low`, `standard`, `high`) |
| `SMOKE_LEVEL` | `m1` | smoke gate (`scaffold`, `m1`, `full`) |
| `SCAFFOLD_PHASE` | `0` | set to `1` to skip build+smoke in `make ci` |
| `DESKTOP_TARGET` | host triple | Rust target triple for `deputyos-desktop` builds |
| `DEPUTYOS_RELEASE_VERSION` | today (Y.M.D) | version string for `make manifest` |
| `DEPUTYOS_VOICE_OFFLINE` | unset | set to `1` to skip voice-asset downloads |
| `DEPUTYOS_VERIFY_STRICT` | unset | set to `1` to make `make verify` mismatches fatal |

## Target groups

### Pre-flight

#### `make doctor`

```text
host deps:    bash, plus the tools the doctor probes for
produces:     stdout-only summary
common gotcha:  none
```

Walks `scripts/doctor.sh` — checks that every host dependency the
build matrix needs is on PATH. Suggests an install command per
missing item. Idempotent; safe to run repeatedly.

### Lint

#### `make fmt`

Format Rust + YAML in place. `cargo fmt --all` plus optional
`yamllint` (warns if missing).

#### `make lint`

Run every linter. The full pipeline:

- `cargo fmt --all --check`
- `cargo clippy --all-targets -- -D warnings`
- `cargo run -p deputyctl -- profile validate profiles/*.toml`
- `ansible-lint roles/` (graceful skip if missing)
- `yamllint roles/ .github/ cloud-init/` (graceful skip)
- `shellcheck scripts/*.sh test/smoke/*.sh macos/*.sh` (graceful skip)
- `packer validate -syntax-only packer/*.pkr.hcl` (graceful skip)
- `pwsh PSScriptAnalyzer wsl/Install-DeputyOS.ps1` (graceful skip)
- `xmllint templates/unraid/deputyos.xml` (graceful skip)
- `jq .` on every shipped JSON file
- `mkdocs build --strict --quiet` for `documentation/` (graceful skip)

The graceful-skip pattern means CI runs every linter that's installed
and skips (with a warning, not a failure) the rest. Install all of
them locally to maximise coverage.

#### `make test`

```text
host deps:    cargo
produces:     cargo test --all output
common gotcha:  the manifest-roundtrip test reads profiles/*.toml; an in-progress profile edit will fail it
```

### Build / try / smoke

#### `make build`

```text
host deps:    cargo, plus packer + qemu-system-* OR packer-builder-arm
              for hardware targets; plus rclone for cloud builds; plus
              buildah/docker for fly-machines
produces:     build/<target>-<profile>.<format>
common gotcha:  the build script downloads voice assets unless
               DEPUTYOS_VOICE_OFFLINE=1; that step needs network or it fails loudly
```

Variables: `TARGET=<hw> PROFILE=<id> CHANNEL=<channel> TIER=<tier>`.

Dispatches to `scripts/build.sh` which routes to the right Packer
template, OCI build (for `fly-machines`), or template-only path (for
`proxmox` / `unraid` / `truenas` / `macos-qemu`). Cloud-init recipe
targets (`hetzner-cloud` / `vultr` / `linode`) print the recipe and
exit 0 — no local artefact.

#### `make try`

```text
host deps:    qemu-system-* (Linux) / UTM (macOS) / WSL2 (Windows)
produces:     a booting VM with port 8088 forwarded to the wizard
```

Builds (if needed) plus boots the artefact under qemu/UTM with
loopback port forwarding. The wizard is reachable at
`http://localhost:8088/`.

#### `make smoke`

```text
host deps:    qemu-system-*, ssh
produces:     pass/fail per assertion in test/smoke/<target>.sh
```

Run the QEMU smoke harness for a target. `SMOKE_LEVEL=m1` is the
default — adds end-to-end assertions on top of the `scaffold` boot
check. `SMOKE_LEVEL=full` enables M2+ assertions.

#### `make matrix`

```text
host deps:    same as build + smoke for both qemu-aarch64 and qemu-x86_64
produces:     two qcow2 artefacts plus their smoke results
```

Build + smoke both QEMU targets in sequence. Proves the variant
matrix end-to-end. CI runs this on every PR (when `SCAFFOLD_PHASE=0`).

`make matrix` is intentionally limited to QEMU-bootable targets.
Real-hardware targets (rpi4 / arm64-generic / x86_64-mini-pc) need
binfmt_misc registered or actual hardware to validate, so they
build via `make build TARGET=<hw>` on a contributor machine.

### Sign

#### `make sign-dev`

Auto-generates a contributor keypair under
`~/.config/deputyos/dev-keys/` if absent. Signs every artefact in
`build/` with minisign. Loud about which key was used.

#### `make sign-release`

Reads the release key from `$DEPUTYOS_RELEASE_KEY` (a path). Release CI stages
the `DEPUTYOS_RELEASE_KEY` GitHub secret's contents into a mode-0600 temporary
file and points the variable at it. The target refuses to run on a dev laptop
without an explicit key path.

### Manifest / Release loop

These targets form Lane D's local-first release loop. They're NOT in
`make ci` because they require real signed artefacts in `build/`,
which `SCAFFOLD_PHASE=1` doesn't produce. The release-tag GitHub
Actions workflow exercises this path in CI.

#### `make manifest`

```text
required:     DEPUTYOS_RELEASE_VERSION=<Y.M.D>
optional:     CHANNEL=<dev|beta|stable>  (default dev)
produces:     dist/manifest.json + dist/manifest.json.minisig
```

Generate `dist/manifest.json` from signed artefacts in `build/`. The
script (`scripts/manifest.sh`) walks `build/*.{qcow2,img,img.xz,tar.gz}`,
computes sha256s, reads sizes, and emits the manifest schema v1.
Release CI first runs `make stage-release-artifacts` to link the generic QEMU
matrix outputs to the required
`deputyos-<profile>-<target>-<Y.M.D>-<channel>` names.

#### `make publish-local`

```text
produces:    dist/<everything for file:// CDN>
```

Mirror signed artefacts + manifest into `dist/` for `deputyctl update
--check` to consume via `file://` URL. Useful for end-to-end testing
the update loop on a contributor laptop.

#### `make verify`

```text
required:     VERSION=<v>
optional:     TARGET=<hw>, PROFILE=<id>, DEPUTYOS_VERIFY_STRICT=1
produces:     SHA256 comparison output; exit 0 (warn) or non-zero (strict)
```

Rebuild a published image and assert SHA256 match against the
manifest's published hash. The local-reproducibility check; see
[Operations → Update and rollback](../operations/update-and-rollback.md).

### Track (release-tracker)

These mirror `.github/workflows/release-tracker.yml` — CI is a thin
wrapper. Requires network unless `DEPUTYOS_TRACK_OFFLINE=1`.

- `make track` — show which profiles have a newer upstream release
  (no writes).
- `make track-propose` — emit propose-`<id>`-`<v>`.patch + .json files
  in `build/track/`.
- `make track-apply` — apply pending bumps to `profiles/<id>.toml`
  (CI-friendly, sets `--yes`).

### Documentation

- `make docs` — `mkdocs serve` on `:8000` for live dev.
- `make docs-build` — `mkdocs build --strict` to `documentation/site/`.

Both require `pip install -r requirements-docs.txt`.

### Wizard / PWA / Desktop launcher

- `make wizard` — runs `deputywizard serve --port 8088 --no-token` for
  dev; open `http://localhost:8088/wizard`.
- `make pwa` — runs `deputypwa serve --port 8089` with stub data; open
  `http://localhost:8089/app/dashboard`.
- `make desktop-launcher` — `cargo build --release -p deputyos-desktop`,
  optionally for a non-host triple via `DESKTOP_TARGET=`.

### SLSA / SBOM (release-time only)

NOT in `make ci`. Exercised by the release workflow.

- `make sbom ARTEFACT=<path>` — single CycloneDX SBOM via `syft` (or
  best-effort fallback).
- `make sbom-all` — every signable artefact in `build/`.
- `make slsa-attest ARTEFACT=<path>` — SLSA v1.0 in-toto provenance
  + sign.
- `make slsa-all` — every signable artefact.

### CI

#### `make ci`

The meta-target. CI runs this. Steps:

1. `make lint`
2. `make test`
3. If `SCAFFOLD_PHASE=0` (default): `make build TARGET=$TARGET
   PROFILE=$PROFILE`, then `make smoke ...`.
4. `make sign-dev`.

The `SCAFFOLD_PHASE=1` escape hatch skips build+smoke when running
on resource-constrained CI runners or during cross-lane development.
Default is `0` (full).

### Cleanup

#### `make clean`

`cargo clean` plus `rm -rf build/ documentation/site/`.

## Cloud-build credentials

Real builds for these targets need provider creds in env:

| Target | Required env |
| --- | --- |
| `digitalocean` | `DIGITALOCEAN_TOKEN` (DO API token) |
| `oracle-arm-free` | OCI CLI configured (`~/.oci/config`) |
| `fly-machines` | `flyctl auth login` (build is local; only deploy needs it) |

Lint and `packer validate` work without these; only real builds need them.

## Common errors

!!! warning "`make build` exits 64 with 'no packer template'"
    The target name is unrecognized. Check `make help` for the
    canonical list. Cloud-init recipe targets
    (`hetzner-cloud` / `vultr` / `linode`) print the recipe and exit
    0 — that's the design, not an error.

!!! warning "`make ci` is slow on a fresh checkout"
    First `cargo build` of the workspace dominates. Subsequent CI
    runs hit the cargo cache. Local CI runs benefit from
    `RUST_MIN_STACK=8388608` for the bigger crates.

!!! tip "Run `make doctor` after every host upgrade"
    A new packer release, a kernel-update that breaks `binfmt_misc`,
    or a Python venv that aged out — all surface as red lines from
    `make doctor`.

## Related

- [Build → Image bake internals](image-bake-internals.md)
- [Distribution → Hardware matrix](../distribution/hardware-matrix.md)
- [Operations → Update and rollback](../operations/update-and-rollback.md)
- [Contributing → Overview](../contributing/overview.md)
