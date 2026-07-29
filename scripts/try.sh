#!/usr/bin/env bash
# scripts/try.sh — boot a built artefact in the host's preferred runner.
# Linux/WSL2: qemu-system-<arch>. macOS: prefers UTM if installed, falls
# back to qemu. The wizard port is forwarded to localhost:8088.

set -euo pipefail

TARGET="${TARGET:-qemu-aarch64}"
PROFILE="${PROFILE:-openclaw}"
repo_root="$(cd "$(dirname "$0")/.." && pwd)"

# ---- Special-toolchain targets short-circuit before the qcow2 check ----
# wsl2 has no qcow2 (its artefact is a tarball + Windows host); macos-qemu
# delegates to UTM/OrbStack via its own launchers.
case "$TARGET" in
  wsl2)
    echo "info: wsl2 try requires a Windows host."
    echo "  see wsl/README.md for the import path:"
    echo "    PowerShell> .\\Install-DeputyOS.ps1 -LocalTarball <path-to-tarball>"
    echo "  or, if you're already inside WSL2 and want to test the role's"
    echo "  shape under nspawn, see scripts/wsl2-build.sh."
    exit 0
    ;;
  macos-qemu)
    if [[ "$(uname -s)" != "Darwin" ]]; then
      echo "warn: macos-qemu try is intended for macOS hosts" >&2
      echo "  on Linux, use TARGET=qemu-aarch64 instead (same qcow2)." >&2
    fi
    if command -v utmctl >/dev/null 2>&1 || [[ -d "/Applications/UTM.app" ]]; then
      exec bash "${repo_root}/macos/run-utm.sh"
    elif command -v orbctl >/dev/null 2>&1 || command -v orb >/dev/null 2>&1; then
      exec bash "${repo_root}/macos/run-orbstack.sh"
    else
      echo "error: neither UTM nor OrbStack installed" >&2
      echo "  install UTM:     brew install --cask utm" >&2
      echo "  install OrbStack: brew install --cask orbstack" >&2
      exit 1
    fi
    ;;
esac

artefact="${repo_root}/build/${TARGET}-${PROFILE}.qcow2"

if [[ ! -f "$artefact" ]]; then
  echo "error: build artefact not found: ${artefact}" >&2
  echo "  run: make build TARGET=${TARGET} PROFILE=${PROFILE}" >&2
  exit 1
fi

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

case "$TARGET" in
  qemu-aarch64)
    case "$OS" in
      macos)
        if [[ -d "/Applications/UTM.app" ]]; then
          echo "info: UTM is the recommended runner on macOS (HVF acceleration)." >&2
          echo "  open ${artefact} in UTM, or press Ctrl-C and re-run with: brew install qemu" >&2
          open -a UTM "${artefact}"
          exit 0
        fi
        echo "info: UTM not installed; falling back to qemu-system-aarch64" >&2
        ;;
      wsl2)
        echo "info: WSL2 detected; running qemu-system-aarch64 (no KVM needed)" >&2
        ;;
    esac

    if ! command -v qemu-system-aarch64 >/dev/null 2>&1; then
      echo "error: qemu-system-aarch64 not installed (see \`make doctor\`)" >&2
      exit 1
    fi

    firmware=""
    for cand in /usr/share/AAVMF/AAVMF_CODE.fd \
                /usr/share/qemu-efi-aarch64/QEMU_EFI.fd \
                /usr/share/edk2/aarch64/QEMU_EFI.fd; do
      if [[ -f "$cand" ]]; then
        firmware="$cand"
        break
      fi
    done

    if [[ -z "$firmware" ]]; then
      echo "error: no AAVMF/EDK2 aarch64 UEFI firmware found" >&2
      echo "  install with: sudo apt install qemu-efi-aarch64" >&2
      exit 1
    fi

    exec qemu-system-aarch64 \
      -M virt -cpu cortex-a72 -smp 2 -m 2048 \
      -bios "$firmware" \
      -nographic \
      -drive "file=${artefact},format=qcow2,if=virtio" \
      -netdev "user,id=net0,hostfwd=tcp::8088-:8088,hostfwd=tcp::2222-:22" \
      -device virtio-net-device,netdev=net0
    ;;

  qemu-x86_64)
    case "$OS" in
      macos)
        if [[ -d "/Applications/UTM.app" ]]; then
          echo "info: UTM can run x86_64 qcow2 on macOS too (slow on Apple Silicon — TCG only)." >&2
          open -a UTM "${artefact}"
          exit 0
        fi
        echo "info: UTM not installed; falling back to qemu-system-x86_64" >&2
        ;;
      wsl2)
        echo "info: WSL2 detected; running qemu-system-x86_64 (KVM via /dev/kvm if exposed)" >&2
        ;;
    esac

    if ! command -v qemu-system-x86_64 >/dev/null 2>&1; then
      echo "error: qemu-system-x86_64 not installed (see \`make doctor\`)" >&2
      exit 1
    fi

    firmware=""
    for cand in /usr/share/ovmf/OVMF.fd \
                /usr/share/OVMF/OVMF_CODE.fd \
                /usr/share/OVMF/OVMF_CODE_4M.fd \
                /usr/share/qemu/OVMF.fd \
                /usr/share/edk2/ovmf/OVMF_CODE.fd; do
      if [[ -f "$cand" ]]; then
        firmware="$cand"
        break
      fi
    done

    if [[ -z "$firmware" ]]; then
      echo "error: no OVMF UEFI firmware found for x86_64" >&2
      echo "  install with: sudo apt install ovmf" >&2
      exit 1
    fi

    exec qemu-system-x86_64 \
      -machine "q35,accel=kvm:tcg" -cpu max -smp 2 -m 2048 \
      -bios "$firmware" \
      -nographic \
      -drive "file=${artefact},format=qcow2,if=virtio" \
      -netdev "user,id=net0,hostfwd=tcp::8088-:8088,hostfwd=tcp::2222-:22" \
      -device virtio-net-pci,netdev=net0
    ;;

  *)
    echo "error: try.sh does not yet support TARGET=${TARGET}" >&2
    echo "  supported: qemu-aarch64, qemu-x86_64" >&2
    exit 64
    ;;
esac
