#!/usr/bin/env bash
# scripts/build.sh — dispatch `make build TARGET=...` to the right Packer
# template. Pre-stages an deputyctl release binary, the active profile's
# manifest, the limits.json (if Lane A has shipped it), and a NoCloud
# cloud-init seed (qemu-aarch64 only) into build/staging/. Packer then
# pulls every host-side artefact from there.

set -euo pipefail

TARGET="${TARGET:-qemu-aarch64}"
PROFILE="${PROFILE:-openclaw}"
CHANNEL="${CHANNEL:-dev}"
TIER="${TIER:-standard}"
AIRGAP="${AIRGAP:-0}"
DEPUTYOS_IMAGE_KIND="${DEPUTYOS_IMAGE_KIND:-official}"

repo_root="$(cd "$(dirname "$0")/.." && pwd)"
template="${repo_root}/packer/${TARGET}.pkr.hcl"
staging="${repo_root}/build/staging"

case "$DEPUTYOS_IMAGE_KIND" in
  official)
    core_staging="${DEPUTYOS_CORE_STAGING:-}"
    if [[ -z "$core_staging" || ! -d "$core_staging" ]]; then
      echo "error: official images require DEPUTYOS_CORE_STAGING from the private deputyos-core build" >&2
      exit 66
    fi
    ;;
  agentless-dev)
    if [[ "${DEPUTYOS_ALLOW_AGENTLESS_DEV:-0}" != "1" ]]; then
      echo "error: agentless-dev requires the explicit DEPUTYOS_ALLOW_AGENTLESS_DEV=1 safety switch" >&2
      echo "       agentless development outputs cannot be signed or published as deputyOS images" >&2
      exit 66
    fi
    ;;
  *)
    echo "error: DEPUTYOS_IMAGE_KIND must be official or agentless-dev" >&2
    exit 64
    ;;
esac
export DEPUTYOS_IMAGE_KIND

# ---- Recipe-only TARGETs (no Packer template, no OCI build) ----
# hetzner-cloud, vultr, linode are cloud-init recipes the user pastes
# into the provider's User-Data field. There is no local artefact to
# build; print the recipe path + a copy-paste hint and exit 0. Keeping
# this branch ahead of the template-existence check makes
# `make build TARGET=hetzner-cloud` Just Work on a fresh checkout.
case "$TARGET" in
  hetzner-cloud|vultr|linode)
    case "$TARGET" in
      hetzner-cloud) recipe_short="hetzner" ;;
      *)             recipe_short="$TARGET" ;;
    esac
    recipe="${repo_root}/cloud-init/${recipe_short}.yaml"
    if [[ ! -f "$recipe" ]]; then
      echo "error: no cloud-init recipe for TARGET=${TARGET}" >&2
      echo "  expected: ${recipe}" >&2
      exit 64
    fi
    cat <<EOF
info: ${TARGET} is a cloud-init recipe target — there is no local image
      build for this provider. Copy the YAML below into your provider's
      User-Data / Cloud Config / Startup Script field at instance
      creation. See cloud-init/README.md for sizing and release URLs.

----- ${recipe} -----
EOF
    cat "$recipe"
    echo "----- end -----"
    exit 0
    ;;
esac

# ---- Special-toolchain TARGETs (Lane B-special) ----
# macos-qemu wraps the qemu-aarch64 qcow2; proxmox/unraid/truenas are
# packaging-only. Handle these before the template-existence check.
case "$TARGET" in
  macos-qemu)
    echo "==> macos-qemu: re-dispatching to qemu-aarch64 build"
    # macos-qemu has no separate Packer template; the qcow2 the
    # macOS launcher boots IS the qemu-aarch64 image. Recursively
    # invoke this script so the staging path runs end-to-end.
    TARGET=qemu-aarch64 PROFILE="$PROFILE" CHANNEL="$CHANNEL" TIER="$TIER" AIRGAP="$AIRGAP" \
      bash "$0"
    src="${repo_root}/build/qemu-aarch64-${PROFILE}.qcow2"
    dst="${repo_root}/build/macos-qemu-${PROFILE}.qcow2"
    if [[ ! -f "$src" ]]; then
      # In degraded mode (no packer installed) the qemu-aarch64 build
      # exits 0 with a warning instead of producing a qcow2. Surface
      # the same "wiring is good, install packer to actually build"
      # message rather than a hard failure — keeps the dispatch
      # discoverable for contributors verifying the lane.
      echo "warn: qemu-aarch64 build did not produce ${src}" >&2
      echo "  if packer is missing, install it via 'make doctor' hint and retry." >&2
      echo "  the macos-qemu dispatch is wired correctly; the underlying qcow2" >&2
      echo "  is produced by packer/qemu-aarch64.pkr.hcl." >&2
      exit 0
    fi
    cp "$src" "$dst"
    echo "==> macos-qemu: ${dst}"
    echo
    echo "Next steps on macOS:"
    echo "  ./macos/run-utm.sh PROFILE=${PROFILE}        # UTM (recommended)"
    echo "  ./macos/run-orbstack.sh PROFILE=${PROFILE}   # OrbStack alternative"
    echo "  See macos/README.md for setup."
    exit 0
    ;;
  proxmox|unraid|truenas)
    echo "==> ${TARGET}: deployment template — no separate build"
    echo
    echo "These targets wrap an existing qcow2. Build qemu-x86_64 (typical"
    echo "Proxmox/Unraid/TrueNAS host) or qemu-aarch64 (rare arm64 host)"
    echo "first, then follow templates/${TARGET}/README.md:"
    echo
    echo "  make build TARGET=qemu-x86_64 PROFILE=${PROFILE}"
    echo "  make build TARGET=qemu-aarch64 PROFILE=${PROFILE}   # if your host is arm64"
    echo
    qcow_x86="${repo_root}/build/qemu-x86_64-${PROFILE}.qcow2"
    qcow_arm="${repo_root}/build/qemu-aarch64-${PROFILE}.qcow2"
    if [[ -f "$qcow_x86" ]]; then
      echo "  found: ${qcow_x86}"
    fi
    if [[ -f "$qcow_arm" ]]; then
      echo "  found: ${qcow_arm}"
    fi
    if [[ ! -f "$qcow_x86" && ! -f "$qcow_arm" ]]; then
      echo "  (no qcow2 found in build/ yet — run one of the make commands above)"
    fi
    echo
    echo "Then follow templates/${TARGET}/README.md for the install path."
    exit 0
    ;;
esac

# ---- fly-machines: OCI artefact via Buildah / Docker ----
# Built locally; no Packer template. We still pre-stage deputyctl + the
# profile manifest below (the Containerfile COPYs from build/staging),
# so fall through to the staging code and special-case the build at the
# bottom of the script.
if [[ "$TARGET" == "fly-machines" ]]; then
  : # handled at the bottom of this script (after staging is populated)
elif [[ ! -f "$template" ]]; then
  echo "error: no packer template for TARGET=${TARGET}" >&2
  echo "  expected: ${template}" >&2
  exit 64
fi

profile_src="${repo_root}/profiles/${PROFILE}.toml"
if [[ ! -f "$profile_src" ]]; then
  echo "error: profile manifest not found: ${profile_src}" >&2
  exit 65
fi

mkdir -p "${repo_root}/build" "${staging}/profiles" "${staging}/cloud-init"
if [[ "$DEPUTYOS_IMAGE_KIND" == "official" ]]; then
  rm -f "${staging}/agentless-development"
  rm -f "${repo_root}/build/${TARGET}-${PROFILE}.agentless"
else
  : >"${staging}/agentless-development"
  : >"${repo_root}/build/${TARGET}-${PROFILE}.agentless"
fi

# ---- Build the public guest binaries (control and first-boot wizard) ----
# These run INSIDE the Bookworm image, whose glibc is 2.36
# (debian-12-generic-amd64). A binary built on a newer-glibc build host —
# e.g. Ubuntu 25.04 / pop-os ship glibc 2.39 — links against symbol
# versions the image's 2.36 libc doesn't have, so the guest logs
# `/usr/local/bin/deputywizard: GLIBC_2.39 not found` and the service
# crash-loops. The TLS stack is pure rustls (no OpenSSL), so there's no
# native system dep tying us to the host — we can build the guest
# binaries in a debian:bookworm container (glibc 2.36) and the result
# runs on the image regardless of the build host's glibc.
#
# Strategy: when the build host's glibc is NEWER than the image's (2.36),
# build in the rust:1.91.1-bookworm container (pinned to the workspace
# channel in rust-toolchain.toml; cc present for ring's C/asm). CI
# runners whose glibc ≤ 2.36 build on the host directly — no Docker
# needed for the binary build there. Override either path with
# DEPUTYOS_GUEST_BIN_BUILD=host|container (e.g. to force a host build for
# debugging, or to force the container on a CI runner you want to keep
# host-independent). Lane B's wizard-baseline.yml copies
# build/staging/deputywizard and build/staging/deputyd into /usr/local/bin
# inside the guest; the role asserts the staged binaries exist, so a missing
# artefact here fails the bake loudly.
image_glibc="2.36"   # debian-12-generic (Bookworm) cloud image base
host_glibc="$(getconf GNU_LIBC_VERSION 2>/dev/null | awk '{print $2}')"

build_guest_bin_in_container() {
  local bin="$1"
  echo "==> building ${bin} in rust:1.91.1-bookworm (host glibc ${host_glibc:-?} > image glibc ${image_glibc})"
  # Run as the host uid:gid + a throwaway HOME/CARGO_HOME so cargo's cache
  # and the target dir land user-owned — otherwise the bind-mounted
  # target/bookworm fills with root-owned files the host user can't
  # `cargo clean` later. The cargo registry is a named volume (cached
  # across builds); CARGO_TARGET_DIR points at the bind-mounted repo so
  # the staged binary is readable from the host.
  docker run --rm \
    --user "$(id -u):$(id -g)" \
    -v "$repo_root:/work" -w /work \
    -v deputyos-cargo-registry:/usr/local/cargo/registry \
    -e CARGO_TARGET_DIR=/work/target/bookworm \
    -e CARGO_HOME=/tmp/cargo-home \
    -e HOME=/tmp \
    rust:1.91.1-bookworm \
    cargo build --release --bin "$bin"
  cp "${repo_root}/target/bookworm/release/${bin}" "${staging}/${bin}"
}

build_guest_bin_on_host() {
  local bin="$1"
  echo "==> building ${bin} release binary (host glibc ${host_glibc:-?} ≤ image glibc ${image_glibc})"
  (cd "$repo_root" && cargo build --release --bin "$bin")
  cp "${repo_root}/target/release/${bin}" "${staging}/${bin}"
}

# Decide the build path unless DEPUTYOS_GUEST_BIN_BUILD pins it. "container"
# when the host glibc is strictly greater than the image's; else "host".
guest_build_mode="${DEPUTYOS_GUEST_BIN_BUILD:-}"
if [[ -z "$guest_build_mode" ]]; then
  if [[ -n "$host_glibc" ]] \
     && [[ "$host_glibc" != "$image_glibc" ]] \
     && [[ "$(printf '%s\n%s\n' "$image_glibc" "$host_glibc" | sort -V | tail -1)" == "$host_glibc" ]]; then
    guest_build_mode="container"
  else
    guest_build_mode="host"
  fi
fi

for bin in deputyctl deputywizard; do
  if [[ "$guest_build_mode" == "container" ]]; then
    build_guest_bin_in_container "$bin"
  else
    build_guest_bin_on_host "$bin"
  fi
  chmod +x "${staging}/${bin}"
done

# The proprietary implementation is never built from this repository. Every
# official image must arrive through deputyos-core's explicit staging contract.
if [[ "$DEPUTYOS_IMAGE_KIND" == "official" ]]; then
  echo "==> staging proprietary resident agent from deputyos-core"
  rm -rf "${staging}/core"
  mkdir -p "${staging}/core"
  cp -a "${core_staging}/." "${staging}/core/"
  for required in deputyd deputy-terminal templates/deputyd.service.j2 \
    templates/deputy-terminal.service.j2 templates/deputyos-workloads.slice.j2 \
    templates/deputyos-reconcile.service.j2 templates/deputyos-reconcile.timer.j2 \
    templates/deputyos-tunnel.service.j2 templates/deputyos-command-poller.service.j2 \
    MANIFEST.sha256; do
    if [[ ! -e "${staging}/core/${required}" ]]; then
      echo "error: incomplete deputyos-core overlay: missing ${required}" >&2
      exit 66
    fi
  done
  echo "==> verifying deputyos-core payload integrity"
  (cd "${staging}/core" && sha256sum --quiet -c MANIFEST.sha256)
else
  rm -rf "${staging}/core"
  echo "warn: building an agentless development base; this output is not releasable" >&2
fi

# ---- Stage profile manifest ----
echo "==> staging profile manifest: ${PROFILE}"
cp "$profile_src" "${staging}/profiles/${PROFILE}.toml"

# ---- Stage limits.json (per-target) ----
# Lane A ships the qemu-aarch64 sample at deputyctl/etc/. Lane B owns
# per-hardware limits files at roles/deputyos/files/limits.<hw>.json
# (rpi4, arm64-generic, x86_64-mini-pc). For targets that have neither
# a Lane-A sample nor a Lane-B file, we synthesize limits.json inline
# below. Schema tracks deputyctl/src/limits.rs.
hw_limits_lane_a="${repo_root}/deputyctl/etc/limits.${TARGET}.json"
hw_limits_lane_b="${repo_root}/roles/deputyos/files/limits.${TARGET}.json"
if [[ -f "$hw_limits_lane_a" ]]; then
  echo "==> staging limits.json from ${hw_limits_lane_a}"
  cp "$hw_limits_lane_a" "${staging}/limits.json"
elif [[ -f "$hw_limits_lane_b" ]]; then
  echo "==> staging limits.json from ${hw_limits_lane_b}"
  cp "$hw_limits_lane_b" "${staging}/limits.json"
else
  case "$TARGET" in
    qemu-x86_64)
      echo "==> synthesizing staging limits.json for ${TARGET} (Lane A sample not present)"
      cat >"${staging}/limits.json" <<'EOF'
{
  "_comment": "Synthesized at bake time by scripts/build.sh for the qemu-x86_64 target. Mirrors limits.qemu-aarch64.json with x86_64-appropriate caps. Schema in deputyctl/src/limits.rs.",
  "target": "qemu-x86_64",
  "tier": "standard",
  "ram_mb": 4096,
  "ram_class": "standard",
  "storage_class": "ssd",
  "capabilities": {
    "local_llm": false,
    "voice_wake_word": false,
    "voice_tts": false,
    "clamav_daemon": true,
    "channels_heavy": ["telegram", "slack", "discord"],
    "channels_disabled_by_ram": ["whatsapp-cloud-webhook"]
  },
  "limitations": [
    {
      "id": "no-local-llm",
      "reason": "RAM tier 'standard' below local-LLM threshold (8GB)",
      "unblock": "x86_64-mini-pc 16GB or larger; or run model via cloud provider"
    },
    {
      "id": "no-voice",
      "reason": "qemu emulation makes wake-word and TTS unworkably slow",
      "unblock": "rpi5 8GB+ or x86_64-mini-pc"
    },
    {
      "id": "no-whatsapp-cloud-webhook",
      "reason": "WhatsApp Cloud webhook RSS exceeds standard tier headroom",
      "unblock": "upgrade to high-RAM tier"
    }
  ]
}
EOF
      ;;
    *)
      echo "warn: no limits.json found for TARGET=${TARGET} (looked at ${hw_limits_lane_a} and ${hw_limits_lane_b}); the role will use its embedded fallback" >&2
      rm -f "${staging}/limits.json"
      ;;
  esac
fi

# ---- Stage providers.json (provider catalogue; deputywizard reads it at boot) ----
# deputywizard's model::load_providers() resolves the provider catalogue via
# deputyctl::paths::providers_file(): DEPUTYOS_PROVIDERS_FILE env, then
# /etc/deputyos/providers.json, then deputyctl/etc/providers.json (a dev fallback
# relative to CWD — which is absent in the guest, so the wizard crash-loops
# with "loading providers catalogue: No such file or directory" and the
# first-boot UI never comes up). The role installs this staged copy to
# /etc/deputyos/providers.json. Single source of truth: deputyctl/etc/providers.json
# (see deputyctl/src/model.rs §"bake-time data").
deputyos_providers_src="${repo_root}/deputyctl/etc/providers.json"
if [[ -f "$deputyos_providers_src" ]]; then
  echo "==> staging providers.json from ${deputyos_providers_src}"
  cp "$deputyos_providers_src" "${staging}/providers.json"
else
  echo "warn: ${deputyos_providers_src} missing — deputywizard will crash-loop on boot (loading providers catalogue)" >&2
fi

# ---- Stage the API JWT public key (M9.6 AccountOwner remote-wizard auth) ----
# The wizard validates the account owner's JWT (issued by api.deputyos.com)
# against /etc/deputyos/api-pubkey.pem; wizard-baseline.yml copies it from
# build/staging/api-pubkey.pem (deputyos_api_pubkey_path). This is the *public*
# half of the API's JWT_PRIVATE_KEY/JWT_PUBLIC_KEY RSA keypair — non-secret, so
# it's fine to bake. It is NOT in the repo (it's operator-provisioned); source
# it from DEPUTYOS_API_PUBKEY_FILE, defaulting to the conventional
# ~/.config/deputyos/api-pubkey.pem. If absent, the bake skips it and the wizard
# falls back to Token mode (remote tunnel management unavailable) — a warning,
# not a hard failure, so dev/offline bakes still succeed.
deputyos_api_pubkey_src="${DEPUTYOS_API_PUBKEY_FILE:-${HOME}/.config/deputyos/api-pubkey.pem}"
if [[ -f "$deputyos_api_pubkey_src" ]]; then
  echo "==> staging api-pubkey.pem from ${deputyos_api_pubkey_src}"
  cp "$deputyos_api_pubkey_src" "${staging}/api-pubkey.pem"
else
  echo "warn: API pubkey not found at ${deputyos_api_pubkey_src} (set DEPUTYOS_API_PUBKEY_FILE) — /etc/deputyos/api-pubkey.pem will not be baked; remote-wizard falls back to Token mode" >&2
fi

# ---- Stage the stable-release minisign verification key ----
# deputyctl reads /etc/deputyos/pubkey.minisign before accepting a stable
# manifest. Release CI also embeds this key into desktop launcher binaries.
deputyos_release_pubkey_src="${DEPUTYOS_RELEASE_PUBKEY_FILE:-${HOME}/.config/deputyos/release-pubkey.minisign}"
if [[ -f "$deputyos_release_pubkey_src" ]]; then
  echo "==> staging release-pubkey.minisign from ${deputyos_release_pubkey_src}"
  cp "$deputyos_release_pubkey_src" "${staging}/release-pubkey.minisign"
else
  echo "warn: release pubkey not found at ${deputyos_release_pubkey_src} (set DEPUTYOS_RELEASE_PUBKEY_FILE) — stable update verification will be unavailable in this image" >&2
fi

# ---- Voice assets (Phase 7 Lane Voice / M6) ----
# Stage whisper.cpp + Piper into build/staging/voice/ for the voice-
# baseline role tasks to copy onto the appliance. The role is gated
# on deputyos_voice_enabled (default false) so this stage is a no-op
# for non-voice variants — but we run it unconditionally for any
# target NOT in the no-voice set, so a contributor flipping the
# wizard switch later finds the assets ready.
#
# Set DEPUTYOS_VOICE_OFFLINE=1 to skip the download (useful in
# air-gapped CI / sandboxed contributor laptops); the role tolerates
# missing assets and will log a clear "rerun the bake with assets
# staged" warning.
voice_no_target_set=("rpi4" "wsl2" "macos-qemu" "digitalocean" "oracle-arm-free" "hetzner-cloud" "vultr" "linode" "fly-machines")
voice_skip=0
for nv in "${voice_no_target_set[@]}"; do
  if [[ "$TARGET" == "$nv" ]]; then voice_skip=1; break; fi
done

if [[ "$voice_skip" == "1" ]]; then
  echo "==> voice-asset stage: skipping (TARGET=${TARGET} is in no-voice set)"
elif [[ "${DEPUTYOS_VOICE_OFFLINE:-0}" == "1" ]]; then
  echo "==> voice-asset stage: DEPUTYOS_VOICE_OFFLINE=1 — skipping download"
  mkdir -p "${staging}/voice"
  cat >"${staging}/voice/MANIFEST" <<'MANIFEST_EOF'
# voice assets not staged — set DEPUTYOS_VOICE_OFFLINE=0 (or unset) and
# rerun scripts/build.sh to fetch whisper.cpp + Piper. The role tolerates
# this gracefully — deputyos-voice-relay.service refuses to start until
# real binaries land at /opt/deputyos/voice/.
MANIFEST_EOF
else
  echo "==> voice-asset stage: downloading whisper.cpp + Piper into ${staging}/voice"
  mkdir -p "${staging}/voice"

  # Pinned upstream URLs + sha256. These are placeholders pending the
  # M6-rest "find a recent prebuilt with stable SHAs" sweep; whisper.cpp
  # does not currently ship prebuilt CLI binaries, so the contract is:
  #   * if a SHA256SUMS file ships in the upstream release, fetch + verify
  #   * else log a warning, write the partial MANIFEST, and continue
  # The role copies everything that lands; missing files trigger a
  # bake-time warn (not failure) so contributors without network can
  # still produce a non-voice image.
  voice_arch="$(uname -m)"
  case "$voice_arch" in
    aarch64|arm64) voice_arch_tag="aarch64" ;;
    x86_64|amd64)  voice_arch_tag="amd64" ;;
    *)             voice_arch_tag="unknown" ;;
  esac

  # Whisper ggml model (the most stable URL of the bunch — Hugging Face
  # serves the official ggerganov/whisper.cpp models with a redirect-
  # stable LFS URL).
  whisper_model_id="${DEPUTYOS_WHISPER_MODEL:-tiny.en}"
  whisper_model_url="https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-${whisper_model_id}.bin?download=true"
  whisper_model_dest="${staging}/voice/whisper-${whisper_model_id}.bin"

  # Piper voice + binary URLs (Piper's GitHub releases ship aarch64 +
  # amd64 prebuilts; voice models live on Hugging Face).
  piper_version="${DEPUTYOS_PIPER_VERSION:-2023.11.14-2}"
  piper_url="https://github.com/rhasspy/piper/releases/download/${piper_version}/piper_linux_${voice_arch_tag}.tar.gz"
  piper_voice_id="${DEPUTYOS_PIPER_VOICE:-en_US-amy-medium}"
  piper_voice_url="https://huggingface.co/rhasspy/piper-voices/resolve/main/en/en_US/amy/medium/${piper_voice_id}.onnx?download=true"
  piper_voice_meta_url="https://huggingface.co/rhasspy/piper-voices/resolve/main/en/en_US/amy/medium/${piper_voice_id}.onnx.json?download=true"

  fetch_with_warn() {
    local url="$1" dest="$2" label="$3"
    if [[ -f "$dest" ]]; then
      echo "    [cached] ${label}: ${dest}"
      return 0
    fi
    echo "    [fetch] ${label}: ${url}"
    if ! curl -fsSL --retry 3 --connect-timeout 10 -o "$dest.partial" "$url"; then
      echo "    [warn] failed to fetch ${label} (${url}); image will bake without voice assets" >&2
      rm -f "$dest.partial"
      return 1
    fi
    mv "$dest.partial" "$dest"
    return 0
  }

  voice_failures=0
  fetch_with_warn "$whisper_model_url" "$whisper_model_dest" "whisper-${whisper_model_id}" \
    || voice_failures=$((voice_failures + 1))

  if fetch_with_warn "$piper_url" "${staging}/voice/piper.tar.gz" "piper-${piper_version}-${voice_arch_tag}"; then
    # tar contents: piper/piper, piper/lib*, piper/espeak-ng-data/...
    tar -xzf "${staging}/voice/piper.tar.gz" -C "${staging}/voice" 2>/dev/null || true
    if [[ -f "${staging}/voice/piper/piper" ]]; then
      cp "${staging}/voice/piper/piper" "${staging}/voice/piper.bin"
      mv "${staging}/voice/piper.bin" "${staging}/voice/piper"
    fi
  else
    voice_failures=$((voice_failures + 1))
  fi

  fetch_with_warn "$piper_voice_url" "${staging}/voice/${piper_voice_id}.onnx" "piper-voice-${piper_voice_id}" \
    || voice_failures=$((voice_failures + 1))
  fetch_with_warn "$piper_voice_meta_url" "${staging}/voice/${piper_voice_id}.onnx.json" "piper-voice-${piper_voice_id}-meta" \
    || voice_failures=$((voice_failures + 1))

  # whisper-cli: build from source when no prebuilt is available.
  # whisper.cpp ships with a CMake build that produces a single binary.
  if [[ ! -f "${staging}/voice/whisper-cli" ]]; then
    whisper_version="${DEPUTYOS_WHISPER_VERSION:-v1.7.4}"
    echo "    [build] whisper.cpp ${whisper_version} (building from source)"
    whisper_tmp="$(mktemp -d "/tmp/deputyos-whisper-build.XXXXXX")"
    if git clone --depth 1 --branch "${whisper_version}" \
      https://github.com/ggerganov/whisper.cpp.git "$whisper_tmp" 2>/dev/null; then
      if (cd "$whisper_tmp" && cmake -B build -DCMAKE_BUILD_TYPE=Release >/dev/null 2>&1 && \
          cmake --build build --target whisper-cli -j"$(nproc 2>/dev/null || echo 2)" >/dev/null 2>&1); then
        cp "$whisper_tmp/build/bin/whisper-cli" "${staging}/voice/whisper-cli"
        echo "    [ok] whisper-cli built and staged"
      else
        echo "    [warn] whisper.cpp build failed; image will bake without whisper-cli" >&2
        voice_failures=$((voice_failures + 1))
      fi
    else
      echo "    [warn] whisper.cpp clone failed; network issues or repo unreachable" >&2
      voice_failures=$((voice_failures + 1))
    fi
    rm -rf "$whisper_tmp"
  else
    echo "    [cached] whisper-cli: ${staging}/voice/whisper-cli"
  fi

  # MANIFEST records every artefact (and its sha256, when present) so
  # the appliance can attest to what was baked.
  {
    echo "# deputyos voice assets — staged $(date -u +%FT%TZ)"
    echo "# target=${TARGET} arch=${voice_arch_tag}"
    echo "#"
    echo "# Pinned upstream sources:"
    echo "#   whisper-${whisper_model_id}.bin: ${whisper_model_url}"
    echo "#   piper.tar.gz: ${piper_url}"
    echo "#   ${piper_voice_id}.onnx: ${piper_voice_url}"
    echo "#   ${piper_voice_id}.onnx.json: ${piper_voice_meta_url}"
    echo "#"
    echo "# Per-file sha256:"
    if command -v sha256sum >/dev/null 2>&1; then
      (cd "${staging}/voice" && \
        find . -maxdepth 2 -type f ! -name MANIFEST ! -name '*.partial' \
          -exec sha256sum {} +) 2>/dev/null || true
    else
      echo "# (sha256sum not on PATH — install coreutils to populate)"
    fi
    echo "#"
    if [[ "$voice_failures" -gt 0 ]]; then
      echo "# WARN: ${voice_failures} asset(s) failed to stage. Set DEPUTYOS_VOICE_OFFLINE=1"
      echo "#       to suppress this warning, or rerun with network connectivity."
    fi
  } >"${staging}/voice/MANIFEST"

  if [[ "$voice_failures" -gt 0 && "${DEPUTYOS_VOICE_OFFLINE:-0}" != "1" ]]; then
    echo "warn: ${voice_failures} voice asset(s) missing — image will bake without voice." >&2
    echo "      set DEPUTYOS_VOICE_OFFLINE=1 to silence this warning." >&2
    # Non-fatal — the role tolerates missing assets per the spec.
  fi
fi

# ---- Airgap LLM assets (M4.5) ----
# When AIRGAP=1, download the tier-appropriate GGUFs from Hugging Face into
# build/staging/llm/. The Ansible role copies them from staging into the guest.
# Every model URL has a mandatory pinned SHA256 in llm-airgap.yml. Downloads
# and cached files are rejected unless they match it.
if [[ "$AIRGAP" == "1" ]]; then
  echo "==> airgap-llm stage: downloading GGUFs for tier=${TIER}"
  mkdir -p "${staging}/llm"

  airgap_vars="${repo_root}/roles/deputyos/vars/llm-airgap.yml"
  if [[ ! -f "$airgap_vars" ]]; then
    echo "error: missing llm-airgap.yml at ${airgap_vars}" >&2
    exit 64
  fi

  # Extract model blocks for the current tier. The YAML is shaped as:
  #   deputyos_airgap_llm_by_tier:
  #     lean|standard|rich:
  #       - id: "..."
  #         filename: "..."
  #         sha256: "..."
  #         url: "..."
  #         port: N
  #         default: true|false
  # We pull the tier block by finding the tier: line, then collecting all
  # subsequent "- id:" blocks until the next top-level key or EOF.
  declare -a airgap_model_ids=()
  declare -a airgap_model_files=()
  declare -a airgap_model_shas=()
  declare -a airgap_model_urls=()

  in_tier=0
  while IFS= read -r line; do
    # Detect tier start.
    if [[ "$line" =~ ^[[:space:]]*${TIER}:[[:space:]]*$ ]]; then
      in_tier=1
      continue
    fi
    # Stop at the next top-level key (no leading spaces) or next tier key.
    if [[ $in_tier -eq 1 ]]; then
      if [[ "$line" =~ ^[[:space:]]*[a-z] ]]; then
        break
      fi
      if [[ "$line" =~ ^[[:space:]]*-[[:space:]]*id:[[:space:]]*\"(.+)\"$ ]]; then
        airgap_model_ids+=("${BASH_REMATCH[1]}")
      fi
      if [[ "$line" =~ filename:[[:space:]]*\"(.+)\"$ ]]; then
        airgap_model_files+=("${BASH_REMATCH[1]}")
      fi
      if [[ "$line" =~ sha256:[[:space:]]*\"(.+)\"$ ]]; then
        airgap_model_shas+=("${BASH_REMATCH[1]}")
      fi
      if [[ "$line" =~ url:[[:space:]]*\"(.+)\"$ ]]; then
        airgap_model_urls+=("${BASH_REMATCH[1]}")
      fi
    fi
  done <"$airgap_vars"

  if [[ ${#airgap_model_files[@]} -eq 0 ]]; then
    echo "error: no models defined for tier=${TIER} in ${airgap_vars}" >&2
    exit 64
  fi

  airgap_failures=0
  for i in "${!airgap_model_files[@]}"; do
    id="${airgap_model_ids[$i]:-unknown}"
    file="${airgap_model_files[$i]}"
    sha="${airgap_model_shas[$i]:-}"
    url="${airgap_model_urls[$i]}"
    dest="${staging}/llm/${file}"

    if [[ ! "$sha" =~ ^[0-9a-f]{64}$ ]]; then
      echo "error: ${id}: missing or invalid pinned SHA256 in ${airgap_vars}" >&2
      exit 65
    fi

    if [[ -f "$dest" ]]; then
      got_sha=$(sha256sum "$dest" | awk '{print $1}')
      if [[ "$got_sha" == "$sha" ]]; then
        echo "  [cached+ok] ${id}: ${dest}"
        continue
      else
        echo "  [cached:stale] ${id}: SHA mismatch; re-fetching"
        rm -f "$dest"
      fi
    fi

    if [[ -z "$url" ]]; then
      echo "  [warn] ${id}: no url; skipping" >&2
      airgap_failures=$((airgap_failures + 1))
      continue
    fi

    echo "  [fetch] ${id}: ${url}"
    if ! curl -fsSL --retry 3 --connect-timeout 30 --max-time 1800 \
         -o "${dest}.partial" "$url"; then
      echo "  [warn] ${id}: download failed; image will bake without this model" >&2
      rm -f "${dest}.partial"
      airgap_failures=$((airgap_failures + 1))
      continue
    fi

    # Verify SHA256 after download.
    got_sha=$(sha256sum "${dest}.partial" | awk '{print $1}')
    if [[ "$got_sha" != "$sha" ]]; then
      echo "  [warn] ${id}: SHA256 mismatch — expected ${sha}, got ${got_sha}" >&2
      rm -f "${dest}.partial"
      airgap_failures=$((airgap_failures + 1))
      continue
    fi
    echo "  [sha256-ok] ${id}: ${sha}"

    mv "${dest}.partial" "$dest"
  done

  # MANIFEST for the baked models.
  {
    echo "# deputyos airgap LLM assets — staged $(date -u +%FT%TZ)"
    echo "# target=${TARGET} tier=${TIER}"
    echo "#"
    echo "# Per-file sha256:"
    if command -v sha256sum >/dev/null 2>&1; then
      (cd "${staging}/llm" && \
        find . -maxdepth 1 -type f ! -name MANIFEST ! -name '*.partial' \
          -exec sha256sum {} +) 2>/dev/null || true
    fi
    echo "#"
    if [[ "$airgap_failures" -gt 0 ]]; then
      echo "# WARN: ${airgap_failures} model(s) failed to stage. Image may bake"
      echo "#       without some airgap models. Rerun with network connectivity."
    fi
  } >"${staging}/llm/MANIFEST"

  if [[ "$airgap_failures" -gt 0 ]]; then
    echo "warn: ${airgap_failures} airgap model(s) failed to stage — image will be incomplete." >&2
    echo "      rerun with network connectivity to populate the cache." >&2
  fi
fi

# ---- Airgap apt-mirror staging (M4.5) ----
# When AIRGAP=1, populate a Debian package mirror for offline apt operations.
# The Ansible airgap-baseline.yml configures apt sources to point at
# file:///opt/deputyos/airgap/apt-mirror/ — this step stages the packages
# on the host side so Packer can bake them in.
if [[ "$AIRGAP" == "1" ]]; then
  mirror_dest="${staging}/airgap/apt-mirror"
  echo "==> airgap apt-mirror: staging Debian packages for offline apt"
  echo "    [note] full apt-mirror population requires debmirror or apt-mirror"
  echo "           tooling on the build host. For now, the Ansible role will"
  echo "           configure apt sources pointing at file:// — the package pool"
  echo "           must be staged externally or baked as a separate layer."
  mkdir -p "${mirror_dest}/dists" "${mirror_dest}/pool"
  # debmirror --arch=arm64,amd64 --dist=bookworm,bookworm-updates \
  #   --section=main,contrib,non-free-firmware \
  #   --host=deb.debian.org --root=debian \
  #   --method=http --progress --nosource \
  #   "${mirror_dest}"
fi

# ---- QEMU targets: generate cloud-init seed + throwaway SSH key ----
# The seed is target-agnostic (same user-data, same SSH key) — every
# target that uses the HashiCorp `qemu` builder boots from a Debian
# cloud image and uses cloud-init's NoCloud datasource. The Packer
# template points -drive at the seed ISO regardless of arch.
# x86_64-mini-pc uses the same QEMU builder (just with a qcow2->raw->xz
# post-processor) so it needs the same cloud-init seed.
# rpi4 / arm64-generic use packer-builder-arm (chroot-based, no SSH),
# so they don't need the seed.
case "$TARGET" in
  qemu-aarch64|qemu-x86_64|x86_64-mini-pc|oracle-arm-free)
    ssh_key="${staging}/ssh-key"
    if [[ ! -f "$ssh_key" ]]; then
      echo "==> generating throwaway SSH key for Packer build session"
      ssh-keygen -t ed25519 -N "" -f "$ssh_key" -C "deputyos-packer" >/dev/null
    fi
    pubkey="$(cat "${ssh_key}.pub")"

    cat >"${staging}/cloud-init/meta-data" <<EOF
instance-id: deputyos-packer
local-hostname: deputyos-build
EOF

    cat >"${staging}/cloud-init/user-data" <<EOF
#cloud-config
# Throwaway provisioning user injected for Packer's SSH session.
users:
  - name: deputyos-build
    sudo: ALL=(ALL) NOPASSWD:ALL
    shell: /bin/bash
    ssh_authorized_keys:
      - ${pubkey}
ssh_pwauth: false
package_update: false
runcmd:
  - [systemctl, enable, --now, qemu-guest-agent]
EOF
    ;;
esac

# ---- fly-machines: build the OCI artefact ----
# Buildah is preferred (rootless, daemonless); fall back to Docker if
# Buildah isn't on PATH. Either tool produces an OCI-compatible image
# that `flyctl deploy` can push to Fly's registry. The image stays in
# the host's local image store; we don't auto-push.
if [[ "$TARGET" == "fly-machines" ]]; then
  containerfile="${repo_root}/fly/Containerfile"
  if [[ ! -f "$containerfile" ]]; then
    echo "error: missing fly/Containerfile" >&2
    exit 64
  fi
  image_tag="deputyos-${PROFILE}:${CHANNEL}"
  cd "$repo_root"
  if command -v buildah >/dev/null 2>&1; then
    echo "==> building OCI artefact with buildah: ${image_tag}"
    exec buildah build \
      --build-arg "DEPUTYOS_PROFILE=${PROFILE}" \
      --build-arg "DEPUTYOS_CHANNEL=${CHANNEL}" \
      --build-arg "DEPUTYOS_TIER=${TIER}" \
      -t "$image_tag" \
      -f "$containerfile" \
      .
  elif command -v docker >/dev/null 2>&1; then
    echo "==> building OCI artefact with docker: ${image_tag}"
    exec docker build \
      --build-arg "DEPUTYOS_PROFILE=${PROFILE}" \
      --build-arg "DEPUTYOS_CHANNEL=${CHANNEL}" \
      --build-arg "DEPUTYOS_TIER=${TIER}" \
      -t "$image_tag" \
      -f "$containerfile" \
      .
  else
    echo "error: fly-machines build needs buildah or docker on PATH" >&2
    echo "  install with: sudo apt install buildah   # or: install Docker Desktop" >&2
    echo "  staged artefacts left in ${staging}" >&2
    exit 69
  fi
fi

# ---- DigitalOcean: require API token before invoking packer ----
# `packer validate -syntax-only` works without the token, but a real
# build needs the API. Fail loudly and early with the exact env-var name.
if [[ "$TARGET" == "digitalocean" && -z "${DIGITALOCEAN_TOKEN:-}" ]]; then
  echo "error: digitalocean build needs DIGITALOCEAN_TOKEN env var (DO API token)" >&2
  echo "  generate one at: https://cloud.digitalocean.com/account/api/tokens" >&2
  echo "  then: export DIGITALOCEAN_TOKEN=dop_v1_..." >&2
  exit 78
fi

if ! command -v packer >/dev/null 2>&1; then
  echo "warn: packer not installed; staged artefacts in ${staging} but cannot build" >&2
  echo "  run \`make doctor\` for the install hint" >&2
  exit 0
fi

cd "$repo_root"

# `packer init` once per template — idempotent. Validate before build.
packer init "$template"

# DigitalOcean's API token is the only target-specific extra var. Build
# the var-list piecewise so packer's CLI doesn't see an empty -var when
# the token isn't relevant.
extra_vars=()
if [[ "$TARGET" == "digitalocean" ]]; then
  extra_vars+=(-var "do_token=${DIGITALOCEAN_TOKEN}")
fi

# Ansible role resolution: Packer's ansible provisioner runs ansible-playbook
# with cwd = the template dir (packer/), so ansible's default `./roles` lookup
# resolves to packer/roles (missing) instead of the repo-root roles/ where the
# `deputyos` role lives. Pin ANSIBLE_ROLES_PATH to the repo-root roles/ so the
# role is found regardless of the provisioner's cwd. Inherited by the
# ansible-playbook subprocess Packer spawns.
export ANSIBLE_ROLES_PATH="${repo_root}/roles"

# Clear Packer's output dir BEFORE both `packer validate` and `packer
# build`. Packer's QEMU builder errors "Output directory 'build/packer-
# <target>' already exists. It must not exist." when its output_directory
# lingers from a prior bake — and this check fires during `packer validate`
# too (not just build), so cleaning must happen before validate or build.sh
# exits under `set -e` at the validate step and never reaches the build. On
# Packer 1.15.x `packer build -force` does NOT override this check for a
# custom output_directory (it only handles the default artifact name), so
# the flag alone isn't enough. Re-baking is the normal case here (this
# script always re-stages the guest binaries first), so a stale output dir
# must be replaced, never preserved. `rm -rf` makes the bake idempotent;
# -force is kept on the build as belt-and-suspenders for other overwrites.
packer_out="${repo_root}/build/packer-${TARGET}"
if [[ -d "$packer_out" ]]; then
  echo "==> clearing stale Packer output dir: ${packer_out}"
  rm -rf "$packer_out"
fi

packer validate \
  -var "profile=${PROFILE}" \
  -var "channel=${CHANNEL}" \
  -var "tier=${TIER}" \
  -var "airgap=${AIRGAP}" \
  -var "deputyos_staging_dir=${staging}" \
  "${extra_vars[@]}" \
  "$template"

exec packer build \
  -force \
  -var "profile=${PROFILE}" \
  -var "channel=${CHANNEL}" \
  -var "tier=${TIER}" \
  -var "airgap=${AIRGAP}" \
  -var "deputyos_staging_dir=${staging}" \
  "${extra_vars[@]}" \
  "$template"
