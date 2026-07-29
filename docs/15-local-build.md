# 15 — Local development builds

The public deputyOS image layer is rebuildable on a contributor's laptop. The
same Packer templates and Ansible role used by the private release pipeline run
on macOS, Windows (WSL2), and Linux. Official images additionally contain the
proprietary resident agent from `deputyos-core`; that payload cannot be rebuilt
from this public repository.

## Why this exists

Three audiences:

1. **Contributors**: clone the repo, change one Ansible task, build, boot in a local VM, smoke-test, ship a PR — without flashing real hardware.
2. **Skeptical users**: inspect and rebuild the public image layer, then verify
   the signed official image, payload checksums, SBOM, and provenance.
3. **Hardware-curious evaluators**: try the appliance in a local VM (UTM on Mac, qemu on Linux, WSL2 on Windows) before buying a Pi.

Contributor builds are deliberately marked `agentless-dev`. They are useful for
public-layer development, but cannot be signed, manifested, or published as
deputyOS images.

## Build hosts

| Host | Status | Notes |
|---|---|---|
| **Linux x86_64** | first-class | Matches CI exactly. `qemu-user-static` for arm64 emulation. ~10–25 min build per target. |
| **Linux arm64** (Pi 5 16GB, Ampere, Apple Silicon under Linux VM) | first-class | Native arm64 builds skip emulation; ~3–8 min for arm64 targets. |
| **macOS (Apple Silicon)** | first-class | Packer + Docker Desktop (or OrbStack) provides Linux containers; arm64 builds run natively, x86 builds use Rosetta-translated qemu. UTM is the recommended local-VM runner. |
| **macOS (Intel)** | supported | qemu-system runs natively; same Packer flow. |
| **Windows + WSL2** | first-class | All Linux tooling runs inside WSL2; Windows host stays clean. WSL2 must be Ubuntu 22.04+ with systemd enabled. |
| **Windows native** | not supported | Use WSL2. We won't maintain a Windows-native build path. |

## Five-minute "try it" quickstart (no clone, no Pi)

For evaluators who just want to see it run:

```sh
# macOS / Linux / WSL2
curl -fsSL https://www.deputyos.com/try.sh | bash
```

The script downloads the latest signed `qcow2`, verifies its SHA256 + minisign, launches qemu (Linux/WSL2) or UTM (macOS), and forwards the wizard to `http://localhost:8088`. Total time on a modern laptop: ~5 minutes including download.

The image is the **macos-qemu** or **qemu-aarch64** target from the published manifest. It's the lean profile (no voice, no local LLM) — the point is to see the appliance working, not to benchmark the heavy targets.

For evaluators who want the heavier targets (3B local LLM, voice), `try.sh --target oracle-arm-free` downloads the 24-GB-class image and runs it under qemu — slower than native but functional.

## Full local build (clone, edit, rebuild)

```sh
git clone https://github.com/deputyos/deputyos.git
cd deputyos
make doctor              # checks Packer, Ansible, qemu, Docker available
DEPUTYOS_IMAGE_KIND=agentless-dev \
DEPUTYOS_ALLOW_AGENTLESS_DEV=1 \
  make build TARGET=qemu-aarch64 PROFILE=openclaw
make try TARGET=qemu-aarch64
```

Total time on a Linux x86_64 host: ~12 minutes for the qemu target on a fresh checkout (Ansible role caches help on subsequent builds).

### `Makefile` surface (committed in M1)

| Target | What it does |
|---|---|
| `make doctor` | Checks the build host has the right tools. Tells the user what to install. |
| `make build TARGET=<hw> PROFILE=<id> [CHANNEL=stable|beta] [TIER=lean|standard|rich]` | Packer build. Output to `build/deputyos-<profile>-<hw>-dev-<sha>.img.xz`. |
| `make try TARGET=<hw>` | Build (if not already) + boot in qemu/UTM. Forwards `:8088` to `localhost:8088`. |
| `make smoke TARGET=<hw>` | Runs the same QEMU smoke harness CI uses. Hard pass/fail. |
| `make matrix` | Builds every target in parallel (long; ~45 min on a fast Linux box). |
| `make verify VERSION=<v> [TARGET=<hw>]` | Verifies the signed published artifact and its provenance; private-core access is required for a byte-identical rebuild. |
| `make clean` | Wipes `build/` and the Ansible cache. |
| `make sign-dev` | Generates a contributor's dev minisign keypair under `~/.deputyos/dev-keys/`. |

The Makefile dispatches to a `tools/deputyos-build` Bash script that knows the host OS and chooses the right Packer plugin path; users typing `make build` never see the dispatch logic.

For users who prefer a single binary, `deputyctl-builder build --target rpi5 --profile openclaw` is the future Rust equivalent (M2). It calls the same scripts; Makefile and `deputyctl-builder` are interchangeable.

## What every local build does, step by step

1. **Pre-flight** (`tools/deputyos-build doctor`): asserts Packer, Ansible, qemu, Docker, and host CPU support are present. Prints install commands per OS for anything missing.
2. **Resolve the target**: looks up `targets/<hw>.yaml` for hardware-specific Packer source + variant gates.
3. **Packer build**: runs the right builder (`packer-builder-arm` for Pi/arm64, Packer QEMU for x86, Packer DigitalOcean for cloud), provisioning via the shared Ansible role with `hw=<hw>`, `profile=<id>`, `tier=<tier>` extra-vars.
4. **Mark as agentless development output**. Public signing and manifest tools
   reject the result, preventing it from being confused with an official image.
5. **Stamp the SBOM**: `syft` produces `/etc/deputyos/sbom.json` inside the image; `/etc/deputyos/build.json` records git commit, builder OS, builder hostname, and `SOURCE_DATE_EPOCH`.
6. **Compress and emit**: `.img.xz` (or `.qcow2`, or DO snapshot, or OCI artefact) lands in `build/`.
7. **Optional `make try`**: launches qemu/UTM with the right machine type, console, and port forwards. The wizard publishes `deputyos-dev.local` (mDNS) and `localhost:8088`.

## The dev-key vs project-key distinction

Every contributor builds with a **dev key** generated by `make sign-dev`. The dev key:

- Lives at `~/.deputyos/dev-keys/`, mode 0600, never committed.
- Signs `dev`-channel manifests only.
- `deputyctl` has separate trust roots: `/etc/deputyos/trusted-keys/stable/` (project minisign + cosign), `/etc/deputyos/trusted-keys/dev/` (any dev key the user has explicitly imported with `deputyctl trust dev <pubkey>`).
- A device built from a stable image will refuse a dev image as an update unless the user explicitly trusts the dev key.

This means a contributor cannot accidentally produce something that masquerades as a stable release, and a user cannot install a third-party dev image without an explicit consent step.

## Iteration loop (the contributor experience)

```
edit roles/deputyos/tasks/profile-hermes.yml
DEPUTYOS_IMAGE_KIND=agentless-dev DEPUTYOS_ALLOW_AGENTLESS_DEV=1 \
  make build TARGET=qemu-aarch64 PROFILE=hermes
make try TARGET=qemu-aarch64                        # boots image
# wizard runs at http://localhost:8088
# verify the change works
make smoke TARGET=qemu-aarch64                      # smoke harness asserts baseline
git commit -m "..."
gh pr create
```

Hot reload tricks:

- **Ansible cache**: subsequent builds skip already-applied tasks unless their content changed.
- **qemu snapshot**: `make try-fast` boots a previously-snapshotted image with your new deputyctl binary copied in via `9p` filesystem mount — useful for iterating on `deputyctl` itself without rebuilding the image.
- **Layer cache**: large bake steps (npm cache populate, ClamAV signature download, Magika model fetch) are layered separately so editing the deputyctl crate doesn't invalidate them.

## Verification (the trust path)

To confirm a published image is what its source says it is:

```sh
make verify VERSION=2026.04.27 TARGET=rpi5 PROFILE=openclaw
```

With access to both repositories, this:

1. Fetches the published manifest and the source tarball at the matching git tag.
2. Runs `make build` with `SOURCE_DATE_EPOCH` set from the manifest's `released_at`.
3. Computes the SHA256 of the resulting image.
4. Compares against the published SHA256.
5. Asserts equal; non-zero exit if not.

Public-only users verify the published signature, provenance, SBOM, and the
checksums of the private payload embedded by the trusted release pipeline. A
byte-identical rebuild of an official image requires access to the private
resident source and pinned toolchain.

A successful full rebuild proves that the image matches both its pinned public
source revision and private-core revision. We do not claim that an official
image can be reconstructed from the public repository alone.

## Per-host-OS specifics

### macOS (Apple Silicon)

- **arm64 targets** (`rpi5`, `rpi4`, `arm64-generic`, `oracle-arm-free`, `qemu-aarch64`) build natively — no emulation, fast.
- **x86 targets** (`x86_64-mini-pc`, `digitalocean`, `qemu-x86_64`) build under qemu; ~3× slower than Linux x86_64.
- **Local-VM run** (`make try`): UTM is the recommended runner. Falls back to qemu-system if UTM isn't installed.
- **Required tools**: Homebrew → `packer`, `ansible`, `qemu`, `docker` (or OrbStack), `xz`. `make doctor` prints the exact `brew install` line.
- Notable footgun: macOS doesn't expose `/dev/kvm`; nested KVM acceleration uses Apple's HVF instead. Build performance is good; runtime VM performance for x86 emulation is the slow case.

### Windows (WSL2)

- **All builds run inside WSL2** (Ubuntu 22.04 LTS+). Windows host stays clean.
- **systemd must be enabled** in WSL2 (`[boot] systemd=true` in `/etc/wsl.conf`). `make doctor` checks this.
- **Local-VM run**: qemu-system from inside WSL2; the wizard URL `localhost:8088` reaches the Windows host via WSL2's networking.
- **`wsl2` target**: special — produces a `.wsl` distro tarball that's installable with `wsl --import`. Not a qemu image; runs natively.
- Required tools: same as Linux x86_64 (everything inside WSL2).

### Linux x86_64

- The canonical build host. CI runs Linux x86_64 + `qemu-user-static`.
- arm64 builds use `binfmt_misc` registration; emulation is via the static qemu user binary.
- Required: `packer`, `ansible-core`, `qemu-system-arm`, `qemu-system-x86`, `qemu-user-static`, `docker`, `xz`, `mkfs.ext4`, `parted`. Distro packages.
- Build speed: ~10–25 min per target on a 16-core/32 GB box.

### Linux arm64

- Native builds for arm64 targets are 3–4× faster than x86_64 + emulation.
- This is the right host for the Hailo and Coral build variants if the dev wants to test the runtime themselves.
- A Pi 5 16GB is a serviceable build host for arm64 targets.

## Local-build limitations (named explicitly per [docs/14-limitations.md](14-limitations.md))

- **Reproducibility requires SLSA L3 (M7)** to be third-party-verifiable; M0–M6 builds are reproducible-best-effort.
- **macOS qemu cannot run NPU-accelerated builds with hardware acceleration**; the Hailo/Coral variants build but smoke tests for accelerator-using paths are skipped.
- **Public local builds are agentless development bases** and cannot enter the
  signing or manifest path.
- **WSL2 cannot run the audio/voice paths** in `make try` even on a target that supports them — Windows doesn't pass `/dev/snd` through.
- **Build host disk needs 30+ GB free** for a single-target build, 80+ GB for `make matrix`.
- **First build downloads ~5 GB** of base images, package caches, and signature DBs from the upstream sources. Subsequent builds reuse the local Ansible/Packer cache.

## How this slots into the roadmap

- **M1** ships: `make doctor`, `make build TARGET=qemu-aarch64`, `make try TARGET=qemu-aarch64`, `make smoke TARGET=qemu-aarch64`. Just enough for the OpenClaw walking skeleton to be hackable.
- **M2** ships: full `make matrix` covering every published target. `try.sh` quickstart goes live on the website.
- **M3** ships: `make build` produces an image whose wizard works end-to-end on the dev's laptop in qemu/UTM/WSL2.
- **M4** ships: `make verify VERSION=<v>` against any published manifest.
- **M7** ships: SLSA L3 attestations verified by `make verify` against an arbitrary third-party reproducer.

`deputyctl-builder` (Rust) is the M2 successor to the bash scripts; users who'd rather not have Make / Bash on their machine can use it instead. The Makefile remains the contributor-facing entry point because contributors live with Make daily.

## What this does for the project

- **Day-one contributor onboarding**: clone, `make try`, edit, repeat. No special access, no reach-out for keys.
- **Trust through verifiability**: users can validate signatures, public-layer
  provenance, SBOMs, and private payload checksums without publishing the
  resident implementation.
- **Adoption beyond hardware buyers**: a curious developer evaluates in qemu without committing to a Pi. If they like it, *then* they buy hardware.
- **Education**: the Ansible role is the source of truth for "what is in this appliance." Reading it once is the fastest way to understand the project.
