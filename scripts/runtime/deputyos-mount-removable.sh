#!/usr/bin/env bash
# /usr/local/lib/deputyos/deputyos-mount-removable.sh
#
# Invoked by udev (99-deputyos-removable.rules) when a USB block device
# appears or disappears. Reads /etc/deputyos/mounts-policy.json and:
#
#   add    — mount under /mnt/deputyos/<label-or-uuid> with safe options
#   remove — umount and rmdir if the dir is empty
#
# Refuses to act if removable.enabled=false in the policy (silent exit 0).
# Refuses LUKS / encrypted volumes (silent exit 0; the user unlocks them
# manually via cryptsetup luksOpen, then re-plugs).

set -eu

action="${1:-}"
kernel_name="${2:-}"

POLICY="/etc/deputyos/mounts-policy.json"
MOUNT_BASE="/mnt/deputyos"
RUN_DIR="/run/deputyos"

mkdir -p "$RUN_DIR"

# No-op if no policy or removable disabled — keep udev quiet on bare images.
if [ ! -f "$POLICY" ]; then
  exit 0
fi
if ! command -v jq >/dev/null 2>&1; then
  echo "deputyos-mount-removable: jq missing; skipping" >&2
  exit 0
fi

enabled="$(jq -r '.removable.enabled // false' "$POLICY" 2>/dev/null || echo false)"
if [ "$enabled" != "true" ]; then
  exit 0
fi

dev="/dev/${kernel_name}"
[ -b "$dev" ] || exit 0

case "$action" in
  add)
    fstype="$(lsblk -no FSTYPE "$dev" 2>/dev/null || true)"
    label="$(lsblk -no LABEL "$dev" 2>/dev/null | tr -d ' ' || true)"
    uuid="$(lsblk -no UUID "$dev" 2>/dev/null || true)"

    if [ -z "$fstype" ] || [ "$fstype" = "crypto_LUKS" ]; then
      echo "deputyos-mount-removable: refusing $dev (fs=$fstype)"
      exit 0
    fi

    name="${label:-${uuid:-${kernel_name}}}"
    target="${MOUNT_BASE}/${name}"
    mkdir -p "$target"

    mode="$(jq -r '.removable.default_mode // "ro"' "$POLICY")"
    safe_opts="$(jq -r '.removable.mount_options_unknown_fs // "nosuid,nodev,noexec"' "$POLICY")"
    case "$fstype" in
      ext4|ext3|ext2|btrfs|xfs)
        opts="${mode},${safe_opts}"
        ;;
      vfat|exfat|ntfs)
        opts="${mode},${safe_opts},uid=0,gid=0"
        ;;
      *)
        opts="${mode},${safe_opts}"
        ;;
    esac

    if mount -t "$fstype" -o "$opts" "$dev" "$target"; then
      jq -n \
        --arg dev "$dev" \
        --arg target "$target" \
        --arg fstype "$fstype" \
        --arg mode "$mode" \
        '{dev:$dev,target:$target,fstype:$fstype,mode:$mode,added_at:(now|todate)}' \
        > "${RUN_DIR}/removable-${kernel_name}.json"
      echo "deputyos-mount-removable: mounted $dev → $target ($fstype, $opts)"
    else
      echo "deputyos-mount-removable: mount failed for $dev" >&2
      rmdir "$target" 2>/dev/null || true
    fi
    ;;
  remove)
    state_file="${RUN_DIR}/removable-${kernel_name}.json"
    if [ -f "$state_file" ]; then
      target="$(jq -r '.target' "$state_file")"
      umount -l "$target" 2>/dev/null || true
      rmdir "$target" 2>/dev/null || true
      rm -f "$state_file"
      echo "deputyos-mount-removable: detached $dev"
    fi
    ;;
  *)
    echo "deputyos-mount-removable: unknown action $action" >&2
    exit 64
    ;;
esac
