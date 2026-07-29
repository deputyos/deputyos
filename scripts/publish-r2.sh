#!/usr/bin/env bash
# scripts/publish-r2.sh — backward-compat shim.
#
# The artefact CDN moved from Cloudflare R2 to Backblaze B2 (fronted by
# Cloudflare). The publisher is now backend-agnostic in scripts/publish-cdn.sh.
# This shim keeps the old entrypoint working: it forwards to publish-cdn.sh,
# defaulting the remote to R2 (`r2:cdn-deputyos-com`) so an existing R2 rclone
# setup still works unchanged. New setups should use `make publish-cdn`
# (default `b2:cdn-deputyos-com`).

set -euo pipefail
repo_root="$(cd "$(dirname "$0")/.." && pwd)"
export DEPUTYOS_CDN_REMOTE="${DEPUTYOS_CDN_REMOTE:-${DEPUTYOS_R2_REMOTE:-r2:cdn-deputyos-com}}"
exec bash "${repo_root}/scripts/publish-cdn.sh"
