#!/usr/bin/env bash
# macos/run-orbstack.sh — boot the deputyOS qemu-aarch64 qcow2 inside
# OrbStack on macOS. OrbStack is the lightweight alternative to UTM
# for users who prefer Docker-Desktop-shape Linux VMs; it supports
# qcow2 images and has built-in port forwarding.

set -euo pipefail

PROFILE="${PROFILE:-openclaw}"
MEMORY_MB="${MEMORY_MB:-2048}"
HOSTPORT_WIZARD="${HOSTPORT_WIZARD:-8088}"
HOSTPORT_SSH="${HOSTPORT_SSH:-2222}"

repo_root="$(cd "$(dirname "$0")/.." && pwd)"
qcow2="${repo_root}/build/macos-qemu-${PROFILE}.qcow2"

if [[ "$(uname -s)" != "Darwin" ]]; then
  echo "warn: run-orbstack.sh is for macOS hosts; you're on $(uname -s)" >&2
fi

if [[ ! -f "$qcow2" ]]; then
  echo "error: artefact not found: ${qcow2}" >&2
  echo "  build it first: make build TARGET=macos-qemu PROFILE=${PROFILE}" >&2
  exit 1
fi

if ! command -v orbctl >/dev/null 2>&1 && ! command -v orb >/dev/null 2>&1; then
  echo "error: OrbStack not installed (orbctl/orb not on PATH)" >&2
  echo "  install: brew install --cask orbstack" >&2
  echo "  or use UTM:           ./macos/run-utm.sh PROFILE=${PROFILE}" >&2
  exit 1
fi

# OrbStack's CLI is `orb` (with `orbctl` as an alias on some versions).
orb_cli="$(command -v orb || command -v orbctl)"

vmname="deputyos-${PROFILE}"

# Idempotent: reuse an existing VM if one's already there.
if "$orb_cli" list 2>/dev/null | grep -qE "(^|[[:space:]])${vmname}([[:space:]]|$)"; then
  echo "info: OrbStack VM '${vmname}' already exists; starting it"
  "$orb_cli" start "${vmname}" || true
else
  echo "==> creating OrbStack VM '${vmname}' from ${qcow2}"
  # OrbStack's `create` flag-shape: --image points at a disk image;
  # OrbStack auto-detects qcow2 and converts internally if needed.
  "$orb_cli" create \
    --image "${qcow2}" \
    --memory "${MEMORY_MB}M" \
    "${vmname}"
fi

echo
echo "==> port forwards (OrbStack auto-publishes guest ports to host):"
echo "  wizard: http://localhost:${HOSTPORT_WIZARD}"
echo "  ssh:    ssh -p ${HOSTPORT_SSH} agent@localhost"
echo
echo "==> shell into the VM:"
echo "  ${orb_cli} shell ${vmname}"
echo
echo "Note: if OrbStack's auto-port-publish doesn't reach :8088,"
echo "open OrbStack settings -> ${vmname} -> Network -> add forward"
echo "TCP host ${HOSTPORT_WIZARD} -> guest 8088."
