#!/usr/bin/env bash
# scripts/verify.sh — rebuild a published artefact locally and assert SHA256 match.
#
# This is the trust path: a third party can re-execute the build pipeline
# from source and confirm the bytes that `make publish-local` (or the real
# CDN) emitted are exactly what `make build` would have produced.
#
# Reproducible builds are a property landed by M4-M7. For now this script
# is end-to-end-wired but accepts that bit-identical reproduction may fail
# until hermetic builds harden. Default: warn-and-pass on mismatch.
# DEPUTYOS_VERIFY_STRICT=1: hard-fail on mismatch.

set -euo pipefail

usage() {
  cat >&2 <<'USAGE'
usage: scripts/verify.sh <release_version> [TARGET=<hw>] [PROFILE=<id>]

Env:
  DEPUTYOS_DIST_URL          base URL of the release dist (default: file://<repo>/dist)
  DEPUTYOS_VERIFY_STRICT=1   fail on SHA256 mismatch (default: warn-and-pass)
  TARGET, PROFILE           which artefact in the manifest to rebuild
USAGE
}

if [[ $# -lt 1 ]]; then
  usage
  exit 64
fi
release_version="$1"
shift

repo_root="$(cd "$(dirname "$0")/.." && pwd)"
dist_url="${DEPUTYOS_DIST_URL:-file://${repo_root}/dist}"
target="${TARGET:-qemu-aarch64}"
profile="${PROFILE:-openclaw}"
strict="${DEPUTYOS_VERIFY_STRICT:-0}"

# Fetch URL into a local file. Supports file://, http://, https://.
fetch() {
  local url="$1" dest="$2"
  case "$url" in
    file://*)
      local path="${url#file://}"
      cp -- "$path" "$dest"
      ;;
    http://*|https://*)
      if command -v curl >/dev/null 2>&1; then
        curl -fsSL "$url" -o "$dest"
      elif command -v wget >/dev/null 2>&1; then
        wget -q "$url" -O "$dest"
      else
        echo "verify.sh: need curl or wget for $url" >&2
        return 1
      fi
      ;;
    *) echo "verify.sh: unsupported URL scheme: $url" >&2; return 1 ;;
  esac
}

tmpdir="$(mktemp -d)"
trap 'rm -rf "$tmpdir"' EXIT

manifest_url="${dist_url}/${release_version}/manifest.json"
manifest_sig_url="${manifest_url}.minisig"
echo "verify.sh: fetching $manifest_url"
fetch "$manifest_url" "${tmpdir}/manifest.json"
fetch "$manifest_sig_url" "${tmpdir}/manifest.json.minisig"

# Verify the manifest signature with whatever pubkey we have.
pubkey="${DEPUTYOS_UPDATE_PUBKEY:-}"
if [[ -z "$pubkey" ]]; then
  if [[ -f "${repo_root}/dist/pubkey.minisign" ]]; then
    pubkey="${repo_root}/dist/pubkey.minisign"
  elif [[ -f "${HOME}/.config/deputyos/dev-keys/deputyos-dev.pub" ]]; then
    pubkey="${HOME}/.config/deputyos/dev-keys/deputyos-dev.pub"
  fi
fi
if [[ -n "$pubkey" && -f "$pubkey" ]] && command -v minisign >/dev/null 2>&1; then
  echo "verify.sh: verifying manifest signature with $pubkey"
  minisign -V -p "$pubkey" -m "${tmpdir}/manifest.json" -x "${tmpdir}/manifest.json.minisig"
else
  echo "verify.sh: warn: no pubkey or minisign; skipping signature check" >&2
fi

# Find the matching artefact for our target+profile.
mapfile -t entry < <(
  python3 -c "
import json, sys
m = json.load(open('${tmpdir}/manifest.json'))
for a in m['artefacts']:
    if a['target'] == '$target' and a['profile'] == '$profile':
        print(a['filename'])
        print(a['sha256'])
        print(a.get('url') or a['filename'])
        break
"
)

if [[ ${#entry[@]} -lt 3 ]]; then
  echo "verify.sh: no manifest entry for target=$target profile=$profile in ${release_version}" >&2
  exit 1
fi
fname="${entry[0]}"
published_sha="${entry[1]}"

echo "verify.sh: published sha256 $published_sha for $fname"

# Trigger a rebuild. The build script emits into build/. We use a
# dedicated build dir so we don't clobber the contributor's existing
# build/ tree.
rebuild_dir="${tmpdir}/rebuild"
mkdir -p "$rebuild_dir"
echo "verify.sh: rebuilding via 'make build TARGET=$target PROFILE=$profile DEPUTYOS_RELEASE_VERSION=$release_version'"
if ! ( cd "$repo_root" && \
       DEPUTYOS_RELEASE_VERSION="$release_version" \
       DEPUTYOS_BUILD_DIR="$rebuild_dir" \
       make build TARGET="$target" PROFILE="$profile" CHANNEL=dev ); then
  echo "verify.sh: rebuild failed (make build exited nonzero)" >&2
  if [[ "$strict" = "1" ]]; then
    exit 1
  else
    echo "verify.sh: warn: rebuild failed; reproducibility lands in M4-M7. Set DEPUTYOS_VERIFY_STRICT=1 to make this fatal." >&2
    exit 0
  fi
fi

# Locate the rebuilt artefact. build.sh writes into $DEPUTYOS_BUILD_DIR
# if set, else build/. Look in both.
rebuilt=""
for candidate in "$rebuild_dir/$fname" "${repo_root}/build/$fname"; do
  if [[ -f "$candidate" ]]; then
    rebuilt="$candidate"
    break
  fi
done

if [[ -z "$rebuilt" ]]; then
  echo "verify.sh: rebuild produced no artefact named $fname" >&2
  if [[ "$strict" = "1" ]]; then
    exit 1
  else
    echo "verify.sh: warn: cannot compare; reproducibility hardens in M4-M7." >&2
    exit 0
  fi
fi

rebuilt_sha="$(sha256sum "$rebuilt" | awk '{print $1}')"
echo "verify.sh: rebuilt sha256  $rebuilt_sha"

if [[ "$rebuilt_sha" == "$published_sha" ]]; then
  echo "verify.sh: PASS — bit-identical reproduction of $fname"
  exit 0
fi

echo "verify.sh: MISMATCH"
echo "  published: $published_sha"
echo "  rebuilt:   $rebuilt_sha"
echo "  artefact:  $rebuilt"
echo "  build did not reproduce — first byte difference:"
if command -v cmp >/dev/null 2>&1; then
  # Need a published copy to diff against. Fetch the artefact too.
  art_url_path="${entry[2]}"
  case "$art_url_path" in
    http://*|https://*|file://*) art_url="$art_url_path" ;;
    *) art_url="${dist_url}/${release_version}/${fname}" ;;
  esac
  if fetch "$art_url" "${tmpdir}/${fname}" 2>/dev/null; then
    cmp -l "${tmpdir}/${fname}" "$rebuilt" 2>&1 | head -5 || true
  fi
fi

if [[ "$strict" = "1" ]]; then
  echo "verify.sh: DEPUTYOS_VERIFY_STRICT=1 — failing." >&2
  exit 1
fi

echo "verify.sh: warn: reproducibility lands in M4-M7. Set DEPUTYOS_VERIFY_STRICT=1 to make this fatal." >&2
exit 0
