#!/usr/bin/env bash
# test/smoke/_common.sh — shared smoke harness pieces.
#
# Sourced by test/smoke/qemu-aarch64.sh and qemu-x86_64.sh. Covers
# everything that is target-agnostic: cloud-init seed generation,
# SSH-wait, assertion ladder, summary.
#
# Per-target script responsibilities:
#   - Set: TARGET, repo_root, artefact, cache_dir, seed_iso, ssh_key,
#          qemu_pidfile, serial_log, SMOKE_LEVEL, PROFILE
#   - Pick UEFI firmware and machine flags
#   - Launch qemu-system-<arch> with -daemonize -pidfile, forwarding
#     :2222->:22 and :8088->:8088
#   - After launch, call: smoke_wait_for_ssh && smoke_run_assertions
#
# This file is shellcheck-clean (bash 5+); no associative arrays.

# shellcheck shell=bash
# Variables (cache_dir, seed_iso, ssh_key, serial_log, qemu_pidfile, qga_socket,
# TARGET, PROFILE, SMOKE_LEVEL) are exported by the sourcing per-target
# script before this file is sourced. Silence shellcheck's unassigned
# warnings here once.
# shellcheck disable=SC2154

# ---- cloud-init seed generation ----
#
# Args: $1 = ssh_key path (without .pub)
# Writes: $seed_iso, $cache_dir/user-data, $cache_dir/meta-data
smoke_generate_seed() {
  local key="$1"
  local pubkey
  if [[ ! -f "$key" ]]; then
    ssh-keygen -t ed25519 -N "" -f "$key" -C "deputyos-smoke" >/dev/null
  fi
  pubkey="$(cat "${key}.pub")"

  cat >"${cache_dir}/meta-data" <<EOF
instance-id: deputyos-smoke
local-hostname: deputyos-smoke
EOF

  cat >"${cache_dir}/user-data" <<EOF
#cloud-config
users:
  - name: agent
    sudo: ALL=(ALL) NOPASSWD:ALL
    shell: /bin/bash
    ssh_authorized_keys:
      - ${pubkey}
ssh_pwauth: false
EOF

  if command -v cloud-localds >/dev/null 2>&1; then
    cloud-localds "$seed_iso" "${cache_dir}/user-data" "${cache_dir}/meta-data"
  elif command -v genisoimage >/dev/null 2>&1; then
    genisoimage -quiet -output "$seed_iso" -volid cidata -joliet -rock \
      "${cache_dir}/user-data" "${cache_dir}/meta-data"
  elif command -v xorriso >/dev/null 2>&1; then
    xorriso -as mkisofs -quiet -V cidata -o "$seed_iso" -J -r \
      "${cache_dir}/user-data" "${cache_dir}/meta-data"
  else
    echo "error: need one of cloud-localds, genisoimage, xorriso to build cloud-init seed" >&2
    return 1
  fi
}

# ---- cleanup trap ----
smoke_cleanup() {
  if [[ -f "${qemu_pidfile:-}" ]]; then
    local pid
    pid="$(cat "$qemu_pidfile" 2>/dev/null || true)"
    if [[ -n "$pid" ]]; then
      kill "$pid" 2>/dev/null || true
      sleep 1
      kill -9 "$pid" 2>/dev/null || true
    fi
  fi
  rm -f "${seed_iso:-}" "${cache_dir:-}/user-data" "${cache_dir:-}/meta-data" \
    "${qemu_pidfile:-}" "${qga_socket:-}"
}

# ---- SSH wait ----
smoke_wait_for_ssh() {
  echo "info: waiting up to 180s for SSH on :2222"
  local deadline booted
  deadline=$(( $(date +%s) + 180 ))
  booted=0
  while (( $(date +%s) < deadline )); do
    if ssh -o StrictHostKeyChecking=no -o UserKnownHostsFile=/dev/null \
           -o ConnectTimeout=3 -o BatchMode=yes -o LogLevel=ERROR \
           -i "$ssh_key" -p 2222 agent@127.0.0.1 true 2>/dev/null; then
      booted=1
      break
    fi
    sleep 3
  done

  if (( booted == 0 )); then
    echo "FAIL: SSH never came up on :2222 within 180s" >&2
    echo "---- last 60 lines of serial log ----" >&2
    tail -60 "$serial_log" >&2 || true
    return 1
  fi
}

# ---- SSH helpers (used by run_ssh / run_sudo / assert) ----
smoke_run_ssh() {
  ssh -o StrictHostKeyChecking=no -o UserKnownHostsFile=/dev/null \
      -o ConnectTimeout=5 -o BatchMode=yes -o LogLevel=ERROR \
      -i "$ssh_key" -p 2222 agent@127.0.0.1 "$@"
}

smoke_run_sudo() {
  smoke_run_ssh "sudo $*"
}

# ---- assertion harness ----
smoke_run_assertions() {
  local PASS=0 FAIL=0 SKIP=0

  _assert() {
    local label="$1"; shift
    if "$@" >/dev/null 2>&1; then
      printf '  \033[32mPASS\033[0m %s\n' "$label"
      PASS=$((PASS + 1))
    else
      printf '  \033[31mFAIL\033[0m %s\n' "$label" >&2
      FAIL=$((FAIL + 1))
    fi
  }
  _skip() {
    local label="$1"; local reason="$2"
    printf '  \033[33mSKIP\033[0m %s (%s)\n' "$label" "$reason"
    SKIP=$((SKIP + 1))
  }

  echo
  echo "==> TARGET=${TARGET} SMOKE_LEVEL=${SMOKE_LEVEL}"
  local image_kind="${DEPUTYOS_IMAGE_KIND:-official}"

  # ---- scaffold-level (always) ----
  _assert "kernel reached multi-user.target" \
    smoke_run_sudo "systemctl is-system-running --wait | grep -qE '^(running|degraded)\$'"
  _assert "deputyctl binary is executable"     smoke_run_ssh "test -x /usr/local/bin/deputyctl"
  _assert "deputyctl --version exits 0"        smoke_run_ssh "deputyctl --version"
  _assert "image kind marker is correct"       smoke_run_ssh "test \"\$(cat /etc/deputyos/image-kind)\" = '${image_kind}'"
  if [[ "$image_kind" == "official" ]]; then
    _assert "deputyd binary is executable"       smoke_run_ssh "test -x /usr/local/bin/deputyd"
    _assert "deputyd service is active"          smoke_run_sudo "systemctl is-active deputyd.service"
    _assert "deputyd socket is owner-only"       smoke_run_sudo "test \"\$(stat -c %a /run/deputyos/deputyd.sock)\" = 600"
    _assert "deputyd health protocol is v2"      smoke_run_sudo "deputyd health | python3 -c 'import json,sys; d=json.load(sys.stdin); assert d[\"kind\"] == \"health\"; assert d[\"report\"][\"protocol\"] == 2; assert d[\"report\"][\"lifecycle\"][\"phase\"] == \"active\"'"
    _assert "backup encryption tool is baked in" smoke_run_ssh "command -v age && command -v age-keygen"
    _assert "object-store client is baked in"    smoke_run_ssh "command -v rclone"
    _assert "resident agent advertises backup"  smoke_run_sudo "deputyd execute --request-json '{\"command\":\"capabilities\"}' | python3 -c 'import json,sys; d=json.load(sys.stdin); assert \"backup.run\" in d[\"report\"][\"commands\"]; assert \"backup.status\" in d[\"report\"][\"commands\"]'"
    _assert "QEMU guest agent is active"         smoke_run_sudo "systemctl is-active qemu-guest-agent.service"
  fi

  # ---- m1 ----
  if [[ "$SMOKE_LEVEL" == "m1" || "$SMOKE_LEVEL" == "full" ]]; then
    _assert "ufw status is active"               smoke_run_sudo "ufw status verbose | grep -q 'Status: active'"
    _assert "ufw default-deny incoming"          smoke_run_sudo "ufw status verbose | grep -q 'Default: deny (incoming)'"
    _assert "kernel.kptr_restrict == 1"          smoke_run_sudo "test \"\$(sysctl -n kernel.kptr_restrict)\" = 1"
    _assert "kernel.dmesg_restrict == 1"         smoke_run_sudo "test \"\$(sysctl -n kernel.dmesg_restrict)\" = 1"
    _assert "vm.swappiness == 10"                smoke_run_sudo "test \"\$(sysctl -n vm.swappiness)\" = 10"
    _assert "AppArmor module enabled"            smoke_run_sudo "aa-enabled"
    _assert "AppArmor enforces at least 1 profile" smoke_run_sudo "apparmor_status | grep -qE 'profiles are in enforce mode'"
    _assert "/etc/deputyos/active-profile == ${PROFILE}" smoke_run_ssh "test \"\$(cat /etc/deputyos/active-profile)\" = '${PROFILE}'"
    _assert "/etc/deputyos/limits.json exists"    smoke_run_ssh "test -f /etc/deputyos/limits.json"
    _assert "/etc/deputyos/limits.json is JSON"   smoke_run_ssh "python3 -c 'import json,sys; json.load(open(\"/etc/deputyos/limits.json\"))'"
    _assert "agent user exists"                  smoke_run_ssh "id agent"
    _assert "/etc/deputyos/secrets.env mode 0600" smoke_run_sudo "test \"\$(stat -c %a /etc/deputyos/secrets.env)\" = 600"
    _assert "fail2ban service enabled"           smoke_run_sudo "systemctl is-enabled fail2ban"
    _assert "zramswap service enabled"           smoke_run_sudo "systemctl is-enabled zramswap"
    _assert "magika is on PATH"                  smoke_run_ssh "command -v magika"
    _assert "deputyctl doctor exits 0"            smoke_run_ssh "deputyctl doctor"
    if [[ "$image_kind" == "official" ]]; then
      _assert "deputyd pause/resume round-trip"    smoke_run_sudo "deputyd prepare-pause >/dev/null && deputyd health | python3 -c 'import json,sys; assert json.load(sys.stdin)[\"report\"][\"lifecycle\"][\"phase\"] == \"quiesced\"' && deputyd resume >/dev/null"
    fi

    # ---- M4.5 airgap assertions (only when airgap baked) ----
    if smoke_run_ssh "test -f /etc/deputyos/airgap.flag"; then
      _assert "nftables is active"                  smoke_run_sudo "systemctl is-active nftables"
      _assert "nftables output policy is drop"      smoke_run_sudo "nft list chain inet deputyos output | grep -q 'policy drop'"
      _assert "airgap models catalog.json exists"   smoke_run_ssh "test -f /opt/deputyos/airgap/models/catalog.json"
      _assert "airgap models catalog is JSON"       smoke_run_ssh "python3 -c 'import json,sys; json.load(open(\"/opt/deputyos/airgap/models/catalog.json\"))'"
      _assert "network-policy.json mode is airgap"  smoke_run_sudo "grep -q '\"mode\".*\"airgap\"' /etc/deputyos/network-policy.json"
      _assert "deputyctl model list includes airgap" smoke_run_ssh "deputyctl model list --json | python3 -c 'import json,sys; d=json.load(sys.stdin); assert d[\"airgap\"] == True'"
    fi
  fi

  # ---- full (deferred to M3+) ----
  if [[ "$SMOKE_LEVEL" == "full" ]]; then
    _skip "wizard healthz on :8088" "lands in M3 (Lane C)"
    _skip "EICAR detection by clamd" "lands in M3"
  fi

  echo
  echo "summary: passed: ${PASS}  failed: ${FAIL}  skipped: ${SKIP}"

  if (( FAIL > 0 )); then
    return 1
  fi
}
