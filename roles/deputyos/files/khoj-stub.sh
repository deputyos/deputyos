#!/usr/bin/env bash
# Stub Khoj — replaces the real binary when the bake-time install couldn't
# reach PyPI. Boots an HTTP server on 127.0.0.1:42110 that returns a healthz
# response so `deputyctl doctor` can pass and the smoke harness can assert
# the unit is "active (running)".
#
# Contract:
#   * `khoj --no-gui --host <addr> --port <port>` — listen on $KHOJ_PORT
#     (default 42110) until SIGTERM/SIGINT. Always responds 200 with a JSON
#     body advertising `stub:true` so callers know they're talking to the
#     fallback. Argument flags --host and --port override the env defaults.
#   * `khoj --version` / `khoj version` — print the stub version, exit 0.
#   * Any other invocation — exit 64 (EX_USAGE) so misuse is loud.
#
# Lane F owns this. Remove (or gate behind KHOJ_FORCE_STUB) once the real
# `pip install khoj==<pinned>` is reliable in the bake environment.
#
# Khoj runs on port 42110 in production (see profiles/khoj.toml [health]).

set -euo pipefail

STUB_VERSION="stub-0.1"

PORT="${KHOJ_PORT:-42110}"
BIND="${KHOJ_BIND:-127.0.0.1}"

# Parse Khoj-style flags. We accept (and ignore most of) the real CLI surface
# so the systemd entrypoint works unchanged.
saw_subcommand=0
while [[ $# -gt 0 ]]; do
  case "$1" in
    --version | version)
      echo "khoj ${STUB_VERSION}"
      exit 0
      ;;
    --no-gui | --anonymous-mode | --non-interactive)
      saw_subcommand=1
      shift
      ;;
    --host)
      BIND="${2:-$BIND}"
      saw_subcommand=1
      shift 2
      ;;
    --host=*)
      BIND="${1#--host=}"
      saw_subcommand=1
      shift
      ;;
    --port)
      PORT="${2:-$PORT}"
      saw_subcommand=1
      shift 2
      ;;
    --port=*)
      PORT="${1#--port=}"
      saw_subcommand=1
      shift
      ;;
    --help | -h)
      echo "khoj stub: usage: khoj --no-gui --host <addr> --port <port>"
      exit 0
      ;;
    -*)
      # Unknown flag — accept silently so future Khoj flags don't break us.
      saw_subcommand=1
      shift
      ;;
    *)
      echo "khoj stub: unknown argument: $1" >&2
      exit 64
      ;;
  esac
done

if [[ "${saw_subcommand}" -eq 0 ]]; then
  # Bare `khoj` is the real CLI's interactive mode; for the stub we still
  # serve healthz so systemd ExecStart with no flags would also work.
  :
fi

BODY="{\"healthz\":\"ok\",\"stub\":true,\"version\":\"${STUB_VERSION}\",\"model\":\"stub\"}"
CLEN=${#BODY}
RESPONSE="HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: ${CLEN}\r\nConnection: close\r\n\r\n${BODY}"

shutdown() {
  echo "khoj stub: shutting down" >&2
  exit 0
}
trap shutdown TERM INT

echo "khoj stub: listening on ${BIND}:${PORT}" >&2

# Single-shot listener loop. nc.openbsd's -q 0 closes after EOF on stdin so
# the loop body returns and we accept the next connection. Failures are
# tolerated (e.g. EADDRINUSE on the same boot) so the unit doesn't crash-loop
# faster than systemd's RestartSec.
while true; do
  printf '%b' "${RESPONSE}" | nc.openbsd -l -s "${BIND}" -p "${PORT}" -q 0 >/dev/null 2>&1 || sleep 1
done
