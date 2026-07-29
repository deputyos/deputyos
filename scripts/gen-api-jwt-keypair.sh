#!/usr/bin/env bash
# scripts/gen-api-jwt-keypair.sh — generate the deputyOS API JWT RSA keypair.
#
# The API (api.deputyos.com) signs account/access JWTs with an RSA private key
# and the appliance wizard verifies them with the matching public key. One
# keypair, two placements:
#   - PRIVATE half → the API's JWT_PRIVATE_KEY env (CapRover dashboard). SECRET.
#   - PUBLIC  half → the API's JWT_PUBLIC_KEY env  (CapRover dashboard) AND baked
#                    into images at /etc/deputyos/api-pubkey.pem. Non-secret.
#
# This script generates the pair, installs the PUBLIC key where scripts/build.sh
# picks it up automatically (~/.config/deputyos/api-pubkey.pem, override with
# DEPUTYOS_API_PUBKEY_FILE), and prints both PEMs so you can paste them into the
# CapRover dashboard. The PRIVATE key is written 0600 and is NEVER committed or
# baked — keep it safe (a password manager / secrets vault).
#
# Usage:
#   scripts/gen-api-jwt-keypair.sh [--force]
#
# Idempotent: refuses to overwrite an existing private key unless --force
# (regenerating invalidates every already-issued token + every baked image's
# trust of the API).

set -euo pipefail

force=0
[[ "${1:-}" == "--force" ]] && force=1

key_dir="${DEPUTYOS_KEY_DIR:-${HOME}/.config/deputyos}"
priv="${key_dir}/api-jwt-private.pem"
pub="${DEPUTYOS_API_PUBKEY_FILE:-${key_dir}/api-pubkey.pem}"

command -v openssl >/dev/null 2>&1 || { echo "error: openssl not found" >&2; exit 1; }

mkdir -p "$key_dir"
chmod 700 "$key_dir"

if [[ -f "$priv" && "$force" -ne 1 ]]; then
  echo "error: private key already exists at ${priv}" >&2
  echo "  Regenerating invalidates all issued tokens + every baked image's API trust." >&2
  echo "  Re-run with --force only if you intend that (then redeploy the API AND rebuild images)." >&2
  exit 1
fi

echo "==> generating RSA-2048 keypair"
openssl genrsa -out "$priv" 2048 2>/dev/null
chmod 600 "$priv"
openssl rsa -in "$priv" -pubout -out "$pub" 2>/dev/null
chmod 644 "$pub"

echo "==> private key: ${priv} (0600 — SECRET, do not commit/bake)"
echo "==> public  key: ${pub} (0644 — build.sh stages this into images)"
echo

cat <<EOF
────────────────────────────────────────────────────────────────────────────
Next steps (nothing here is committed to git):

1) CapRover dashboard → app 'deputyos-api' → Environmental Variables:

   JWT_PRIVATE_KEY  = (paste the FULL contents of ${priv}, incl. the
                       -----BEGIN/END----- lines and real newlines)

   JWT_PUBLIC_KEY   = (paste the FULL contents of ${pub})

2) Image builds pick up the public key automatically — scripts/build.sh stages
   ${pub}
   into /etc/deputyos/api-pubkey.pem on the next bake. Override the source path
   with DEPUTYOS_API_PUBKEY_FILE if you keep it elsewhere.

To print the PEMs now:   cat ${priv}   and   cat ${pub}
────────────────────────────────────────────────────────────────────────────
EOF
