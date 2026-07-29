#!/usr/bin/env bash
# scripts/desktop-local.sh — run the full deputyOS stack locally: a Docker
# "remote" (release CDN + accounts/tunnel/backup API) that the desktop
# installer pulls from, boots a real qemu-x86_64 VM, and whose in-VM agent
# talks to the local Docker API instead of api.deputyos.com.
#
# This script is the "wire the booted image at the local API" half of the loop.
# It assumes you already ran the build half:
#
#   make desktop-local-build   # build qcow2 + launcher, sign, manifest, publish
#   make cdn-up                # docker compose up cdn api www
#
# Then:
#
#   make desktop-local         # runs this script → install + start the VM
#
# What it does:
#   1. Builds a NoCloud cloud-init seed ISO whose write_files drops a systemd
#      DefaultEnvironment=DEPUTYOS_API_BASE=http://10.0.2.2:3000 (so the
#      tunnel + wizard — which don't source secrets.env — still hit the local
#      API) and the same line into /etc/deputyos/secrets.env (so the gateways,
#      which DO source it, pick it up). 10.0.2.2 is the qemu user-net host
#      gateway — i.e. "the host running docker" from inside the VM.
#   2. Points the launcher at the local CDN manifest + the dev minisign pubkey.
#   3. Runs `deputyos-desktop install && deputyos-desktop start`, attaching the
#      seed so the booted image is pre-provisioned with the local API base.
#
# Override the API target via DEPUTYOS_LOCAL_API (default http://10.0.2.2:3000).
# The wizard opens at http://localhost:${DEPUTYOS_DESKTOP_WIZARD_PORT:-7088}
# once the VM is up — the loop uses the 7000 series for host-side port forwards
# to dodge collisions with stale processes dev hosts often have on 8088/8080.
# The in-VM wizard still listens on :8088; only the HOST forward moves.
set -euo pipefail

repo_root="$(cd "$(dirname "$0")/.." && pwd)"
cd "$repo_root"

# ---- config (env-overridable) ----
manifest_url="${DEPUTYOS_DESKTOP_MANIFEST_URL:-http://localhost:8090/manifest.json}"
pubkey="${DEPUTYOS_DESKTOP_PUBKEY:-$HOME/.config/deputyos/dev-keys/deputyos-dev.pub}"
launcher="${DEPUTYOS_DESKTOP_BIN:-target/release/deputyos-desktop}"
api_base="${DEPUTYOS_LOCAL_API:-http://10.0.2.2:3000}"
seed_iso="${DEPUTYOS_DESKTOP_SEED_ISO:-build/desktop-local-seed.iso}"
# Which agent profile to install. The local CDN manifest aggregates every
# profile built under this release version, so it usually carries more than
# one image for qemu-x86_64; `deputyos-desktop install` then refuses to guess
# and requires --profile. Default to the PROFILE make variable (set by the
# desktop-local / desktop-local-build targets). Override with
# DEPUTYOS_DESKTOP_PROFILE; set to empty to install into a single-image manifest.
profile="${DEPUTYOS_DESKTOP_PROFILE:-${PROFILE:-}}"
# Host-side qemu forwards (guest wizard is on :8088, chat/relay on :8080).
# Defaults are the 7000 series so the loop doesn't fight whatever else a dev
# host has grabbed on 8088/8080.
wizard_port="${DEPUTYOS_DESKTOP_WIZARD_PORT:-7088}"
gateway_port="${DEPUTYOS_DESKTOP_GATEWAY_PORT:-7080}"
seed_dir="$(mktemp -d -t deputyos-desktop-local-seed.XXXXXX)"
trap 'rm -rf "$seed_dir"' EXIT

# ---- prereqs ----
[[ -x "$launcher" ]] || { echo "error: launcher not built at $launcher; run 'make desktop-local-build' first" >&2; exit 1; }
[[ -f "$pubkey" ]]    || { echo "error: dev pubkey missing at $pubkey; run 'make sign-dev' first" >&2; exit 1; }

# Pick the ISO builder (same ladder as test/smoke/_common.sh::smoke_generate_seed).
iso_builder=""
for c in cloud-localds genisoimage xorriso; do
  if command -v "$c" >/dev/null 2>&1; then iso_builder="$c"; break; fi
done
[[ -n "$iso_builder" ]] || { echo "error: need one of cloud-localds, genisoimage, xorriso to build the cloud-init seed" >&2; exit 1; }

# ---- cloud-init seed ----
# instance-id is VERSIONED: bump it whenever the seed's write_files/runcmd
# changes so cloud-init re-runs them on the next boot (with a stable
# instance-id cloud-init treats write_files as first-boot-only and won't
# re-apply). v2 adds the deputywizard --no-token drop-in; v3 opens UFW for
# the qemu user-net (enp0s2 / 10.0.2.0/24) to the wizard + relay ports —
# the baked ufw profile restricts the wizard to "lo only", which blocks
# the host's hostfwd traffic (host 10.0.2.2 → guest :8088).
cat >"$seed_dir/meta-data" <<EOF
instance-id: deputyos-desktop-local-v3
local-hostname: deputyos-local
EOF

# DefaultEnvironment reaches services that DON'T source secrets.env
# (deputyos-tunnel, deputywizard). secrets.env reaches the gateways that do
# (EnvironmentFile=-/etc/deputyos/secrets.env). runcmd reloads + restarts so
# already-booted services pick up the new env on first boot.
#
# The deputywizard drop-in switches the wizard to --no-token (AuthMode::None)
# so the launcher's bare http://localhost:<wizard_port> URL loads the wizard
# directly — the baked unit runs --production (single-use token auth), which
# would otherwise serve page_unauthorized() to the launcher's tokenless URL.
# --production is kept (apply mode that writes /etc); only the auth gate is
# dropped, which is fine for a local dev loop. The drop-in re-specifies
# ExecStart because systemd drop-ins can't append to ExecStart — they must
# clear it (ExecStart=) then reset it.
cat >"$seed_dir/user-data" <<EOF
#cloud-config
write_files:
  - path: /etc/systemd/system.conf.d/10-deputyos-api-base.conf
    permissions: '0644'
    content: |
      [Manager]
      DefaultEnvironment=DEPUTYOS_API_BASE=${api_base}
  - path: /etc/deputyos/secrets.env
    permissions: '0600'
    content: |
      DEPUTYOS_API_BASE=${api_base}
  - path: /etc/systemd/system/deputywizard.service.d/no-token.conf
    permissions: '0644'
    content: |
      [Service]
      ExecStart=
      ExecStart=/usr/local/bin/deputywizard serve --port 8088 --bind 0.0.0.0 --no-token --production
runcmd:
  - systemctl daemon-reload
  # The baked ufw app profile permits the wizard (8088) + relay (8080) on
  # loopback only, so the host's qemu hostfwd traffic — arriving on the
  # user-net NIC (enp0s2, from 10.0.2.2) — is dropped ([UFW BLOCK] DPT=8088).
  # Open those two ports on the user-net interface so the host (and the
  # qemu user-net subnet) can reach the wizard + relay. '|| true' keeps
  # runcmd green if ufw isn't installed/enabled on a given image.
  - ufw allow in on enp0s2 to any port 8088 proto tcp || true
  - ufw allow in on enp0s2 to any port 8080 proto tcp || true
  - ufw reload || true
  - systemctl try-restart deputyos-tunnel deputywizard openclaw-gateway hermes-gateway deputyos-voice-relay khoj-gateway 2>/dev/null || true
EOF

echo "==> building cloud-init seed ($iso_builder) pointing the agent at $api_base"
case "$iso_builder" in
  cloud-localds) cloud-localds "$seed_iso" "$seed_dir/user-data" "$seed_dir/meta-data" ;;
  genisoimage)   genisoimage -quiet -output "$seed_iso" -volid cidata -joliet -rock "$seed_dir/user-data" "$seed_dir/meta-data" ;;
  xorriso)       xorriso -as mkisofs -quiet -V cidata -o "$seed_iso" -J -r "$seed_dir/user-data" "$seed_dir/meta-data" ;;
esac

# ---- run the installer against the local CDN ----
mkdir -p build
export DEPUTYOS_DESKTOP_MANIFEST_URL="$manifest_url"
export DEPUTYOS_DESKTOP_PUBKEY="$pubkey"
export DEPUTYOS_DESKTOP_SEED_ISO="$seed_iso"
# 7000-series host forwards (read by the launcher's config::wizard_host_port
# / gateway_host_port → qemu hostfwd). The launcher opens the wizard URL at
# http://localhost:<wizard_port>, which matches the forward.
export DEPUTYOS_DESKTOP_WIZARD_PORT="$wizard_port"
export DEPUTYOS_DESKTOP_GATEWAY_PORT="$gateway_port"

echo "==> deputyos-desktop install (manifest=$manifest_url, pubkey=$pubkey, profile=${profile:-<auto>})"
# `install` bails with a numbered list when the manifest offers more than one
# image for this target and --profile is omitted; pass --profile when the loop
# knows which one it's exercising.
if [[ -n "$profile" ]]; then
  "$launcher" install --profile "$profile"
else
  "$launcher" install
fi

echo "==> deputyos-desktop start (seed=$seed_iso, api=$api_base, wizard=http://localhost:$wizard_port)"
"$launcher" start

echo
echo "==> wizard at http://localhost:$wizard_port  (agent API base: $api_base)"
echo "    first boot takes ~30-60s (cloud-init + wizard restart into --no-token);"
echo "    if you see 'Unauthorized', wait a few seconds and refresh."
echo "==> logs: docker compose -f docker-compose.dev.yml logs -f api"
echo "==> stop:  make cdn-down  &&  $launcher stop"
