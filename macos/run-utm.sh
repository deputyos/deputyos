#!/usr/bin/env bash
# macos/run-utm.sh — boot the deputyOS qemu-aarch64 qcow2 inside UTM on
# macOS. Used by `make try TARGET=macos-qemu` and by users who built
# locally with `make build TARGET=macos-qemu`.
#
# UTM is the recommended runner on Apple Silicon — it uses Hypervisor
# Framework for KVM-equivalent acceleration on M1/M2/M3/M4. On Intel
# Macs UTM falls back to TCG (slow but functional). For OrbStack
# users see run-orbstack.sh.

set -euo pipefail

PROFILE="${PROFILE:-openclaw}"
MEMORY_MB="${MEMORY_MB:-2048}"
HOSTPORT_WIZARD="${HOSTPORT_WIZARD:-8088}"
HOSTPORT_SSH="${HOSTPORT_SSH:-2222}"

repo_root="$(cd "$(dirname "$0")/.." && pwd)"
qcow2="${repo_root}/build/macos-qemu-${PROFILE}.qcow2"

if [[ "$(uname -s)" != "Darwin" ]]; then
  echo "warn: run-utm.sh is for macOS hosts; you're on $(uname -s)" >&2
  echo "  on Linux: use run-utm.sh's qemu fallback path or scripts/try.sh TARGET=qemu-aarch64" >&2
fi

if [[ ! -f "$qcow2" ]]; then
  echo "error: artefact not found: ${qcow2}" >&2
  echo "  build it first: make build TARGET=macos-qemu PROFILE=${PROFILE}" >&2
  exit 1
fi

# ---- preferred: UTM CLI (utmctl) ----
# UTM ships a lifecycle CLI plus an AppleScript creation API. `utmctl`
# deliberately has no create command, so creation uses the documented
# `make new virtual machine` record and lifecycle uses utmctl.
if command -v utmctl >/dev/null 2>&1; then
  echo "==> macos-qemu: using utmctl"
  vmname="deputyos-${PROFILE}"

  # Idempotent: if a VM by this name already exists, reuse it.
  if utmctl list 2>/dev/null | grep -qE "(^|[[:space:]])${vmname}([[:space:]]|$)"; then
    echo "info: VM '${vmname}' already exists; starting it"
  else
    echo "==> creating UTM VM '${vmname}'"
    osascript - "${vmname}" "${qcow2}" "${MEMORY_MB}" <<'APPLESCRIPT'
on run argv
  set vmName to item 1 of argv
  set diskPath to item 2 of argv
  set memoryMb to (item 3 of argv) as integer
  tell application "UTM"
    set diskImage to POSIX file diskPath
    make new virtual machine with properties {backend:qemu, configuration:{name:vmName, architecture:"aarch64", memory:memoryMb, cpu cores:2, hypervisor:true, uefi:true, drives:{{interface:VirtIO, source:diskImage}}}}
  end tell
end run
APPLESCRIPT
  fi

  echo "==> starting VM"
  utmctl start "${vmname}"
  guest_ip="$(utmctl ip-address "${vmname}" 2>/dev/null | head -1 || true)"
  echo
  if [[ -n "$guest_ip" ]]; then
    echo "==> wizard: open http://${guest_ip}:8088"
    echo "==> ssh:    ssh agent@${guest_ip}"
  else
    echo "==> wizard: available through the deputyOS outbound tunnel once boot completes"
  fi
  echo
  echo "UTM shared networking gives each VM its own guest IP; no localhost"
  echo "port remap is needed. External access uses the authenticated tunnel."
  exit 0
fi

# ---- fallback: open the qcow2 directly in UTM.app ----
if [[ -d "/Applications/UTM.app" ]]; then
  echo "info: utmctl not on PATH; opening qcow2 in UTM.app GUI"
  echo "  brew install --cask utm   # to get utmctl alongside the app"
  open -a UTM "${qcow2}"
  echo
  echo "==> next steps in UTM:"
  echo "  1. Click 'Create a New Virtual Machine' -> Virtualize -> Linux"
  echo "  2. Skip ISO image; use existing drive image: ${qcow2}"
  echo "  3. Memory: ${MEMORY_MB} MB; Storage: existing"
  echo "  4. Network: Shared; add Port Forward TCP ${HOSTPORT_WIZARD}->8088"
  exit 0
fi

# ---- last resort: qemu-system-aarch64 directly ----
echo "warn: UTM not installed; falling back to qemu-system-aarch64" >&2
echo "  install UTM: brew install --cask utm" >&2
if ! command -v qemu-system-aarch64 >/dev/null 2>&1; then
  echo "error: qemu-system-aarch64 not installed either" >&2
  echo "  brew install qemu" >&2
  exit 1
fi

firmware=""
for cand in /opt/homebrew/share/qemu/edk2-aarch64-code.fd \
            /usr/local/share/qemu/edk2-aarch64-code.fd \
            /usr/share/AAVMF/AAVMF_CODE.fd; do
  if [[ -f "$cand" ]]; then
    firmware="$cand"
    break
  fi
done

if [[ -z "$firmware" ]]; then
  echo "error: no aarch64 UEFI firmware found" >&2
  echo "  brew install qemu  # ships edk2-aarch64-code.fd" >&2
  exit 1
fi

# HVF on Apple Silicon; TCG on Intel.
accel="hvf"
if [[ "$(uname -m)" != "arm64" ]]; then
  accel="tcg"
fi

exec qemu-system-aarch64 \
  -M virt -accel "${accel}" -cpu host -smp 2 -m "${MEMORY_MB}" \
  -bios "$firmware" \
  -nographic \
  -drive "file=${qcow2},format=qcow2,if=virtio" \
  -netdev "user,id=net0,hostfwd=tcp::${HOSTPORT_WIZARD}-:8088,hostfwd=tcp::${HOSTPORT_SSH}-:22" \
  -device virtio-net-device,netdev=net0
