#!/usr/bin/env bash
# test/smoke/qemu-x86_64.sh — boot build/qemu-x86_64-<profile>.qcow2 in
# headless QEMU, SSH in, and run the same assertion ladder used by the
# aarch64 smoke. All target-agnostic logic lives in _common.sh.
#
# x86_64 differences vs aarch64:
#   - q35 machine + OVMF UEFI firmware (vs virt + AAVMF)
#   - virtio-net-pci (vs virtio-net-device on the arm 'virt' machine)
#   - opportunistic KVM via accel=kvm:tcg
#
# SMOKE_LEVEL controls assertions: scaffold | m1 | full.

set -euo pipefail

SMOKE_LEVEL="${SMOKE_LEVEL:-scaffold}"
TARGET="qemu-x86_64"
PROFILE="${PROFILE:-openclaw}"
repo_root="$(cd "$(dirname "$0")/../.." && pwd)"
artefact="${repo_root}/build/${TARGET}-${PROFILE}.qcow2"
cache_dir="${repo_root}/test/smoke/.cache-x86_64"
seed_iso="${cache_dir}/seed.iso"
ssh_key="${cache_dir}/id_ed25519"
qemu_pidfile="${cache_dir}/qemu.pid"
qga_socket="${cache_dir}/qga.sock"
serial_log="${cache_dir}/serial.log"

mkdir -p "$cache_dir"

# Export for _common.sh.
export TARGET PROFILE SMOKE_LEVEL repo_root artefact cache_dir seed_iso \
       ssh_key qemu_pidfile qga_socket serial_log

# shellcheck source=test/smoke/_common.sh
source "$(dirname "$0")/_common.sh"

# ---- pre-flight ----

if [[ ! -f "$artefact" ]]; then
  echo "info: artefact missing; building" >&2
  (cd "$repo_root" && make build "TARGET=${TARGET}" "PROFILE=${PROFILE}")
fi

if [[ ! -f "$artefact" ]]; then
  echo "error: build did not produce ${artefact}" >&2
  exit 1
fi

if ! command -v qemu-system-x86_64 >/dev/null 2>&1; then
  echo "error: qemu-system-x86_64 not installed (see \`make doctor\`)" >&2
  exit 1
fi

# ---- generate cloud-init seed ----
smoke_generate_seed "$ssh_key"

# ---- pick UEFI firmware ----
# Distros split the OVMF blob differently:
#   Debian/Ubuntu (legacy single-file): /usr/share/ovmf/OVMF.fd
#   Debian/Ubuntu (4M-split):           /usr/share/OVMF/OVMF_CODE_4M.fd
#   Arch / generic:                     /usr/share/qemu/OVMF.fd
#   Fedora:                             /usr/share/edk2/ovmf/OVMF_CODE.fd
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

# ---- launch ----

trap smoke_cleanup EXIT

# accel=kvm:tcg falls through to tcg if /dev/kvm is unavailable.
qemu-system-x86_64 \
  -machine "q35,accel=kvm:tcg" -cpu max -smp 2 -m 2048 \
  -bios "$firmware" \
  -nographic -serial "file:${serial_log}" -monitor none \
  -drive "if=virtio,file=${artefact},format=qcow2" \
  -drive "if=virtio,file=${seed_iso},format=raw,readonly=on" \
  -chardev "socket,path=${qga_socket},server=on,wait=off,id=qga0" \
  -device virtio-serial-pci \
  -device virtserialport,chardev=qga0,name=org.qemu.guest_agent.0 \
  -netdev "user,id=net0,hostfwd=tcp::2222-:22,hostfwd=tcp::8088-:8088" \
  -device virtio-net-pci,netdev=net0 \
  -daemonize -pidfile "$qemu_pidfile"

# ---- assertions ----
smoke_wait_for_ssh
smoke_run_assertions
