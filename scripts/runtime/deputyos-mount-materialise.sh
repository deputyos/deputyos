#!/usr/bin/env bash
# deputyos-mount-materialise.sh — apply mount policy at boot
#
# Reads /etc/deputyos/mounts-policy.json and creates bind-mounts
# (host-FS), CIFS mounts, and NFS mounts as configured.
# Called by deputyos-mounts.service at boot and on policy changes.

set -euo pipefail

POLICY="${DEPUTYOS_MOUNTS_POLICY:-/etc/deputyos/mounts-policy.json}"
STATE="${DEPUTYOS_MOUNTS_STATE:-/run/deputyos/mount-state.json}"
SECRETS="${DEPUTYOS_MOUNTS_SECRETS:-/etc/deputyos/secrets.env}"
GUEST_ROOT="/mnt/deputyos"

do_unmount=0
if [[ "${1:-}" == "--unmount" ]]; then
  do_unmount=1
fi

if [[ ! -f "$POLICY" ]]; then
  echo "mount-materialise: no policy at $POLICY — nothing to do"
  exit 0
fi

# Quick JSON extraction: jq is preferred; python3 fallback.
_jq() {
  if command -v jq >/dev/null 2>&1; then
    jq -r "$@"
  else
    python3 -c "import json,sys; d=json.load(open('$POLICY')); print(${1//./[}${1//./]})" 2>/dev/null || true
  fi
}

# ---- Unmount mode ----
if [[ $do_unmount -eq 1 ]]; then
  echo "mount-materialise: unmounting all deputyOS mounts"
  mount | grep "on ${GUEST_ROOT}/" | awk '{print $3}' | sort -r | while read -r mp; do
    echo "  umount: $mp"
    umount -l "$mp" 2>/dev/null || true
    rmdir "$mp" 2>/dev/null || true
  done
  exit 0
fi

echo "mount-materialise: applying policy from $POLICY"

mkdir -p "$GUEST_ROOT"
mkdir -p "$(dirname "$STATE")"

declare -a state_entries=()

# ---- Host-FS bind mounts ----
host_count=$(_jq '.host_fs | length' 2>/dev/null || echo 0)
for ((i=0; i<host_count; i++)); do
  id=$(_jq ".host_fs[$i].id")
  host_path=$(_jq ".host_fs[$i].host_path")
  guest_path=$(_jq ".host_fs[$i].guest_path")
  mode=$(_jq ".host_fs[$i].mode")

  [[ -z "$id" || -z "$host_path" || -z "$guest_path" ]] && continue

  mkdir -p "$guest_path" 2>/dev/null || true

  if mountpoint -q "$guest_path" 2>/dev/null; then
    echo "  [skip] $id: already mounted at $guest_path"
  else
    mode_opts="ro"
    [[ "$mode" == "rw" ]] && mode_opts="rw"
    if mount --bind "$host_path" "$guest_path" -o "$mode_opts" 2>/dev/null; then
      echo "  [ok] $id: $host_path → $guest_path ($mode_opts)"
    else
      echo "  [warn] $id: bind mount failed for $host_path → $guest_path" >&2
    fi
  fi
  state_entries+=("{\"id\":\"$id\",\"guest_path\":\"$guest_path\",\"status\":\"mounted\",\"kind\":\"host-fs\"}")
done

# ---- Network mounts (CIFS/NFS) ----
net_count=$(_jq '.network | length' 2>/dev/null || echo 0)
for ((i=0; i<net_count; i++)); do
  id=$(_jq ".network[$i].id")
  kind=$(_jq ".network[$i].kind")
  source=$(_jq ".network[$i].source")
  guest_path=$(_jq ".network[$i].guest_path")
  mode=$(_jq ".network[$i].mode")
  creds_env=$(_jq ".network[$i].credentials_env")

  [[ -z "$id" || -z "$kind" || -z "$source" || -z "$guest_path" ]] && continue

  mkdir -p "$guest_path" 2>/dev/null || true

  if mountpoint -q "$guest_path" 2>/dev/null; then
    echo "  [skip] $id: already mounted at $guest_path"
  else
    mode_opts="ro"
    [[ "$mode" == "rw" ]] && mode_opts="rw"

    if [[ "$kind" == "cifs" ]]; then
      creds_file=""
      if [[ -n "$creds_env" && -f "$SECRETS" ]]; then
        creds_file="$(mktemp /tmp/deputyos-cifs-creds.XXXXXX)"
        grep "^${creds_env}=" "$SECRETS" | cut -d= -f2- | tr -d '"' > "$creds_file" 2>/dev/null || true
        if [[ -s "$creds_file" ]]; then
          credentials_opt="credentials=$creds_file"
        else
          rm -f "$creds_file"
          creds_file=""
        fi
      fi

      if mount -t cifs "$source" "$guest_path" -o "${mode_opts}${creds_file:+,$credentials_opt},iocharset=utf8" 2>/dev/null; then
        echo "  [ok] $id: $source → $guest_path (cifs, $mode_opts)"
      else
        echo "  [warn] $id: CIFS mount failed for $source → $guest_path" >&2
      fi
      rm -f "$creds_file"

    elif [[ "$kind" == "nfs" ]]; then
      if mount -t nfs "$source" "$guest_path" -o "$mode_opts" 2>/dev/null; then
        echo "  [ok] $id: $source → $guest_path (nfs, $mode_opts)"
      else
        echo "  [warn] $id: NFS mount failed for $source → $guest_path" >&2
      fi
    fi
  fi
  state_entries+=("{\"id\":\"$id\",\"guest_path\":\"$guest_path\",\"status\":\"mounted\",\"kind\":\"network/$kind\"}")
done

# ---- Write state ----
echo "[" > "$STATE"
first=1
for entry in "${state_entries[@]}"; do
  [[ $first -eq 1 ]] && first=0 || echo "," >> "$STATE"
  echo -n "  $entry" >> "$STATE"
done
echo "" >> "$STATE"
echo "]" >> "$STATE"

echo "mount-materialise: done — $(echo "${state_entries[@]}" | wc -w) mount(s)"
