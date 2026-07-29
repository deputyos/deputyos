#!/usr/bin/env bash
# scripts/publish-local.sh — copy signed artefacts + manifest into dist/<version>/.
#
# Mirrors the layout the real CDN will use, so `deputyctl update --check`
# in dev mode reads from file://./dist/manifest.json and exercises exactly
# the same code path as production HTTPS.
#
# Inputs:
#   build/              produced by `make build` and `make sign-dev`
#   dist/manifest.json  produced by `make manifest`
#
# Outputs:
#   dist/manifest.json{,.minisig}        # latest channel-default
#   dist/<release_version>/manifest.json{,.minisig}
#   dist/<release_version>/<artefact> + .sha256 + .minisig (one per artefact)
#   dist/pubkey.minisign                  # the signing public key
#
# Env:
#   DEPUTYOS_PUBLISH_MODE=copy|symlink     (default: copy)

set -euo pipefail

mode="${DEPUTYOS_PUBLISH_MODE:-copy}"
case "$mode" in
  copy|symlink) ;;
  *) echo "publish-local.sh: DEPUTYOS_PUBLISH_MODE must be copy|symlink (got '$mode')" >&2; exit 64 ;;
esac

repo_root="$(cd "$(dirname "$0")/.." && pwd)"
build_dir="${repo_root}/build"
dist_dir="${repo_root}/dist"
manifest_path="${dist_dir}/manifest.json"
sig_path="${manifest_path}.minisig"

if [[ ! -f "$manifest_path" ]]; then
  echo "publish-local.sh: $manifest_path not found; run 'make manifest' first" >&2
  exit 1
fi
if [[ ! -f "$sig_path" ]]; then
  echo "publish-local.sh: $sig_path not found; manifest must be signed (manifest.sh handles this)" >&2
  exit 1
fi

# Pull the version, channel, and artefact list straight from the manifest
# so we don't drift from the source of truth.
read -r release_version channel < <(
  python3 -c "
import json, sys
m = json.load(open('$manifest_path'))
print(m['release_version'], m['channel'])
"
)

if [[ -z "$release_version" || -z "$channel" ]]; then
  echo "publish-local.sh: could not extract release_version/channel from manifest" >&2
  exit 1
fi

versioned_dir="${dist_dir}/${release_version}"
mkdir -p "$versioned_dir"

place() {
  local src="$1" dst="$2"
  if [[ ! -e "$src" ]]; then
    echo "publish-local.sh: missing source $src" >&2
    return 1
  fi
  rm -f "$dst"
  case "$mode" in
    copy)    cp -- "$src" "$dst" ;;
    symlink) ln -s "$src" "$dst" ;;
  esac
}

# Snapshot the manifest into the versioned directory (immutable record).
place "$manifest_path" "${versioned_dir}/manifest.json"
place "$sig_path"      "${versioned_dir}/manifest.json.minisig"

# Copy each artefact + sha256 + minisig into the versioned dir.
mapfile -t filenames < <(
  python3 -c "
import json
m = json.load(open('$manifest_path'))
for a in m['artefacts']:
    print(a['filename'])
"
)

if [[ ${#filenames[@]} -eq 0 ]]; then
  echo "publish-local.sh: manifest has no artefacts; nothing to publish" >&2
  exit 1
fi

for fname in "${filenames[@]}"; do
  src="${build_dir}/${fname}"
  place "$src"             "${versioned_dir}/${fname}"
  place "${src}.sha256"    "${versioned_dir}/${fname}.sha256"
  place "${src}.minisig"   "${versioned_dir}/${fname}.minisig"
  echo "publish-local.sh: published ${fname}"
done

# Publish the release verification key when supplied; otherwise retain the
# contributor dev-key behavior for local builds.
release_pub="${DEPUTYOS_RELEASE_PUBKEY_FILE:-}"
dev_pub="${HOME}/.config/deputyos/dev-keys/deputyos-dev.pub"
if [[ -n "$release_pub" && -f "$release_pub" ]]; then
  cp -- "$release_pub" "${dist_dir}/pubkey.minisign"
  echo "publish-local.sh: copied release pubkey -> ${dist_dir}/pubkey.minisign"
elif [[ -f "$dev_pub" ]]; then
  cp -- "$dev_pub" "${dist_dir}/pubkey.minisign"
  echo "publish-local.sh: copied $dev_pub -> ${dist_dir}/pubkey.minisign"
else
  echo "publish-local.sh: warn: no release or dev pubkey available; verifiers will need DEPUTYOS_UPDATE_PUBKEY set explicitly" >&2
fi

echo "publish-local.sh: dist/ now mirrors release ${release_version} (${channel})"
