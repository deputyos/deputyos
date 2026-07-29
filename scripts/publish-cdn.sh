#!/usr/bin/env bash
# scripts/publish-cdn.sh — push the dist/ tree to the artefact CDN bucket.
#
# The CDN is a Backblaze B2 bucket (cdn-deputyos-com) fronted by Cloudflare
# (Bandwidth Alliance → free egress + edge caching at cdn.deputyos.com). rclone
# speaks B2 natively. Publication uses `rclone copy`, not `sync`: a clean CI
# checkout contains only the current version, so `sync` would delete every
# older immutable release directory from the bucket.
#
# Local-first: identical on a contributor laptop and in CI. Configure the rclone
# remote once (`rclone config`, type=b2, your B2 keyID+applicationKey) as the
# remote named in DEPUTYOS_CDN_REMOTE (default `b2:cdn-deputyos-com`). The
# on-CDN layout is produced by `make publish-local` and is identical regardless
# of backend, so `deputyctl update` / launcher manifest URLs never change.
#
# Backend-agnostic: point DEPUTYOS_CDN_REMOTE at any rclone remote
# (`b2:bucket`, `r2:bucket`, `s3:bucket`, …). The remote's scheme (`b2:` vs
# `r2:`) selects the backend; this script only orchestrates publication.

set -euo pipefail

repo_root="$(cd "$(dirname "$0")/.." && pwd)"
src="${DEPUTYOS_DIST_DIR:-${repo_root}/dist}"
remote="${DEPUTYOS_CDN_REMOTE:-b2:cdn-deputyos-com}"
remote_name="${remote%%:*}"
manifest="${src}/manifest.json"

if [[ ! -d "$src" ]]; then
  echo "error: dist directory not found: $src" >&2
  echo "  run 'make publish-local' first." >&2
  exit 64
fi

if [[ ! -f "$manifest" ]]; then
  echo "error: release manifest not found: $manifest" >&2
  echo "  run 'make publish-local' first." >&2
  exit 64
fi

read -r release_version channel < <(
  python3 -c '
import json, sys
manifest = json.load(open(sys.argv[1]))
print(manifest["release_version"], manifest["channel"])
' "$manifest"
)
if [[ ! "$release_version" =~ ^[0-9]{4}\.[0-9]{1,2}\.[0-9]{1,2}(-[a-z0-9.-]+)?$ ]]; then
  echo "error: manifest has invalid release_version '$release_version'" >&2
  exit 65
fi
case "$channel" in
  dev|beta|stable) ;;
  *)
    echo "error: manifest has invalid channel '$channel'" >&2
    exit 65
    ;;
esac
channel_remote="${remote%/}/${channel}"
version_dir="${src}/${release_version}"
if [[ ! -d "$version_dir" ]]; then
  echo "error: versioned release directory not found: $version_dir" >&2
  exit 65
fi
for root_file in manifest.json manifest.json.minisig pubkey.minisign; do
  if [[ ! -f "${src}/${root_file}" ]]; then
    echo "error: publication file missing: ${src}/${root_file}" >&2
    exit 65
  fi
done

if ! command -v rclone >/dev/null 2>&1; then
  echo "error: rclone not installed." >&2
  echo "  install via 'curl https://rclone.org/install.sh | sudo bash' or your package manager." >&2
  exit 64
fi

# Refuse to run if the configured remote isn't set up.
if ! rclone listremotes 2>/dev/null | grep -qx "${remote_name}:"; then
  echo "error: rclone has no '${remote_name}:' remote configured." >&2
  echo "  run 'rclone config' and add the remote (for B2: type=b2, your keyID + applicationKey)." >&2
  echo "  Or set DEPUTYOS_CDN_REMOTE to a remote you have (e.g. r2:cdn-deputyos-com)." >&2
  exit 64
fi

common_args=(
  --progress \
  --transfers=4 \
  --checkers=8 \
  --no-update-modtime
)

echo "==> publishing immutable release $version_dir → $channel_remote/$release_version"
rclone copy "${common_args[@]}" "$version_dir" "$channel_remote/$release_version"

echo "==> advancing $channel latest manifest"
# Upload the signed dependencies first and manifest.json last, so readers
# cannot observe a latest manifest before its signature/key are present.
for root_file in pubkey.minisign manifest.json.minisig manifest.json; do
  rclone copyto "${common_args[@]}" \
    "${src}/${root_file}" \
    "${channel_remote}/${root_file}"
done
