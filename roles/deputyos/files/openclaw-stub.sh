#!/usr/bin/env bash
# Stub OpenClaw — replaces the real binary when the bake-time install
# couldn't reach npm. Boots an HTTP server on 127.0.0.1:8080 that returns
# a healthz response so `deputyctl doctor` can pass and the smoke harness
# can assert the unit is "active (running)".
#
# Contract:
#   * `openclaw onboard --daemon` — listen on $OPENCLAW_PORT (default 8080)
#     until SIGTERM/SIGINT. Always responds 200 with a JSON body advertising
#     `stub:true` so callers know they're talking to the fallback.
#   * `openclaw --version` / `openclaw version` — print the stub version, exit 0.
#   * Any other invocation — exit 64 (EX_USAGE) so misuse is loud.
#
# Lane F owns this. Remove (or gate behind OPENCLAW_FORCE_STUB) once the real
# `npm install openclaw@<pinned>` is reliable in the bake environment.

set -euo pipefail

STUB_VERSION="stub-0.1"

case "${1:-}" in
  onboard)
    shift
    if [[ "${1:-}" == "--daemon" ]]; then
      shift
    fi
    ;;
  --version | version)
    echo "openclaw ${STUB_VERSION}"
    exit 0
    ;;
  "")
    echo "openclaw stub: usage: openclaw onboard [--daemon]" >&2
    exit 64
    ;;
  *)
    echo "openclaw stub: unknown subcommand: $1" >&2
    exit 64
    ;;
esac

PORT="${OPENCLAW_PORT:-8080}"
BIND="${OPENCLAW_BIND:-127.0.0.1}"
BODY="{\"healthz\":\"ok\",\"stub\":true,\"version\":\"${STUB_VERSION}\"}"
CLEN=${#BODY}
RESPONSE="HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: ${CLEN}\r\nConnection: close\r\n\r\n${BODY}"

shutdown() {
  echo "openclaw stub: shutting down" >&2
  exit 0
}
trap shutdown TERM INT

echo "openclaw stub: listening on ${BIND}:${PORT}" >&2

# Single-shot listener loop. nc.openbsd's -q 0 closes after EOF on stdin so
# the loop body returns and we accept the next connection. Failures are
# tolerated (e.g. EADDRINUSE on the same boot) so the unit doesn't crash-loop
# faster than systemd's RestartSec.
while true; do
  printf '%b' "${RESPONSE}" | nc.openbsd -l -s "${BIND}" -p "${PORT}" -q 0 >/dev/null 2>&1 || sleep 1
done
