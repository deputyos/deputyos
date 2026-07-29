#!/usr/bin/env bash
# scripts/desktop-local-preflight.sh — fail fast for the run-locally loop.
#
# `make desktop-local-build` / `make desktop-local` need tools that `make
# doctor` classifies as OPTIONAL (minisign, packer) — so doctor warns but the
# loop fails mid-build (e.g. after a 15-min Packer run). This preflight checks
# the tools the loop actually requires and bails with the same install hints
# `make doctor` prints, BEFORE the slow build runs.
#
# Sources scripts/doctor.sh for `detect_os` + `fix_hint` (doctor.sh returns
# early when sourced, so its own output isn't printed here).
set -euo pipefail

repo_root="$(cd "$(dirname "$0")/.." && pwd)"
cd "$repo_root"

# shellcheck source=scripts/doctor.sh
source "${repo_root}/scripts/doctor.sh"   # detect_os, fix_hint, OS

profile="${PROFILE:-openclaw}"
qcow2="build/qemu-x86_64-${profile}.qcow2"

# Tools the full loop (build + cdn-up + desktop-local) needs. Packer is only
# needed to bake the qcow2 the first time; if it's already built, treat packer
# as optional so re-runs don't demand it.
required=(minisign ansible docker qemu-system-x86_64)
if [[ ! -f "$qcow2" ]]; then
  required+=(packer)
fi

# An ISO builder is needed for the cloud-init seed (desktop-local). Any one of
# the three suffices (same ladder as test/smoke/_common.sh).
iso_builder=""
for c in cloud-localds genisoimage xorriso; do
  if command -v "$c" >/dev/null 2>&1; then iso_builder="$c"; break; fi
done

missing=()
for dep in "${required[@]}"; do
  command -v "$dep" >/dev/null 2>&1 || missing+=("$dep")
done
[[ -n "$iso_builder" ]] || missing+=("iso-builder")

if (( ${#missing[@]} == 0 )); then
  exit 0
fi

cat >&2 <<EOF
desktop-local: missing prerequisites (the run-locally loop needs these):

EOF
for d in "${missing[@]}"; do
  if [[ "$d" == "iso-builder" ]]; then
    echo "    - one of: cloud-localds | genisoimage | xorriso  (builds the cloud-init seed ISO)" >&2
    echo "      install with: sudo apt install cloud-image-utils   # provides cloud-localds (simplest)" >&2
  else
    echo "    - $d" >&2
    fix_hint "$d" >&2
  fi
done
cat >&2 <<EOF

  Install the above, then re-run:
    make desktop-local-build && make cdn-up && make desktop-local

  (or run 'make doctor' for the full host-tooling report.)
EOF
exit 1