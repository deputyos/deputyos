#!/usr/bin/env bash
# scripts/sign.sh — sign every artefact in build/ with minisign.
#
# `--dev`     : auto-generate a contributor keypair under
#               ~/.config/deputyos/dev-keys/ if absent. Loud about which
#               key was used.
# `--release` : read the key from $DEPUTYOS_RELEASE_KEY (a path). CI
#               populates this from a GitHub secret. Refuses to run on a
#               dev laptop with a clear message.
#
# The two modes share the signing code path; only key sourcing differs.
# That's the point — the release path is exercised every CI run.

set -euo pipefail

mode=""
if [[ "${1:-}" == "--dev" ]]; then
  mode="dev"
elif [[ "${1:-}" == "--release" ]]; then
  mode="release"
else
  echo "usage: sign.sh --dev|--release" >&2
  exit 64
fi

repo_root="$(cd "$(dirname "$0")/.." && pwd)"
build_dir="${repo_root}/build"

if compgen -G "${build_dir}/*.agentless" >/dev/null; then
  echo "error: refusing to sign while agentless development outputs exist in build/" >&2
  echo "       rebuild those target/profile pairs through deputyos-core first" >&2
  exit 66
fi

# Signable artefacts only — image outputs, not the staging tree.
shopt -s nullglob
artefacts=("$build_dir"/*.qcow2 "$build_dir"/*.img "$build_dir"/*.img.xz)

# Desktop launcher binaries (M2.5): deputyos-desktop-<rust-triple>, no
# extension. Exclude the .sha256/.minisig siblings this script itself
# produces so a re-run doesn't try to sign its own sidecars.
for f in "$build_dir"/deputyos-desktop-*; do
  case "$f" in
    *.sha256|*.minisig) continue ;;
  esac
  [[ -f "$f" ]] && artefacts+=("$f")
done

if [[ ${#artefacts[@]} -eq 0 ]]; then
  echo "info: no signable artefacts in build/ (looked for *.qcow2 *.img *.img.xz deputyos-desktop-*); nothing to sign" >&2
  exit 0
fi

if ! command -v minisign >/dev/null 2>&1; then
  echo "error: minisign not installed (see \`make doctor\`)" >&2
  exit 1
fi

key_path=""
case "$mode" in
  dev)
    keys_dir="${HOME}/.config/deputyos/dev-keys"
    mkdir -p "$keys_dir"
    chmod 700 "$keys_dir"
    key_path="${keys_dir}/deputyos-dev.key"
    if [[ ! -f "$key_path" ]]; then
      echo "info: generating new dev keypair at ${keys_dir}" >&2
      minisign -G -p "${keys_dir}/deputyos-dev.pub" -s "$key_path" -W
    fi
    echo "info: signing with dev key ${key_path}" >&2
    ;;
  release)
    if [[ -z "${DEPUTYOS_RELEASE_KEY:-}" ]]; then
      echo "error: DEPUTYOS_RELEASE_KEY not set." >&2
      echo "  This target is CI-only. On a dev laptop use 'make sign-dev'." >&2
      exit 1
    fi
    key_path="${DEPUTYOS_RELEASE_KEY}"
    echo "info: signing with release key (from env DEPUTYOS_RELEASE_KEY)" >&2
    ;;
esac

for artefact in "${artefacts[@]}"; do
  echo "  signing: $(basename "$artefact")"
  minisign -S -s "$key_path" -m "$artefact" -W
  sha256sum "$artefact" > "${artefact}.sha256"
done
