#!/usr/bin/env bash
# scripts/doctor.sh — host-tooling preflight for `make doctor`.
#
# Prints one missing dep per line with a host-OS-appropriate fix command.
# Exits nonzero if any required dep is missing, so `make doctor` fails
# loudly. Optional deps (packer, shellcheck) emit a warning but don't fail.

set -euo pipefail

REQUIRED=(rustup cargo ansible ansible-lint yamllint qemu-system-aarch64 qemu-system-x86_64 qemu-img xz ssh-keygen ssh)
OPTIONAL=(packer shellcheck minisign mkimage mtools qemu-user-static cloud-localds genisoimage xorriso \
          buildah docker flyctl doctl oci cloud-init \
          pwsh systemd-nspawn proot utmctl orb xmllint jq \
          cloudflared tailscale avahi-publish qrencode age \
          aplay arecord whisper-cli piper)
# Probed via file-existence rather than `command -v` (these are firmware
# blobs / packages, not commands).
OPTIONAL_FILES=(
  "ovmf:/usr/share/ovmf/OVMF.fd"
  "qemu-efi-aarch64:/usr/share/AAVMF/AAVMF_CODE.fd"
)

detect_os() {
  if [[ "$(uname -s)" == "Darwin" ]]; then
    echo "macos"
  elif grep -qiE "(microsoft|wsl)" /proc/version 2>/dev/null; then
    echo "wsl2"
  else
    echo "linux"
  fi
}

OS="$(detect_os)"

fix_hint() {
  local dep="$1"
  case "$OS:$dep" in
    macos:rustup|macos:cargo) echo "  install with: curl https://sh.rustup.rs -sSf | sh" ;;
    macos:packer)             echo "  install with: brew tap hashicorp/tap && brew install hashicorp/tap/packer" ;;
    # Specific macos:<dep> patterns must precede the macos:* catch-all
    # below; otherwise the catch-all silently swallows them.
    macos:buildah)            echo "  install with: brew install buildah   # (Linux preferred; rootless containers)" ;;
    macos:docker)             echo "  install with: brew install --cask docker   # or: brew install docker (CLI only)" ;;
    macos:flyctl)             echo "  install with: brew install flyctl   # only required to deploy fly-machines builds" ;;
    macos:doctl)              echo "  install with: brew install doctl   # only required to verify digitalocean snapshots locally" ;;
    macos:oci)                echo "  install with: brew install oci-cli   # only required to upload oracle-arm-free qcow2" ;;
    macos:cloud-init)         echo "  not installable on macOS; skip — used to schema-check cloud-init/*.yaml on Linux" ;;
    macos:pwsh)               echo "  install with: brew install --cask powershell   # only required to lint wsl/Install-DeputyOS.ps1" ;;
    macos:systemd-nspawn)     echo "  not available on macOS; skip — only used by scripts/wsl2-build.sh inside Linux/WSL2" ;;
    macos:proot)              echo "  install with: brew install proot   # unprivileged-user fallback for wsl2 rootfs bake" ;;
    macos:utmctl)             echo "  install with: brew install --cask utm   # macOS only; required for ./macos/run-utm.sh" ;;
    macos:orb)                echo "  install with: brew install --cask orbstack   # macOS only; alternative to UTM" ;;
    macos:cloudflared)        echo "  install with: brew install cloudflared   # only required for host-side tunnel dev/test" ;;
    macos:tailscale)          echo "  install with: brew install tailscale   # only required for host-side mesh dev/test" ;;
    macos:avahi-publish)      echo "  not available on macOS (Bonjour ships with the OS); skip — used to test mDNS announcements on Linux" ;;
    macos:qrencode)           echo "  install with: brew install qrencode   # host-side aid; deputywizard uses the qrcode crate at runtime" ;;
    macos:*)                  echo "  install with: brew install $dep" ;;
    wsl2:rustup|wsl2:cargo|linux:rustup|linux:cargo)
                              echo "  install with: curl https://sh.rustup.rs -sSf | sh" ;;
    *:packer)                 echo "  install via HashiCorp apt repo: see https://developer.hashicorp.com/packer/install" ;;
    *:qemu-system-aarch64)    echo "  install with: sudo apt install qemu-system-arm" ;;
    *:qemu-system-x86_64)     echo "  install with: sudo apt install qemu-system-x86" ;;
    *:qemu-user-static)       echo "  install with: sudo apt install qemu-user-static binfmt-support" ;;
    *:ovmf)                   echo "  install with: sudo apt install ovmf" ;;
    *:qemu-efi-aarch64)       echo "  install with: sudo apt install qemu-efi-aarch64" ;;
    *:mkimage)                echo "  install with: sudo apt install u-boot-tools" ;;
    *:mtools)                 echo "  install with: sudo apt install mtools dosfstools" ;;
    *:minisign)               echo "  install with: sudo apt install minisign" ;;
    *:cloud-localds)          echo "  install with: sudo apt install cloud-image-utils" ;;
    *:genisoimage)            echo "  install with: sudo apt install genisoimage" ;;
    *:xorriso)                echo "  install with: sudo apt install xorriso" ;;
    *:qemu-img)               echo "  install with: sudo apt install qemu-utils" ;;
    *:ssh-keygen|*:ssh)       echo "  install with: sudo apt install openssh-client" ;;
    *:ansible|*:ansible-lint|*:yamllint)
                              echo "  install with: pipx install $dep   # or: sudo apt install $dep" ;;
    *:buildah)                echo "  install with: sudo apt install buildah   # rootless OCI builds for fly-machines" ;;
    *:docker)                 echo "  install with: see https://docs.docker.com/engine/install/   # alternative to buildah for fly-machines" ;;
    *:flyctl)                 echo "  install with: curl -L https://fly.io/install.sh | sh   # only required to deploy fly-machines builds" ;;
    *:doctl)                  echo "  install with: see https://docs.digitalocean.com/reference/doctl/how-to/install/   # only for DO snapshot verify" ;;
    *:oci)                    echo "  install with: bash -c 'curl -L https://raw.githubusercontent.com/oracle/oci-cli/master/scripts/install/install.sh | bash'   # only for oracle import" ;;
    *:cloud-init)             echo "  install with: sudo apt install cloud-init   # only required to run 'cloud-init schema' lint on cloud-init/*.yaml" ;;
    *:pwsh)                   echo "  install with: see https://learn.microsoft.com/en-us/powershell/scaling-and-performance/installing-powershell-on-linux   # only for PS1 lint" ;;
    *:systemd-nspawn)         echo "  install with: sudo apt install systemd-container   # for wsl2 rootfs bake (recommended over proot)" ;;
    *:proot)                  echo "  install with: sudo apt install proot   # unprivileged-user fallback for wsl2 rootfs bake (alt to systemd-nspawn)" ;;
    *:utmctl)                 echo "  utmctl is macOS-only; skip on Linux/WSL2" ;;
    *:orb)                    echo "  orb (OrbStack) is macOS-only; skip on Linux/WSL2" ;;
    *:xmllint)                echo "  install with: sudo apt install libxml2-utils   # used to validate templates/unraid/deputyos.xml" ;;
    *:jq)                     echo "  install with: sudo apt install jq   # used to validate templates/truenas/deputyos.json" ;;
    *:cloudflared)            echo "  install with: see https://pkg.cloudflare.com/   # only required for host-side tunnel dev/test (image bakes its own copy)" ;;
    *:tailscale)              echo "  install with: see https://pkgs.tailscale.com/   # only required for host-side mesh dev/test (image bakes its own copy)" ;;
    *:avahi-publish)          echo "  install with: sudo apt install avahi-utils   # used to verify mDNS announcements during dev" ;;
    *:qrencode)               echo "  install with: sudo apt install qrencode   # host-side aid; deputywizard uses the qrcode crate at runtime" ;;
    *:age)                    echo '  install with: sudo apt install age   # required for deputyctl backup --to cloud / restore --from cloud (client-side age encryption)' ;;
    *:aplay|*:arecord)        echo "  install with: sudo apt install alsa-utils   # host-side voice dev; the appliance bakes its own copy (Phase 7 Lane Voice)" ;;
    *:whisper-cli)            echo "  optional — host-side STT smoke test for voice-relay.sh; the appliance gets a pinned binary via scripts/build.sh. Build from https://github.com/ggerganov/whisper.cpp." ;;
    *:piper)                  echo "  optional — host-side TTS smoke test; install via 'pip install piper-tts' or download a release from https://github.com/rhasspy/piper" ;;
    *)                        echo "  install with: sudo apt install $dep" ;;
  esac
}

missing_required=()
missing_optional=()

for dep in "${REQUIRED[@]}"; do
  if ! command -v "$dep" >/dev/null 2>&1; then
    missing_required+=("$dep")
  fi
done

for dep in "${OPTIONAL[@]}"; do
  if ! command -v "$dep" >/dev/null 2>&1; then
    missing_optional+=("$dep")
  fi
done

# File-probe optional deps (firmware blobs).
for entry in "${OPTIONAL_FILES[@]}"; do
  pkg="${entry%%:*}"
  path="${entry#*:}"
  if [[ ! -f "$path" ]]; then
    missing_optional+=("$pkg")
  fi
done

# Print + exit only when run directly, not when sourced (scripts/
# desktop-local-preflight.sh sources this for detect_os + fix_hint).
if [[ "${BASH_SOURCE[0]}" != "${0}" ]]; then
  # shellcheck disable=SC2317 # reachable when this file is sourced
  return 0 2>/dev/null || exit 0
fi

HOST_ARCH="$(uname -m)"
echo "deputyOS doctor — host: $OS ($HOST_ARCH)"
echo
case "$HOST_ARCH" in
  x86_64|amd64)
    echo "  note: on x86_64 host, qemu-aarch64 builds need qemu-user-static for binfmt;"
    echo "        qemu-x86_64 builds run natively (KVM if /dev/kvm is exposed, TCG otherwise)."
    ;;
  aarch64|arm64)
    echo "  note: on aarch64 host, qemu-aarch64 runs natively (KVM/HVF);"
    echo "        qemu-x86_64 falls back to TCG (slow but works)."
    ;;
esac
echo

if (( ${#missing_required[@]} == 0 )); then
  echo "  [ok] all required tools present"
else
  echo "  [missing] required:"
  for d in "${missing_required[@]}"; do
    echo "    - $d"
    fix_hint "$d"
  done
fi

if (( ${#missing_optional[@]} > 0 )); then
  echo
  echo "  [warn] optional (lint/build steps will skip):"
  for d in "${missing_optional[@]}"; do
    echo "    - $d"
    fix_hint "$d"
  done
fi

if (( ${#missing_required[@]} > 0 )); then
  exit 1
fi
