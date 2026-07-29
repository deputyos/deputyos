#!/usr/bin/env bash
# scripts/sbom.sh — generate a CycloneDX SBOM for a build artefact.
#
# Status: scaffold (M7 Lane B). Full SLSA L3 verification needs hermetic
# builds (M4) plus bit-identical reproduction. This script lands the
# *generator* — what gets attached to a release. The verifier lands later
# in M7 proper. Header explicit so reviewers don't mistake scaffolding
# for finished work.
#
# Sources, in order of preference:
#   1. `syft` if installed                — most accurate; reads the artefact.
#   2. Vanilla fallback                   — parses Ansible role apt: tasks
#                                           and `cargo metadata` for the
#                                           deputyctl/deputywizard crates.
#
# Output: <artefact>.sbom.cyclonedx.json (CycloneDX v1.5 JSON).
#
# Usage:
#   scripts/sbom.sh <artefact-path>
#   scripts/sbom.sh --self-test

set -euo pipefail

SCRIPT_NAME="$(basename "$0")"
readonly SCRIPT_NAME
REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
readonly REPO_ROOT

usage() {
  cat <<EOF
usage: $SCRIPT_NAME <artefact-path>
       $SCRIPT_NAME --self-test

Emits <artefact>.sbom.cyclonedx.json (CycloneDX v1.5).
EOF
}

err() {
  echo "$SCRIPT_NAME: error: $*" >&2
  exit 1
}

# --- helpers ----------------------------------------------------------------

json_escape() {
  local s="$1"
  s="${s//\\/\\\\}"
  s="${s//\"/\\\"}"
  s="${s//$'\n'/\\n}"
  s="${s//$'\r'/\\r}"
  s="${s//$'\t'/\\t}"
  printf '%s' "$s"
}

uuidv4() {
  if command -v uuidgen >/dev/null 2>&1; then
    uuidgen | tr '[:upper:]' '[:lower:]'
  elif [[ -r /proc/sys/kernel/random/uuid ]]; then
    cat /proc/sys/kernel/random/uuid
  else
    # Synthetic but unique-enough for a scaffold.
    printf '00000000-0000-4000-8000-%012d' "$(date -u +%s)"
  fi
}

now_utc() {
  date -u +"%Y-%m-%dT%H:%M:%SZ"
}

# --- generator: syft path ---------------------------------------------------

syft_generate() {
  local artefact="$1"
  local out="$2"
  echo "info: using syft to generate CycloneDX SBOM"
  syft "$artefact" -o cyclonedx-json > "$out"
}

# --- generator: fallback path -----------------------------------------------

# Emit one CycloneDX component object per line on stdout.
#
# Best-effort: walks the Ansible role's task files and pulls package names
# out of `ansible.builtin.apt` (and bare `apt:`) blocks. We use Python
# rather than line-grep because YAML "name:" appears in many unrelated
# contexts (handlers, top-level task names, systemd unit references).
# When PyYAML is missing we degrade silently — syft is the right tool for
# real coverage; this is the M7-Lane-B scaffold.
collect_apt_components() {
  local roles_dir="$REPO_ROOT/roles"
  [[ -d "$roles_dir" ]] || return 0
  command -v python3 >/dev/null 2>&1 || return 0

  python3 - "$roles_dir" <<'PY' 2>/dev/null || true
import json, os, re, sys

try:
    import yaml  # type: ignore
except Exception:
    sys.exit(0)

roles_dir = sys.argv[1]
pkgs = set()


def collect(task):
    if not isinstance(task, dict):
        return
    for key in ("apt", "ansible.builtin.apt"):
        block = task.get(key)
        if block is None:
            continue
        if isinstance(block, str):
            # apt: name=foo state=present
            m = re.search(r"name=([a-zA-Z0-9._+\-]+)", block)
            if m:
                pkgs.add(m.group(1))
        elif isinstance(block, dict):
            n = block.get("name")
            if isinstance(n, str):
                pkgs.add(n)
            elif isinstance(n, list):
                for item in n:
                    if isinstance(item, str):
                        pkgs.add(item)


for root, _, files in os.walk(roles_dir):
    for f in files:
        if not f.endswith((".yml", ".yaml")):
            continue
        try:
            with open(os.path.join(root, f)) as fh:
                docs = list(yaml.safe_load_all(fh))
        except Exception:
            continue
        for doc in docs:
            if isinstance(doc, list):
                for t in doc:
                    collect(t)
            elif isinstance(doc, dict):
                # play with tasks: section
                for key in ("tasks", "pre_tasks", "post_tasks", "handlers"):
                    for t in doc.get(key, []) or []:
                        collect(t)
                collect(doc)

# Strip Jinja templating noise.
clean = sorted(p for p in pkgs if re.fullmatch(r"[a-z0-9][a-z0-9._+\-]*", p or ""))
for p in clean:
    print(json.dumps({"type": "library", "name": p, "purl": f"pkg:deb/debian/{p}"}))
PY
}

collect_cargo_components() {
  local manifest="$REPO_ROOT/Cargo.toml"
  [[ -f "$manifest" ]] || return 0
  command -v cargo >/dev/null 2>&1 || return 0
  command -v python3 >/dev/null 2>&1 || return 0

  local meta_file
  meta_file="$(mktemp)"
  if ! cargo metadata --format-version 1 --no-deps --manifest-path "$manifest" \
       > "$meta_file" 2>/dev/null; then
    rm -f "$meta_file"
    return 0
  fi
  # `cargo metadata --no-deps` lists workspace crates only — full transitive
  # chains are syft's job; the fallback path is intentionally a sketch.
  python3 - "$meta_file" <<'PY' || true
import json, sys
with open(sys.argv[1]) as fh:
    d = json.load(fh)
for p in d.get("packages", []):
    name = p.get("name", "")
    version = p.get("version", "")
    if not name:
        continue
    purl = f"pkg:cargo/{name}@{version}" if version else f"pkg:cargo/{name}"
    print(json.dumps({"type": "library", "name": name, "version": version, "purl": purl}))
PY
  rm -f "$meta_file"
}

fallback_generate() {
  local artefact="$1"
  local out="$2"
  echo "info: syft not installed; emitting fallback CycloneDX SBOM"

  local sha
  if command -v sha256sum >/dev/null 2>&1; then
    sha="$(sha256sum "$artefact" | awk '{print $1}')"
  elif command -v shasum >/dev/null 2>&1; then
    sha="$(shasum -a 256 "$artefact" | awk '{print $1}')"
  else
    sha=""
  fi

  local serial
  serial="urn:uuid:$(uuidv4)"
  local now
  now="$(now_utc)"
  local subject_name
  subject_name="$(basename "$artefact")"

  local components_json
  components_json="$( {
    collect_apt_components
    collect_cargo_components
  } | paste -sd, - )"
  components_json="${components_json:-}"

  local hashes_field=""
  if [[ -n "$sha" ]]; then
    hashes_field=",\"hashes\":[{\"alg\":\"SHA-256\",\"content\":\"$(json_escape "$sha")\"}]"
  fi

  cat > "$out" <<EOF
{
  "bomFormat": "CycloneDX",
  "specVersion": "1.5",
  "serialNumber": "$(json_escape "$serial")",
  "version": 1,
  "metadata": {
    "timestamp": "$(json_escape "$now")",
    "tools": [
      {"vendor": "deputyos", "name": "scripts/sbom.sh", "version": "scaffold"}
    ],
    "component": {
      "type": "operating-system",
      "name": "$(json_escape "$subject_name")",
      "bom-ref": "$(json_escape "$subject_name")"$hashes_field
    }
  },
  "components": [$components_json]
}
EOF
}

# --- self-test --------------------------------------------------------------

self_test() {
  local tmp
  tmp="$(mktemp -d)"
  # shellcheck disable=SC2064
  trap "rm -rf '$tmp'" EXIT

  local artefact="$tmp/synth.qcow2"
  printf 'pretend-image-bytes' > "$artefact"
  local out="${artefact}.sbom.cyclonedx.json"
  fallback_generate "$artefact" "$out"

  [[ -s "$out" ]] || err "self-test: empty SBOM output"

  if command -v jq >/dev/null 2>&1; then
    jq -e '
      .bomFormat == "CycloneDX"
      and .specVersion == "1.5"
      and (.serialNumber | startswith("urn:uuid:"))
      and (.metadata.timestamp | type == "string")
      and (.components | type == "array")
    ' "$out" >/dev/null || err "self-test: missing required CycloneDX fields"
  elif command -v python3 >/dev/null 2>&1; then
    python3 - "$out" <<'PY'
import json, sys
d = json.load(open(sys.argv[1]))
assert d["bomFormat"] == "CycloneDX"
assert d["specVersion"] == "1.5"
assert d["serialNumber"].startswith("urn:uuid:")
assert isinstance(d["components"], list)
assert d["metadata"]["timestamp"]
PY
  else
    err "self-test: need jq or python3"
  fi

  echo "self-test: OK ($out)"
}

# --- entry ------------------------------------------------------------------

main() {
  if [[ $# -eq 0 ]]; then
    usage >&2
    exit 64
  fi

  case "${1:-}" in
    -h|--help)
      usage
      exit 0
      ;;
    --self-test)
      self_test
      exit 0
      ;;
    *)
      local artefact="$1"
      [[ -f "$artefact" ]] || err "artefact not found: $artefact"
      local out="${artefact}.sbom.cyclonedx.json"
      if command -v syft >/dev/null 2>&1; then
        syft_generate "$artefact" "$out"
      else
        fallback_generate "$artefact" "$out"
      fi
      echo "wrote: $out"
      ;;
  esac
}

main "$@"
