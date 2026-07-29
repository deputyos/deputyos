#!/usr/bin/env bash
# scripts/update-gguf-shas.sh — download airgap GGUFs and refresh the pinned
# SHA256 hashes in roles/deputyos/vars/llm-airgap.yml.
#
# Usage:
#   scripts/update-gguf-shas.sh              # all tiers
#   scripts/update-gguf-shas.sh rich         # specific tier only
#   scripts/update-gguf-shas.sh --dry-run    # print what would change
#
# Requires: curl, sha256sum, python3 (for YAML rewriting).

set -euo pipefail

dry_run=0
tier_filter="${1:-}"

if [[ "$tier_filter" == "--dry-run" ]]; then
  dry_run=1
  tier_filter="${2:-}"
fi

repo_root="$(cd "$(dirname "$0")/.." && pwd)"
vars_file="${repo_root}/roles/deputyos/vars/llm-airgap.yml"
cache_dir="${repo_root}/build/staging/llm"

if [[ ! -f "$vars_file" ]]; then
  echo "error: ${vars_file} not found" >&2
  exit 1
fi

mkdir -p "$cache_dir"

# Parse the YAML and collect: tier, id, url, filename, sha256_line_number
# We use a simple line-based approach since the schema is stable.
echo "==> parsing ${vars_file} for GGUF URLs and pinned SHA256 values"

declare -a tiers=()
declare -a ids=()
declare -a filenames=()
declare -a urls=()
declare -a sha_lines=()

current_tier=""
line_num=0
while IFS= read -r line; do
  line_num=$((line_num + 1))
  # Detect tier header.
  if [[ "$line" =~ ^[[:space:]]*(lean|standard|rich):[[:space:]]*$ ]]; then
    current_tier="${BASH_REMATCH[1]}"
    continue
  fi
  # Stop collecting if we hit a non-tier top-level key.
  [[ "$line" =~ ^[a-z] ]] && current_tier=""

  if [[ -n "$current_tier" ]]; then
    if [[ "$line" =~ ^[[:space:]]*-[[:space:]]*id:[[:space:]]*\"(.+)\"$ ]]; then
      tiers+=("$current_tier")
      ids+=("${BASH_REMATCH[1]}")
    fi
    if [[ "$line" =~ filename:[[:space:]]*\"(.+)\"$ ]]; then
      filenames+=("${BASH_REMATCH[1]}")
    fi
    if [[ "$line" =~ url:[[:space:]]*\"(.+)\"$ ]]; then
      urls+=("${BASH_REMATCH[1]}")
    fi
    if [[ "$line" =~ sha256:[[:space:]]*\"(.+)\"$ ]]; then
      sha_lines+=("$line_num")
    fi
  fi
done <"$vars_file"

echo "==> found ${#ids[@]} models across ${tiers[*]} tiers"

for i in "${!ids[@]}"; do
  tier="${tiers[$i]}"
  id="${ids[$i]}"
  file="${filenames[$i]}"
  url="${urls[$i]}"
  sha_line="${sha_lines[$i]}"

  # Filter by tier if requested.
  if [[ -n "$tier_filter" && "$tier" != "$tier_filter" ]]; then
    continue
  fi

  dest="${cache_dir}/${file}"
  echo ""
  echo "--- ${tier}/${id} ---"

  # Download if not cached.
  if [[ ! -f "$dest" ]]; then
    echo "  downloading: ${url}"
    if [[ $dry_run -eq 1 ]]; then
      echo "  [dry-run] would download ${file}"
      continue
    fi
    if ! curl -fsSL --retry 3 --connect-timeout 30 --max-time 1800 \
         -o "${dest}.partial" "$url"; then
      echo "  [error] download failed; skipping" >&2
      rm -f "${dest}.partial"
      continue
    fi
    mv "${dest}.partial" "$dest"
  else
    echo "  using cached: ${dest}"
  fi

  sha=$(sha256sum "$dest" | awk '{print $1}')
  echo "  sha256: ${sha}"

  if [[ $dry_run -eq 1 ]]; then
    echo "  [dry-run] would set sha256 to ${sha} at line ${sha_line}"
    continue
  fi

  # Replace the SHA line in the YAML.
  # The line format is: `      sha256: "<64 lowercase hex characters>"`.
  sed -i "${sha_line}s/sha256: \".*\"/sha256: \"${sha}\"/" "$vars_file"
  echo "  [updated] line ${sha_line} → ${sha}"
done

echo ""
echo "==> done. ${vars_file} updated."
echo "    Review the changes with: git diff ${vars_file}"
