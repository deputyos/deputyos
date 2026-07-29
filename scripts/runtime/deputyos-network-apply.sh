#!/bin/sh
# deputyos-network-apply.sh — boot-time egress policy re-applier (M5.5).
#
# Installed as the ExecStart of deputyos-network-apply.service. Re-renders
# /etc/nftables.conf from /etc/deputyos/network-policy.json by shelling to
# `deputyctl network apply` — but ONLY when the policy mode is not `open`.
#
# Why the open-mode skip: `deputyctl network apply` regenerates /etc/nftables.conf
# beginning with `flush ruleset`, which is load-bearing for airgap/whitelist
# (it wipes any runtime-injected allow rule so the on-disk policy is the sole
# durable input) but would, in `open` mode, also wipe ufw's *inbound* default-deny
# rules on a connected image. Open mode has no egress posture to self-heal, so we
# leave the running ruleset untouched and let ufw own inbound. This keeps the
# boot oneshot always-on (so whitelist/airgap self-heal and DNS re-pins) without
# regressing connected images.
set -eu

POLICY="${DEPUTYOS_NETWORK_POLICY:-/etc/deputyos/network-policy.json}"

mode() {
    # Print the policy mode, defaulting to "open" if the file/field is absent.
    if [ ! -r "$POLICY" ]; then
        echo open
        return
    fi
    # jq if available; fall back to a grep (mode is a simple string field).
    if command -v jq >/dev/null 2>&1; then
        jq -r '.mode // "open"' "$POLICY" 2>/dev/null || echo open
    else
        grep -Eo '"mode"[[:space:]]*:[[:space:]]*"[^"]+"' "$POLICY" \
            | head -1 | sed -E 's/.*"([^"]+)"$/\1/' || echo open
    fi
}

case "$(mode)" in
    open)
        echo "deputyos-network-apply: mode=open, nothing to apply (ufw owns inbound)"
        ;;
    whitelist|airgap)
        echo "deputyos-network-apply: mode=$(mode), re-applying from $POLICY"
        exec /usr/local/bin/deputyctl network apply
        ;;
    *)
        echo "deputyos-network-apply: unknown mode \"$(mode)\", skipping" >&2
        ;;
esac