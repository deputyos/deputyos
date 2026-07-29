# Add a hook

## What this guide does

Add a custom **hook script** to your deputyOS device. Hooks are
executable files dropped under `/etc/deputyos/hooks.d/<kind>/` that the
runtime fires at four well-defined moments:

- `pre-message` — before a user message is sent to the model.
- `post-message` — after a model response has been delivered.
- `cost-alert` — when a configured cost threshold is crossed.
- `update-applied` — after a staged update has been verified.

Hooks are how you wire deputyOS into your existing tooling — Slack
notifications, Pagerduty alerts, on-disk audit logs, custom redaction,
local cost dashboards — without forking the appliance image.

## Prerequisites

- An deputyOS device, booted, with the wizard finished (so
  `/etc/deputyos/hooks.d/` exists with the four kind subdirectories).
- SSH access (or the PWA logs viewer to debug — but you'll need
  filesystem access to drop the script).
- A reading of the hook payload contract:
  [Reference → Schemas → hook payloads](../reference/schemas/hook-payloads.md).
  The canonical schemas live in
  `deputyctl/etc/hook-payload-schemas.json`.

## The recipe

### 1. Pick a kind

The four kinds and their canonical use cases:

| Kind | Fired by | Typical use |
| --- | --- | --- |
| `pre-message` | the agent (via the relay) | redaction, request logging, prompt injection |
| `post-message` | the agent (via the relay) | per-message audit log, latency dashboard, cost ledger sidecar |
| `cost-alert` | `deputyctl cost evaluate` | Slack / Discord / PagerDuty notification, automated pause |
| `update-applied` | `deputyctl update --apply` | refresh dashboards, send "appliance updated" email |

### 2. Drop the script

Path shape: `/etc/deputyos/hooks.d/<kind>/NN-<name>.sh` (or `.py`, `.js`,
or any executable). Numbering ensures alphabetic ordering — the
dispatcher runs every executable file in the directory in order.

The file must be **executable** (`chmod 0755`). Owner can be
`root:root` or `agent:agent`; the dispatcher only requires the file
mode bit.

### 3. Read the payload from stdin

Every hook receives **one compact JSON object** on stdin, terminated by
EOF. The schema depends on the kind. Use whatever JSON parser you
like — `jq` for shell, `json.loads(sys.stdin.read())` for Python,
`require('fs').readFileSync(0, 'utf-8')` for Node.

Example payloads (see
[Reference → Schemas → hook payloads](../reference/schemas/hook-payloads.md)
for full schemas):

```json
{"timestamp":"2026-04-26T12:34:56Z","channel":"slack-C123","user_id":"U456",
 "message":"hello","model_provider":"openrouter","model_id":"anthropic/claude-sonnet-4-6"}
```

```json
{"timestamp":"2026-04-26T12:34:56Z","threshold_usd":5.00,"spent_usd":4.20,
 "window":"daily","provider":"openrouter","level":"warning"}
```

```json
{"kind":"update-applied","staged_at":"/var/lib/deputyos/staging/deputyos-rpi5-openclaw-2026.4.27.img.xz",
 "filename":"deputyos-rpi5-openclaw-2026.4.27.img.xz",
 "sha256":"deadbeef...","release_version":"2026.4.27"}
```

### 4. Exit code conventions

- `0` — success. The dispatcher records nothing.
- non-zero — the dispatcher logs `script <name> exited <code>: <stderr
  tail>` and continues to the next hook.

**Hooks do not block the calling code path.** A `pre-message` hook that
exits non-zero does **not** prevent the agent from sending the
message. `deputyOS` does not enforce blocking semantics — hooks are
advisory. (The agent itself may choose to honour an advisory veto, but
that's an agent-level decision, not an deputyOS guarantee.)

### 5. Stay under the timeout

Each hook gets **5 seconds** wall-clock. The dispatcher
(`HOOK_TIMEOUT` constant in `deputyctl/src/hooks.rs`) sends SIGKILL on
overrun and records the timeout. If your hook needs to do anything
slow (HTTP POST to a webhook, write to a remote DB), either run it
async (`disown`-style background) or accept that 5 seconds is the
ceiling.

### 6. Stderr is captured

The last **1024 bytes** of stderr (`STDERR_TAIL_LIMIT` in
`hooks.rs`) are preserved on failure. Keep your stderr concise; a
chatty hook makes the failure log unreadable. Logs are visible via
`journalctl -u deputyos-relay` (when wired) or in the per-script error
record returned by the relay's JSON response.

## Verification

```sh
# 1. The dispatcher sees your script.
ls -l /etc/deputyos/hooks.d/<kind>/

# 2. Drive it manually with a synthetic payload.
echo '{"timestamp":"2026-04-26T00:00:00Z","channel":"test","user_id":"u1","message":"hi"}' \
  | /etc/deputyos/hooks.d/pre-message/01-yourhook.sh

# 3. Round-trip through the relay (when the agent is wired).
echo '{"kind":"pre-message","payload":{"timestamp":"2026-04-26T00:00:00Z",
    "channel":"test","user_id":"u1","message":"hi"}}' \
  | nc -U /run/deputyos/relay.sock
```

The relay's response is one JSON line:

```json
{"ok": true, "errors": []}
```

Or, on a script failure:

```json
{"ok": false, "errors": [
  {"script": "/etc/deputyos/hooks.d/pre-message/01-yourhook.sh",
   "code": 2, "stderr_tail": "missing config\n"}
]}
```

## Worked example: cost-alert Slack webhook

A `cost-alert` hook that posts to a Slack incoming webhook when costs
trip a threshold. This is the runbook the Operations page points at.

### File: `/etc/deputyos/hooks.d/cost-alert/01-slack-webhook.sh`

```bash
#!/usr/bin/env bash
# Cost-alert hook: post to a Slack incoming webhook.
#
# Place at: /etc/deputyos/hooks.d/cost-alert/01-slack-webhook.sh
# Make executable: chmod 0755 <path>
#
# Webhook URL is read from /etc/deputyos/hooks.d/cost-alert/.slack-webhook
# (mode 0600, owner agent:agent). Falls back to silent no-op if absent.

set -euo pipefail

webhook_file="/etc/deputyos/hooks.d/cost-alert/.slack-webhook"
[[ -r "$webhook_file" ]] || exit 0

webhook_url="$(head -1 "$webhook_file")"
payload="$(cat)"

threshold=$(echo "$payload" | jq -r '.threshold_usd')
spent=$(echo "$payload"     | jq -r '.spent_usd')
window=$(echo "$payload"    | jq -r '.window')
level=$(echo "$payload"     | jq -r '.level')
provider=$(echo "$payload"  | jq -r '.provider // "unknown"')

emoji=":warning:"
[[ "$level" == "exceeded" ]] && emoji=":rotating_light:"

text="${emoji} deputyOS cost ${level}: \$${spent} of \$${threshold} ${window} cap (provider=${provider})"

curl --silent --show-error --fail \
  --max-time 4 \
  --header 'Content-Type: application/json' \
  --data "{\"text\": \"${text}\"}" \
  "$webhook_url" >/dev/null
```

Set up:

```sh
sudo install -m 0755 -o root -g root \
  01-slack-webhook.sh /etc/deputyos/hooks.d/cost-alert/01-slack-webhook.sh

sudo install -m 0600 -o agent -g agent /dev/null \
  /etc/deputyos/hooks.d/cost-alert/.slack-webhook
echo "https://hooks.slack.com/services/T.../B.../..." \
  | sudo tee /etc/deputyos/hooks.d/cost-alert/.slack-webhook >/dev/null
```

Trigger a test:

```sh
sudo deputyctl cost set --warn-at-pct 1     # fire on virtually any spend
sudo deputyctl cost --check                  # forces an evaluation
```

Or drive directly:

```sh
echo '{"timestamp":"2026-04-26T00:00:00Z","threshold_usd":5,"spent_usd":4.5,
       "window":"daily","provider":"openrouter","level":"warning"}' \
  | /etc/deputyos/hooks.d/cost-alert/01-slack-webhook.sh
```

## Speaking the relay protocol directly

If you're embedding hooks into a custom agent (rather than dropping
scripts), you can speak the message-relay Unix-socket protocol
directly. See
[Reference → APIs → message relay](../reference/apis/message-relay.md)
for the wire format.

Quick summary:

- Connect to `/run/deputyos/relay.sock` (default;
  `DEPUTYOS_RELAY_SOCKET=` overrides).
- Send one newline-terminated JSON line:
  `{"kind": "<kind>", "payload": <object>}`.
- Read one newline-terminated JSON response: `{"ok": <bool>, "errors":
  [...]}`.
- Connection closes after the response. Reconnect per event (cheap on
  the same host).

## Troubleshooting

!!! warning "Hook never runs"
    Either the file is not executable (`chmod 0755 <path>`), it lives
    in the wrong kind directory, or the dispatcher hasn't been wired
    to fire that kind for this profile yet (the `pre-message` and
    `post-message` paths are agent-fired, so they only fire when the
    upstream agent is in the loop). Check `journalctl -u
    deputyos-relay` for "no hooks installed" or path-not-executable
    diagnostics.

!!! warning "Hook is killed mid-execution"
    The 5-second timeout is firm. If your hook does I/O that can take
    longer (slow webhook, large payload), background it:
    `(curl ... &)` and exit immediately. The dispatcher waits only on
    your script's PID.

!!! danger "Stderr leaks secrets"
    The dispatcher captures the **last 1024 bytes** of stderr on
    failure. Make sure your script does not echo API keys, OAuth
    tokens, or webhook URLs to stderr — they end up in the relay log
    and (if you're on a shared appliance) in `journalctl`.

!!! tip "Order hooks by NN- prefix"
    Use `01-`, `02-`, … prefixes for deterministic ordering. Hooks run
    serially, in alphabetic order; you can rely on `01-redact.sh`
    completing before `02-audit.sh` reads the (now-redacted) message.

## Related

- [Reference → Schemas → hook payloads](../reference/schemas/hook-payloads.md)
- [Reference → APIs → message relay](../reference/apis/message-relay.md)
- [Operations → Cost guardrails](../operations/cost-guardrails.md)
- [Reference → CLI → deputyctl](../reference/cli/deputyctl.md)
