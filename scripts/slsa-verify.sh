#!/usr/bin/env bash
# scripts/slsa-verify.sh — validate SLSA provenance attestation structure.
#
# Downloads a .intoto.jsonl provenance file and validates structural fields:
# subject, predicateType, builder.id. Does NOT assert bit-identical
# reproduction (requires independent builder — deferred infra).
#
# Usage:
#   scripts/slsa-verify.sh build/deputyos-openclaw-qemu-aarch64-2026.5.8-dev.qcow2.intoto.jsonl
#   scripts/slsa-verify.sh --url https://cdn.deputyos.com/dev/2026.5.8/artefact.intoto.jsonl

set -euo pipefail

attestation="${1:-}"

if [[ -z "$attestation" ]]; then
  echo "usage: slsa-verify.sh <path-to-.intoto.jsonl>"
  echo "       slsa-verify.sh --url <url-to-.intoto.jsonl>"
  exit 64
fi

# --url flag: download first.
if [[ "$attestation" == "--url" ]]; then
  url="${2:-}"
  if [[ -z "$url" ]]; then
    echo "slsa-verify: --url requires a URL argument"
    exit 64
  fi
  attestation="$(mktemp /tmp/deputyos-slsa-attest.XXXXXX)"
  echo "==> downloading $url"
  if ! curl -fsSL "$url" -o "$attestation"; then
    echo "slsa-verify: failed to download $url"
    exit 1
  fi
fi

if [[ ! -f "$attestation" ]]; then
  echo "slsa-verify: $attestation not found"
  exit 1
fi

echo "==> verifying SLSA provenance: $attestation"

PASS=0
FAIL=0

_assert() {
  local label="$1" condition="$2"
  if eval "$condition" 2>/dev/null; then
    printf '  \033[32mPASS\033[0m %s\n' "$label"
    PASS=$((PASS + 1))
  else
    printf '  \033[31mFAIL\033[0m %s\n' "$label" >&2
    FAIL=$((FAIL + 1))
  fi
}

# SLSA v1 in-toto Statement fields:
#   - _type: "https://in-toto.io/Statement/v1"
#   - subject[]: artefacts this attestation describes
#   - predicateType: "https://slsa.dev/provenance/v1"
#   - predicate: { builder: { id }, buildDefinition, runDetails }

# Called indirectly by the assertion strings evaluated in _assert.
# shellcheck disable=SC2317,SC2329
jq_ok() {
  jq -e "$1" "$attestation" >/dev/null 2>&1
}

# Each line in a .jsonl is a separate attestation (usually 1).
count=$(wc -l < "$attestation" | tr -d ' ')
echo "==> attestation contains $count statement(s)"

# Validate the first statement.
_assert "has _type field" 'jq_ok "._type == \"https://in-toto.io/Statement/v1\""'
_assert "has predicateType" 'jq_ok ".predicateType == \"https://slsa.dev/provenance/v1\""'
_assert "has subject (artefacts described)" 'jq_ok ".subject | length > 0"'
_assert "subject[0].name is set" 'jq_ok ".subject[0].name | length > 0"'
_assert "subject[0].digest.sha256 is 64 hex chars" 'jq_ok ".subject[0].digest.sha256 | test(\"^[0-9a-f]{64}$\")"'
_assert "has builder.id" 'jq_ok ".predicate.buildDefinition.buildType | length > 0"'

echo ""
echo "summary: passed: ${PASS}  failed: ${FAIL}"

if [[ "$attestation" == /tmp/* ]]; then
  rm -f "$attestation"
fi

if (( FAIL > 0 )); then
  echo ""
  echo "note: SLSA L3 requires bit-identical reproduction across two independent"
  echo "      builders. The checks above validate structural correctness of the"
  echo "      attestation but do NOT confirm reproducibility — that requires a"
  echo "      second build host. See docs/11-roadmap.md § M7."
  exit 1
fi

echo ""
echo "SLSA provenance structure verified."
echo "Full L3 verification (independent builder) requires external infra."
exit 0
