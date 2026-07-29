#!/usr/bin/env bash
# Managed by deputyos. Example hook for /etc/deputyos/hooks.d/update-applied/.
#
# Convention:
#   * Files in /etc/deputyos/hooks.d/<event>/ are executed in
#     lexical order whenever <event> fires.
#   * Each hook reads a single JSON event payload from stdin.
#   * Exit 0 means success; non-zero is logged and (for non-blocking
#     events) ignored.
#   * Hooks shipped by the role end in `.disabled` and are NOT
#     executable; rename to remove the suffix and `chmod +x` to opt in.
#
# Available events (M2):
#   * pre-message      JSON: { "channel": "...", "user": "...", "text": "..." }
#   * post-message     JSON: { "channel": "...", "ms": 1234, "tokens": {...} }
#   * cost-alert       JSON: { "provider": "...", "spent_usd": 1.23, ... }
#   * update-applied   JSON: { "from": "2026.4.20", "to": "2026.4.27", ... }
#
# This example reads the JSON, logs a one-liner to syslog, and exits 0.
# It is wired up at /etc/deputyos/hooks.d/update-applied/00-example.sh.disabled.

set -euo pipefail

payload="$(cat)"
logger -t deputyos-hook "update-applied: ${payload}"
exit 0
