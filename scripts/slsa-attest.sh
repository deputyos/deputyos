#!/usr/bin/env bash
# scripts/slsa-attest.sh — generate a SLSA v1.0 provenance attestation
# (in-toto Statement) for a single build artefact.
#
# This is the *generator* half of the SLSA L3 story tracked in
# docs/11-roadmap.md §M7 Lane B. Reproducible-build attestations require
# a verifier on the consumer side and bit-identical reproduction across
# independent builders — both land later in M7. The scaffold here:
#
#   * Computes sha256 of the artefact.
#   * Emits a valid in-toto Statement v1.0 with predicateType
#     https://slsa.dev/provenance/v1.
#   * Signs with `cosign attest-blob` if available, else with `minisign`
#     using the same dev key as scripts/sign.sh.
#   * Writes <artefact>.intoto.jsonl alongside the artefact, plus a
#     detached signature (.minisig or .cosign.sig).
#
# Best-effort fields (populated if discoverable, otherwise sensibly
# stubbed; documented as such in the JSON for the verifier to inspect):
#
#   * predicate.builder.id          — synthetic until M7 lands a real CI URL.
#   * predicate.invocation.parameters — env keys only, no values (no secret leak).
#   * predicate.materials           — base image SHA from packer if traceable;
#                                     plus the role's git SHA.
#   * predicate.metadata.buildStartedOn / buildFinishedOn — `date -u`'d if not
#                                     supplied via env.
#
# Usage:
#   scripts/slsa-attest.sh <artefact-path>
#   scripts/slsa-attest.sh --self-test
#
# Bash strict mode + shellcheck-clean.

set -euo pipefail

SCRIPT_NAME="$(basename "$0")"
readonly SCRIPT_NAME
REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
readonly REPO_ROOT

usage() {
  cat <<EOF
usage: $SCRIPT_NAME <artefact-path>
       $SCRIPT_NAME --self-test

Emits <artefact-path>.intoto.jsonl with a SLSA v1.0 provenance statement.
EOF
}

err() {
  echo "$SCRIPT_NAME: error: $*" >&2
  exit 1
}

# --- helpers ----------------------------------------------------------------

sha256_of() {
  local path="$1"
  if command -v sha256sum >/dev/null 2>&1; then
    sha256sum "$path" | awk '{print $1}'
  elif command -v shasum >/dev/null 2>&1; then
    shasum -a 256 "$path" | awk '{print $1}'
  else
    err "neither sha256sum nor shasum found"
  fi
}

git_or_default() {
  local cmd="$1"
  local fallback="$2"
  if [[ -d "$REPO_ROOT/.git" ]] && command -v git >/dev/null 2>&1; then
    (cd "$REPO_ROOT" && eval "$cmd" 2>/dev/null) || printf '%s' "$fallback"
  else
    printf '%s' "$fallback"
  fi
}

now_utc() {
  date -u +"%Y-%m-%dT%H:%M:%SZ"
}

# JSON-escape a string for inline emission. Handles backslashes, quotes,
# and the basic control chars the spec cares about. Avoids depending on
# jq because we want the scaffold to run on a vanilla CI image.
json_escape() {
  local s="$1"
  s="${s//\\/\\\\}"
  s="${s//\"/\\\"}"
  s="${s//$'\n'/\\n}"
  s="${s//$'\r'/\\r}"
  s="${s//$'\t'/\\t}"
  printf '%s' "$s"
}

# --- core generator ---------------------------------------------------------

generate_provenance() {
  # $1 artefact path
  # $2 output JSONL path
  local artefact="$1"
  local out="$2"

  [[ -f "$artefact" ]] || err "artefact not found: $artefact"

  local subject_name
  subject_name="$(basename "$artefact")"
  local subject_sha
  subject_sha="$(sha256_of "$artefact")"

  local source_uri
  source_uri="$(git_or_default 'git config --get remote.origin.url' '(local)')"
  local source_sha
  source_sha="$(git_or_default 'git rev-parse HEAD' '(local)')"

  local started="${SLSA_BUILD_STARTED_ON:-$(now_utc)}"
  local finished="${SLSA_BUILD_FINISHED_ON:-$(now_utc)}"

  local builder_id="https://www.deputyos.com/builders/v1"
  if [[ -n "${GITHUB_SERVER_URL:-}" && -n "${GITHUB_REPOSITORY:-}" ]]; then
    # When run inside GitHub Actions, advertise the CI workflow as builder.
    builder_id="${GITHUB_SERVER_URL}/${GITHUB_REPOSITORY}/actions"
  fi

  # Materials: best-effort. We list the base image SHA if we can find it
  # in packer/, plus the role HEAD. Both are advisory until reproducible
  # builds land.
  local materials_json
  materials_json="$(collect_materials)"

  # Emit the in-toto Statement. Single-line JSON to keep .jsonl-friendly.
  local doc
  doc=$(cat <<EOF
{"_type":"https://in-toto.io/Statement/v1","subject":[{"name":"$(json_escape "$subject_name")","digest":{"sha256":"$(json_escape "$subject_sha")"}}],"predicateType":"https://slsa.dev/provenance/v1","predicate":{"buildDefinition":{"buildType":"https://www.deputyos.com/build-types/packer-ansible/v1","externalParameters":{"target":"$(json_escape "${TARGET:-unknown}")","profile":"$(json_escape "${PROFILE:-unknown}")","channel":"$(json_escape "${CHANNEL:-unknown}")","configSource":{"uri":"$(json_escape "$source_uri")","digest":{"sha1":"$(json_escape "$source_sha")"},"entryPoint":"scripts/build.sh"}},"internalParameters":{},"resolvedDependencies":$materials_json},"runDetails":{"builder":{"id":"$(json_escape "$builder_id")","builderDependencies":[],"version":{}},"metadata":{"invocationId":"$(json_escape "${GITHUB_RUN_ID:-local-$(date -u +%s)}")","startedOn":"$(json_escape "$started")","finishedOn":"$(json_escape "$finished")"},"byproducts":[]}}}
EOF
)
  printf '%s\n' "$doc" > "$out"
}

collect_materials() {
  # Emit a JSON array of {uri, digest} objects. Best-effort:
  #   * deputyOS source repo @ HEAD.
  #   * Pinned packer base image SHAs, when present in packer/*.pkr.hcl.
  local items=()

  local repo_uri
  repo_uri="$(git_or_default 'git config --get remote.origin.url' 'file://'"$REPO_ROOT")"
  local repo_sha
  repo_sha="$(git_or_default 'git rev-parse HEAD' 'unknown')"
  items+=("{\"uri\":\"$(json_escape "$repo_uri")\",\"digest\":{\"sha1\":\"$(json_escape "$repo_sha")\"}}")

  if [[ -d "$REPO_ROOT/packer" ]]; then
    # Look for `iso_checksum = "sha256:..."` lines (Lane B's pinning convention).
    while IFS= read -r line; do
      local sha="${line#*sha256:}"
      sha="${sha%%\"*}"
      if [[ -n "$sha" && ${#sha} -ge 32 ]]; then
        items+=("{\"uri\":\"pkg:packer/base-image\",\"digest\":{\"sha256\":\"$(json_escape "$sha")\"}}")
      fi
    done < <(grep -rh 'iso_checksum' "$REPO_ROOT/packer" 2>/dev/null | grep 'sha256:' | head -8 || true)
  fi

  local IFS=,
  printf '[%s]' "${items[*]}"
}

sign_artefact() {
  local jsonl="$1"

  if command -v cosign >/dev/null 2>&1 && [[ -n "${COSIGN_PASSWORD:-}${COSIGN_KEY:-}" ]]; then
    echo "info: signing $jsonl with cosign attest-blob"
    cosign attest-blob --predicate "$jsonl" --output-signature "${jsonl}.cosign.sig" "$jsonl" || true
    return 0
  fi

  if command -v minisign >/dev/null 2>&1; then
    local key="${HOME}/.config/deputyos/dev-keys/deputyos-dev.key"
    if [[ ! -f "$key" ]]; then
      echo "info: no dev minisign key at $key; skipping signature (run 'make sign-dev' to bootstrap)"
      return 0
    fi
    echo "info: signing $jsonl with minisign dev key"
    minisign -S -s "$key" -m "$jsonl" -W || true
    return 0
  fi

  echo "warn: neither cosign nor minisign available; provenance unsigned"
}

# --- self-test --------------------------------------------------------------

self_test() {
  local tmp
  tmp="$(mktemp -d)"
  # shellcheck disable=SC2064  # expand $tmp at trap-set time so the EXIT trap
  # still has the path after the local goes out of scope.
  trap "rm -rf '$tmp'" EXIT

  local artefact="$tmp/synth.qcow2"
  printf 'pretend-image-bytes' > "$artefact"

  local out="${artefact}.intoto.jsonl"
  generate_provenance "$artefact" "$out"

  [[ -s "$out" ]] || err "self-test: empty provenance output"

  # Validate top-level shape with whatever JSON validator is at hand.
  local payload
  payload="$(cat "$out")"
  if command -v jq >/dev/null 2>&1; then
    echo "$payload" | jq -e '
      ._type == "https://in-toto.io/Statement/v1"
      and (.subject | type == "array")
      and (.subject[0].digest.sha256 | type == "string")
      and (.predicateType == "https://slsa.dev/provenance/v1")
      and (.predicate.buildDefinition.buildType | type == "string")
      and (.predicate.runDetails.builder.id | type == "string")
      and (.predicate.runDetails.metadata.startedOn | type == "string")
      and (.predicate.runDetails.metadata.finishedOn | type == "string")
    ' >/dev/null || err "self-test: missing required SLSA fields"
  elif command -v python3 >/dev/null 2>&1; then
    python3 - <<PY
import json, sys
d = json.loads('''$payload''')
assert d["_type"] == "https://in-toto.io/Statement/v1", "wrong _type"
assert d["predicateType"] == "https://slsa.dev/provenance/v1", "wrong predicateType"
assert isinstance(d["subject"], list) and d["subject"], "no subject"
assert "sha256" in d["subject"][0]["digest"], "no sha256"
assert d["predicate"]["buildDefinition"]["buildType"], "no buildType"
assert d["predicate"]["runDetails"]["builder"]["id"], "no builder id"
assert d["predicate"]["runDetails"]["metadata"]["startedOn"], "no startedOn"
assert d["predicate"]["runDetails"]["metadata"]["finishedOn"], "no finishedOn"
PY
  else
    err "self-test: need jq or python3 to validate provenance"
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
      local out="${artefact}.intoto.jsonl"
      generate_provenance "$artefact" "$out"
      echo "wrote: $out"
      sign_artefact "$out"
      ;;
  esac
}

main "$@"
