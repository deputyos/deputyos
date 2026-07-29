#!/usr/bin/env bash
# test/smoke/rpi5.sh — boot the rpi5 .img.xz under qemu's `raspi3b`/virt
# emulation and run the same gate. Lane B finalises the right qemu machine
# type in M1; for now this script is a thin wrapper that delegates to the
# qemu-aarch64 harness against the rpi5 artefact.

set -euo pipefail

PROFILE="${PROFILE:-openclaw}"
repo_root="$(cd "$(dirname "$0")/../.." && pwd)"
artefact="${repo_root}/build/rpi5-${PROFILE}.img.xz"

if [[ ! -f "$artefact" ]]; then
  echo "info: artefact missing; building" >&2
  (cd "$repo_root" && make build TARGET=rpi5 "PROFILE=${PROFILE}")
fi

# TODO(M1-LaneB): boot the .img.xz directly. Until then, the rpi5 smoke
# delegates to the qemu-aarch64 path because the role under test is the
# same — only the variant tasks differ.
echo "info: rpi5 smoke currently delegates to qemu-aarch64 (M1 scaffold)" >&2
exec "${repo_root}/test/smoke/qemu-aarch64.sh"
