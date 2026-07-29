#!/usr/bin/env bash
# test/smoke/qemu-aarch64.sh — boot build/qemu-aarch64-<profile>.qcow2 in
# headless QEMU, SSH in, and run the assertion ladder from
# docs/03-image-builds.md §"QEMU smoke test (the gate)".
#
# Per-target script: handles UEFI firmware + qemu-system-aarch64 launch.
# All target-agnostic logic (cloud-init seed, SSH wait, assertions,
# cleanup, summary) lives in test/smoke/_common.sh.
#
# SMOKE_LEVEL controls assertions: scaffold | m1 | full.

set -euo pipefail

SMOKE_LEVEL="${SMOKE_LEVEL:-scaffold}"
TARGET="qemu-aarch64"
PROFILE="${PROFILE:-openclaw}"
repo_root="$(cd "$(dirname "$0")/../.." && pwd)"
artefact="${repo_root}/build/${TARGET}-${PROFILE}.qcow2"
cache_dir="${repo_root}/test/smoke/.cache"
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

if ! command -v qemu-system-aarch64 >/dev/null 2>&1; then
  echo "error: qemu-system-aarch64 not installed (see \`make doctor\`)" >&2
  exit 1
fi

# ---- generate cloud-init seed ----
smoke_generate_seed "$ssh_key"

# ---- pick UEFI firmware ----

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

# ---- launch ----

trap smoke_cleanup EXIT

qemu-system-aarch64 \
  -M virt -cpu cortex-a72 -smp 2 -m 2048 \
  -bios "$firmware" \
  -nographic -serial "file:${serial_log}" -monitor none \
  -drive "if=virtio,file=${artefact},format=qcow2" \
  -drive "if=virtio,file=${seed_iso},format=raw,readonly=on" \
  -chardev "socket,path=${qga_socket},server=on,wait=off,id=qga0" \
  -device virtio-serial-device \
  -device virtserialport,chardev=qga0,name=org.qemu.guest_agent.0 \
  -netdev "user,id=net0,hostfwd=tcp::2222-:22,hostfwd=tcp::8088-:8088" \
  -device virtio-net-device,netdev=net0 \
  -daemonize -pidfile "$qemu_pidfile"

# ---- assertions ----
smoke_wait_for_ssh
smoke_run_assertions
