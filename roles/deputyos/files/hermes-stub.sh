#!/usr/bin/env bash
# Stub Hermes Agent — replaces the real binary when the bake-time install
# couldn't reach PyPI. Boots an HTTP server on 127.0.0.1:8080 that returns
# a healthz response so `deputyctl doctor` can pass and the smoke harness
# can assert the unit is "active (running)".
#
# Contract:
#   * `hermes gateway start` — listen on $HERMES_PORT (default 8080) until
#     SIGTERM/SIGINT. Always responds 200 with a JSON body advertising
#     `stub:true` so callers know they're talking to the fallback.
#   * `hermes serve [--daemon]` — alias for `gateway start`, kept so legacy
#     entrypoints (the old M2 spec) still work.
#   * `hermes --version` / `hermes version` — print the stub version, exit 0.
#   * Any other invocation — exit 64 (EX_USAGE) so misuse is loud.
#
# Lane F owns this. Remove (or gate behind HERMES_FORCE_STUB) once the real
# `pip install hermes-agent==<pinned>` is reliable in the bake environment.
#
# Hermes runs on port 8080 in production (see profiles/hermes.toml [health]).
# OpenClaw also defaults to 8080; only one profile is active per image, so
# the collision is not a runtime issue. For local dev where both stubs run
# side by side, override via $HERMES_PORT / $OPENCLAW_PORT.

set -euo pipefail

STUB_VERSION="stub-0.1"

case "${1:-}" in
  gateway)
    shift
    if [[ "${1:-}" == "start" ]]; then
      shift
    fi
    ;;
  serve)
    shift
    if [[ "${1:-}" == "--daemon" ]]; then
      shift
    fi
    ;;
  --version | version)
    echo "hermes ${STUB_VERSION}"
    exit 0
    ;;
  "")
    echo "hermes stub: usage: hermes gateway start" >&2
    exit 64
    ;;
  *)
    echo "hermes stub: unknown subcommand: $1" >&2
    exit 64
    ;;
esac

PORT="${HERMES_PORT:-8080}"
BIND="${HERMES_BIND:-127.0.0.1}"
BODY="{\"healthz\":\"ok\",\"stub\":true,\"version\":\"${STUB_VERSION}\",\"model\":\"stub\"}"
CLEN=${#BODY}
RESPONSE="HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: ${CLEN}\r\nConnection: close\r\n\r\n${BODY}"

shutdown() {
  echo "hermes stub: shutting down" >&2
  exit 0
}
trap shutdown TERM INT

echo "hermes stub: listening on ${BIND}:${PORT}" >&2

# Single-shot listener loop. nc.openbsd's -q 0 closes after EOF on stdin so
# the loop body returns and we accept the next connection. Failures are
# tolerated (e.g. EADDRINUSE on the same boot) so the unit doesn't crash-loop
# faster than systemd's RestartSec.
while true; do
  printf '%b' "${RESPONSE}" | nc.openbsd -l -s "${BIND}" -p "${PORT}" -q 0 >/dev/null 2>&1 || sleep 1
done
