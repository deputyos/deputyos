# Message relay (Unix socket)

The agent processes shipped by deputyOS (OpenClaw — Node, Hermes —
Python, Khoj — Python) live outside the Rust crate. They need to
invoke the [hook dispatcher](../schemas/hook-payloads.md) at message
boundaries — fast, language-agnostically, without paying the cost of
forking `deputyctl` per event.

The **message relay** is the answer: a `SOCK_STREAM` Unix-domain
socket served by `deputyctl` in a hidden mode, speaking
newline-delimited JSON. Sub-millisecond round trip; no per-message
process spawn; any client that can write a JSON line to a Unix socket
works.

The implementation is in
[`deputyctl/src/message_relay.rs`](https://github.com/deputyos/deputyos/blob/main/deputyctl/src/message_relay.rs);
the dispatcher it wraps is in
[`deputyctl/src/hooks.rs`](https://github.com/deputyos/deputyos/blob/main/deputyctl/src/hooks.rs).

[TOC]

## Why a relay?

Three options were considered:

1. **Shell out to `deputyctl message-relay --kind pre-message`** — adds
   fork/exec latency on every message. Unacceptable at chat cadence.
2. **Unix-domain socket; `deputyctl` serves** — sub-ms round trip,
   language-agnostic. *Picked.*
3. **Embed a Rust stub via Node-NAPI / PyO3** — tightest coupling, but
   forces every profile to build native extensions. Rejected.

## Socket location

Default path: `/run/deputyos/relay.sock` (mode `0600`, owned by the
service user).

Resolution order (`message_relay::default_socket_path`):

1. `$DEPUTYOS_RELAY_SOCKET` env var, if set.
2. `/run/deputyos/relay.sock`.

The runner can be told an explicit path via the hidden flag (see
[Hidden runner](#hidden-runner) below).

The parent dir (`/run/deputyos/`) is provisioned by systemd's
`RuntimeDirectory=deputyos` on the gateway units. The relay creates the
socket file with mode `0600` and removes any stale socket file at
`bind()` time.

## Wire protocol

| Aspect | Detail |
|---|---|
| Transport | Unix-domain `SOCK_STREAM` |
| Encoding | Newline-delimited UTF-8 JSON |
| Framing | One line per message |
| Connection lifecycle | One request per connection, then close. The agent reconnects per event (cheap on the same host). |
| Concurrency | One connection at a time, handled inline. No async runtime. If load profile changes, this is the natural place to add a thread pool. |

## Request shape

```json
{"kind":"<HookKind>","payload":<object>}
```

`kind` is one of:

- `pre-message`
- `post-message`
- `cost-alert`
- `update-applied`

Mapping is implemented in `HookKind::parse`. Any other value yields a
parse-error response (see below).

`payload` is an arbitrary JSON object that conforms to the
corresponding shape from the [hook payload schemas](../schemas/hook-payloads.md).
The relay does **not** validate the payload — the dispatcher pipes it
verbatim to each hook script's stdin. If `payload` is omitted, the
dispatcher passes an empty `{}` object.

## Response shape

```json
{
  "ok": <bool>,
  "errors": [
    {
      "script": "<filename>",
      "code": <int>,
      "stderr_tail": "<string>",
      "reason": "<string>"
    }
  ]
}
```

| Field | Type | Always present | Description |
|---|---|---|---|
| `ok` | bool | yes | `true` iff every fired hook script exited zero (or no hooks were installed). |
| `errors` | array | yes | One entry per non-OK script. Empty when `ok=true`. |
| `errors[].script` | string | yes | Filename of the failing script (or `"-"` for parse errors). |
| `errors[].code` | int | for failed exits | Process exit code; omitted (`null`) for spawn errors, timeouts, parse errors. |
| `errors[].stderr_tail` | string | for failed exits | Trailing bytes of stderr (≤ 1024 bytes, UTF-8-lossy). Omitted when empty. |
| `errors[].reason` | string | for non-exit failures | `"timed out (>5s)"`, `"spawn failed: <e>"`, or `"invalid JSON request: …"`. Omitted when empty. |

The `errors[]` array always reflects per-script outcomes in lexical
order (the same order the dispatcher walks the on-disk hooks dir).

## Parse errors

If the request line isn't valid JSON, or `kind` doesn't parse, the
relay returns:

```json
{
  "ok": false,
  "errors": [{"script": "-", "reason": "invalid JSON request: <serde error>"}]
}
```

…and closes the connection.

## Per-kind payload shapes

Cross-reference: full schemas at
[Reference / Schemas / Hook payloads](../schemas/hook-payloads.md).

| Kind | Required fields |
|---|---|
| `pre-message` | `timestamp`, `channel`, `user_id`, `message` |
| `post-message` | `timestamp`, `channel`, `user_id`, `duration_ms` |
| `cost-alert` | `timestamp`, `threshold_usd`, `spent_usd`, `window` |
| `update-applied` | `kind`, `staged_at`, `filename`, `sha256`, `release_version` |

## Timeouts and limits

| Limit | Value | Source |
|---|---|---|
| Per-script timeout | 5 seconds | `hooks::HOOK_TIMEOUT` |
| Max captured stderr | 1024 bytes (tail) | `hooks::STDERR_TAIL_LIMIT` |
| Concurrent connections | 1 (synchronous) | `serve_loop` |
| Bind permissions | mode 0600 | `bind()` |

A hook script that exceeds the timeout is killed and reported as
`{"reason": "timed out (>5s)"}`.

## Hidden runner

The `deputyctl` binary exposes the server via a top-level **hidden
flag** so it does not pollute `--help` and is not part of the frozen
CLI surface (per [`docs/02-profiles.md`](https://github.com/deputyos/deputyos/blob/main/docs/02-profiles.md)
the public surface is 24 frozen subcommands; this is none of them):

```sh
deputyctl --internal-run-relay /run/deputyos/relay.sock
```

The argument is the socket path. Omitting it falls back to the env
var / default. The flag is intended to be invoked by a future
systemd-managed `deputyos-relay.service`; for now the gateway profiles
launch it as a child or rely on the agent process's own connection
shim.

## Connection example: `nc -U`

Drive the relay by hand from a shell:

```sh
$ printf '{"kind":"pre-message","payload":{"timestamp":"2026-04-27T15:30:00Z","channel":"tty","user_id":"me","message":"hello"}}\n' \
  | nc -U /run/deputyos/relay.sock
{"ok":true,"errors":[]}
```

With a hook that fails:

```sh
$ ls /etc/deputyos/hooks.d/cost-alert/
00-fail.sh

$ cat /etc/deputyos/hooks.d/cost-alert/00-fail.sh
#!/bin/sh
echo boom >&2
exit 7

$ printf '{"kind":"cost-alert","payload":{"timestamp":"2026-04-27T22:00:00Z","threshold_usd":5,"spent_usd":4.87,"window":"daily"}}\n' \
  | nc -U /run/deputyos/relay.sock
{"ok":false,"errors":[{"script":"00-fail.sh","code":7,"stderr_tail":"boom"}]}
```

With a malformed request:

```sh
$ echo not-json | nc -U /run/deputyos/relay.sock
{"ok":false,"errors":[{"script":"-","reason":"invalid JSON request: expected value at line 1 column 1"}]}
```

## Per-language clients

Idiomatic pattern in each shipped language. Each reconnects per
event:

### Python

```python
import json, socket

def fire(kind, payload, sock="/run/deputyos/relay.sock"):
    s = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
    s.connect(sock)
    line = json.dumps({"kind": kind, "payload": payload}) + "\n"
    s.sendall(line.encode())
    s.shutdown(socket.SHUT_WR)
    resp = s.recv(65536).decode()
    s.close()
    return json.loads(resp)
```

### Node

```js
const net = require("net");
function fire(kind, payload, sock = "/run/deputyos/relay.sock") {
  return new Promise((resolve) => {
    const c = net.createConnection(sock);
    let buf = "";
    c.on("data", (d) => (buf += d.toString()));
    c.on("end", () => resolve(JSON.parse(buf)));
    c.write(JSON.stringify({ kind, payload }) + "\n");
    c.end();
  });
}
```

### Bash (used by the voice-relay)

```sh
fire_hook() {
  local kind=$1 payload=$2
  printf '%s\n' "$(jq -nc --arg k "$kind" --argjson p "$payload" \
    '{kind:$k, payload:$p}')" \
    | nc -U /run/deputyos/relay.sock
}
```

The voice-relay shim
([`/opt/deputyos/voice/voice-relay.sh`](../system/systemd-units.md#deputyos-voice-relayservice))
fires `pre-message` with `source=voice` in the payload — a convention,
not a new HookKind.

## Permissions and confinement

- Socket file mode: 0600.
- Owner: the user that calls `deputyctl --internal-run-relay` (the
  service user, `agent`).
- Per-profile [AppArmor profile](../system/apparmor-profiles.md) grants
  `network unix stream` and `/run/deputyos/relay.sock rw` for clients
  that need to talk to the relay (the voice-relay's profile makes this
  explicit).

## See also

- [Reference / Schemas / Hook payloads](../schemas/hook-payloads.md) —
  the four payload shapes.
- [How-to / Add a hook](../../how-to/add-a-hook.md) — drop-script
  recipe and end-to-end example.
- [Reference / CLI / deputyctl](../cli/deputyctl.md) — `--internal-run-relay`
  flag (hidden).
- [Reference / System / systemd units](../system/systemd-units.md) —
  the gateway services that consume the relay; the voice-relay that
  speaks pre-message.
- [Reference / System / AppArmor profiles](../system/apparmor-profiles.md) —
  network and FS mediations for relay clients.
- [Reference / System / Filesystem layout](../system/filesystem-layout.md#rundeputyos-runtime-tmpfs-cleared-on-reboot) —
  `/run/deputyos/relay.sock` row.
