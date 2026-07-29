#!/usr/bin/env bash
# scripts/manifest.sh — generate dist/manifest.json from signed artefacts in build/.
#
# Scans build/ for signable artefacts (.qcow2, .img, .img.xz, .tar.gz),
# their .sha256 siblings, and their .minisig siblings; emits one entry per
# artefact in a manifest matching docs/schemas/manifest-v1.json. Then signs
# the manifest itself with minisign so `deputyctl update --check` can verify
# it before reading any artefact metadata.
#
# Filename pattern (docs/03-image-builds.md §"Build outputs (naming)"):
#   deputyos-<profile>-<target>-<version>-<channel>.<ext>
#
# Hard rule: identical operation locally and in CI. No magic env that only
# CI sets. Key sourcing parallels scripts/sign.sh — `--key-mode dev` uses
# the contributor key under ~/.config/deputyos/dev-keys/, `--key-mode release`
# reads from $DEPUTYOS_RELEASE_KEY.

set -euo pipefail

usage() {
  cat >&2 <<'USAGE'
usage: scripts/manifest.sh --release-version <Y.M.D[-pre]> [--channel dev|beta|stable] [--key-mode dev|release] [--validate]

Env vars (alternative to flags):
  DEPUTYOS_RELEASE_VERSION   release version (Y.M.D)
  DEPUTYOS_CHANNEL           channel (default: dev)
  DEPUTYOS_KEY_MODE          dev | release (default: dev)

Outputs:
  dist/manifest.json
  dist/manifest.json.minisig
USAGE
}

release_version="${DEPUTYOS_RELEASE_VERSION:-}"
channel="${DEPUTYOS_CHANNEL:-dev}"
key_mode="${DEPUTYOS_KEY_MODE:-dev}"
validate=0

while [[ $# -gt 0 ]]; do
  case "$1" in
    --release-version) release_version="$2"; shift 2 ;;
    --channel)         channel="$2"; shift 2 ;;
    --key-mode)        key_mode="$2"; shift 2 ;;
    --validate)        validate=1; shift ;;
    -h|--help)         usage; exit 0 ;;
    *) echo "manifest.sh: unknown flag: $1" >&2; usage; exit 64 ;;
  esac
done

if [[ -z "$release_version" ]]; then
  echo "manifest.sh: --release-version (or DEPUTYOS_RELEASE_VERSION) required" >&2
  usage
  exit 64
fi

# Validate version format up-front so we fail before doing any work.
if ! [[ "$release_version" =~ ^[0-9]{4}\.[0-9]{1,2}\.[0-9]{1,2}(-[a-z0-9.-]+)?$ ]]; then
  echo "manifest.sh: --release-version '$release_version' is not Y.M.D[-pre]" >&2
  exit 64
fi

case "$channel" in
  dev|beta|stable) ;;
  *) echo "manifest.sh: --channel must be dev|beta|stable, got '$channel'" >&2; exit 64 ;;
esac

case "$key_mode" in
  dev|release) ;;
  *) echo "manifest.sh: --key-mode must be dev|release, got '$key_mode'" >&2; exit 64 ;;
esac

repo_root="$(cd "$(dirname "$0")/.." && pwd)"
build_dir="${repo_root}/build"
dist_dir="${repo_root}/dist"
schema_path="${repo_root}/docs/schemas/manifest-v1.json"

if compgen -G "${build_dir}/*.agentless" >/dev/null; then
  echo "manifest.sh: refusing to publish agentless development outputs" >&2
  echo "             rebuild those target/profile pairs through deputyos-core first" >&2
  exit 66
fi

if [[ "$key_mode" == "release" || "$channel" == "stable" ]]; then
  placeholder_hits="$(
    grep -R -n -E '<<<NEEDS_|REPLACE_ME_' \
      "${repo_root}/README.md" \
      "${repo_root}/cloud-init" \
      "${repo_root}/wsl" \
      "${repo_root}/scripts/wsl2-build.sh" \
      "${repo_root}/deputyos-desktop/src/config.rs" \
      "${repo_root}/roles/deputyos/vars/llm-airgap.yml" \
      2>/dev/null || true
  )"
  if [[ -n "$placeholder_hits" ]]; then
    echo "manifest.sh: release-critical placeholders remain; refusing ${channel}/${key_mode} manifest" >&2
    echo "$placeholder_hits" >&2
    echo "manifest.sh: use channel=dev for scaffold builds, or replace the placeholders before release." >&2
    exit 65
  fi
fi

# Resolve signing key (parallel to scripts/sign.sh).
key_path=""
case "$key_mode" in
  dev)
    keys_dir="${HOME}/.config/deputyos/dev-keys"
    key_path="${keys_dir}/deputyos-dev.key"
    if [[ ! -f "$key_path" ]]; then
      echo "manifest.sh: dev key missing at $key_path; run 'make sign-dev' first" >&2
      exit 1
    fi
    ;;
  release)
    if [[ -z "${DEPUTYOS_RELEASE_KEY:-}" ]]; then
      echo "manifest.sh: DEPUTYOS_RELEASE_KEY not set (release key sourcing parallels sign.sh)" >&2
      exit 1
    fi
    key_path="${DEPUTYOS_RELEASE_KEY}"
    ;;
esac

if ! command -v minisign >/dev/null 2>&1; then
  echo "manifest.sh: minisign not installed (see 'make doctor')" >&2
  exit 1
fi

# Enumerate signed artefacts. We require both a .sha256 and a .minisig
# sibling — otherwise the artefact is unfinished and shouldn't be in a
# manifest.
shopt -s nullglob
candidates=("$build_dir"/*.qcow2 "$build_dir"/*.img "$build_dir"/*.img.xz "$build_dir"/*.tar.gz)

if [[ ${#candidates[@]} -eq 0 ]]; then
  echo "manifest.sh: no artefacts in build/" >&2
  exit 1
fi

# Build the manifest with python3 (always present on CI runners and
# contributor machines per `make doctor`). This avoids ad-hoc JSON
# escaping in bash.
mkdir -p "$dist_dir"
manifest_path="${dist_dir}/manifest.json"

python3 - "$release_version" "$channel" "$build_dir" "$manifest_path" "${candidates[@]}" <<'PY'
import hashlib
import json
import os
import re
import sys
from datetime import datetime, timezone
from pathlib import Path

release_version = sys.argv[1]
channel = sys.argv[2]
build_dir = Path(sys.argv[3])
manifest_path = Path(sys.argv[4])
candidates = [Path(p) for p in sys.argv[5:]]

# deputyos-<profile>-<target>-<version>-<channel>.<ext>
#
# Both `profile` and `target` may legitimately contain hyphens
# (e.g. target=qemu-aarch64, x86_64-mini-pc, oracle-arm-free), so we can't
# treat them as plain `[a-z0-9-]+` segments — the regex would split
# ambiguously. Anchor `version` (digits.dots) and `channel` (closed enum)
# from the right, then resolve the prefix against the known profile set.
KNOWN_PROFILES = {"openclaw", "hermes"}

pattern = re.compile(
    r"^deputyos-(?P<middle>.+)-(?P<version>[0-9]+\.[0-9]+\.[0-9]+(?:-[a-z0-9.-]+)?)-(?P<channel>dev|beta|stable)\.(?P<ext>img\.xz|qcow2|img|tar\.gz)$"
)


def split_profile_target(middle: str) -> "tuple[str, str] | None":
    """Resolve `<profile>-<target>` against the known-profile set. Returns
    (profile, target) or None if no profile prefix matches."""
    for prof in sorted(KNOWN_PROFILES, key=len, reverse=True):
        prefix = prof + "-"
        if middle.startswith(prefix):
            target = middle[len(prefix):]
            if target:
                return prof, target
    return None

EXT_TO_FORMAT = {
    "img.xz": "img.xz",
    "qcow2":  "qcow2",
    "tar.gz": "tar.gz",
    # bare .img is an interim build product; treat as img.xz (post-compression
    # is what ships); skip if not also present compressed.
}

artefacts = []
skipped = []

for path in candidates:
    name = path.name
    m = pattern.match(name)
    if not m:
        skipped.append((name, "filename does not match deputyos-<profile>-<target>-<version>-<channel>.<ext>"))
        continue
    if m.group("version") != release_version:
        skipped.append((name, f"version '{m.group('version')}' != --release-version '{release_version}'"))
        continue
    if m.group("channel") != channel:
        skipped.append((name, f"channel '{m.group('channel')}' != --channel '{channel}'"))
        continue
    split = split_profile_target(m.group("middle"))
    if split is None:
        skipped.append((name, f"middle segment '{m.group('middle')}' does not start with a known profile ({sorted(KNOWN_PROFILES)})"))
        continue
    profile, target = split
    ext = m.group("ext")
    if ext == "img":
        # Bare .img — superseded by the .img.xz variant; skip with note.
        skipped.append((name, "bare .img is superseded by .img.xz; skipping"))
        continue
    fmt = EXT_TO_FORMAT.get(ext)
    if fmt is None:
        skipped.append((name, f"unsupported extension '{ext}'"))
        continue

    # Require .sha256 and .minisig siblings.
    sha_sibling = path.with_suffix(path.suffix + ".sha256")
    sig_sibling = path.with_suffix(path.suffix + ".minisig")
    if not sha_sibling.is_file():
        skipped.append((name, f"missing sibling {sha_sibling.name}"))
        continue
    if not sig_sibling.is_file():
        skipped.append((name, f"missing sibling {sig_sibling.name} (run 'make sign-dev')"))
        continue

    # Re-hash to be paranoid; the .sha256 file is just for human eyeballs.
    h = hashlib.sha256()
    with path.open("rb") as f:
        for chunk in iter(lambda: f.read(1 << 20), b""):
            h.update(chunk)
    sha = h.hexdigest()

    artefacts.append({
        "target":     target,
        "profile":    profile,
        "filename":   name,
        "format":     fmt,
        "size_bytes": path.stat().st_size,
        "sha256":     sha,
        "minisig_url": f"{release_version}/{name}.minisig",
        "url":         f"{release_version}/{name}",
    })

for fname, why in skipped:
    print(f"manifest.sh: skipping {fname}: {why}", file=sys.stderr)

if not artefacts:
    print("manifest.sh: no artefacts matched the requested version+channel", file=sys.stderr)
    sys.exit(1)

# Desktop launcher binaries (M2.5): deputyos-desktop-<rust-triple>, no
# extension, with .sha256 + .minisig siblings (produced by sign.sh). The
# triple is the suffix after "deputyos-desktop-". One entry per signed
# launcher; `deputyos-desktop self-update` looks up its host triple here.
desktop_launchers = {}
for p in sorted(build_dir.glob("deputyos-desktop-*")):
    name = p.name
    if name.endswith(".sha256") or name.endswith(".minisig"):
        continue
    if not p.is_file():
        continue
    triple = name[len("deputyos-desktop-"):]
    sha_sibling = p.with_name(name + ".sha256")
    sig_sibling = p.with_name(name + ".minisig")
    if not (sha_sibling.is_file() and sig_sibling.is_file()):
        print(f"manifest.sh: skipping launcher {name}: missing .sha256/.minisig sibling", file=sys.stderr)
        continue
    h = hashlib.sha256()
    with open(p, "rb") as f:
        for chunk in iter(lambda: f.read(1 << 20), b""):
            h.update(chunk)
    desktop_launchers[triple] = {
        "triple": triple,
        "filename": name,
        "url": f"{release_version}/{name}",
        "sha256": h.hexdigest(),
        "minisig_url": f"{release_version}/{name}.minisig",
    }

manifest = {
    "schema_version": 1,
    "release_version": release_version,
    "channel": channel,
    "released_at": datetime.now(timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ"),
    "artefacts": sorted(artefacts, key=lambda a: (a["profile"], a["target"], a["format"])),
    "desktop_launchers": desktop_launchers,
}

manifest_path.parent.mkdir(parents=True, exist_ok=True)
# Canonical encoding: sorted keys, compact + trailing newline. Sorted keys
# are required so the signature is deterministic across machines.
data = json.dumps(manifest, sort_keys=True, separators=(",", ":")) + "\n"
manifest_path.write_text(data, encoding="utf-8")

print(f"manifest.sh: wrote {manifest_path} ({len(artefacts)} artefacts, {len(desktop_launchers)} launcher(s))")
PY

# Sign the manifest itself.
echo "manifest.sh: signing $manifest_path"
rm -f "${manifest_path}.minisig"
minisign -S -s "$key_path" -m "$manifest_path" -W \
  -t "deputyos manifest ${release_version} ${channel}" \
  -c "deputyos manifest ${release_version} ${channel}"

# Optional schema validation. Prefer ajv (Node), fall back to Python
# jsonschema, else warn-and-skip — the schema lives at a known path so
# contributors can always re-run validation by hand.
if [[ "$validate" == "1" ]]; then
  if command -v ajv >/dev/null 2>&1; then
    ajv validate -s "$schema_path" -d "$manifest_path"
  elif python3 -c 'import jsonschema' >/dev/null 2>&1; then
    python3 -c "
import json, sys
import jsonschema
schema = json.load(open('$schema_path'))
data = json.load(open('$manifest_path'))
jsonschema.validate(data, schema)
print('manifest.sh: schema validation passed')
" || exit 1
  elif command -v jsonschema >/dev/null 2>&1; then
    jsonschema -i "$manifest_path" "$schema_path" \
      && echo "manifest.sh: schema validation passed"
  else
    echo "manifest.sh: warn: no schema validator (ajv / python jsonschema / jsonschema CLI) installed; skipping --validate" >&2
  fi
fi

echo "manifest.sh: done"
