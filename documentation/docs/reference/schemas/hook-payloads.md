# Hook payload schemas

deputyOS fires **four kinds of user-installable hook**: `pre-message`,
`post-message`, `cost-alert`, `update-applied`. Each is a directory of
executable scripts under `/etc/deputyos/hooks.d/<kind>/` that receives
one JSON object on stdin.

The Rust enum is in
[`deputyctl/src/hooks.rs`](https://github.com/deputyos/deputyos/blob/main/deputyctl/src/hooks.rs);
the payload shapes are codified as JSON Schema in
[`deputyctl/etc/hook-payload-schemas.json`](https://github.com/deputyos/deputyos/blob/main/deputyctl/etc/hook-payload-schemas.json)
and quoted verbatim below.

[TOC]

## Hook script contract

Every script under `/etc/deputyos/hooks.d/<kind>/` follows the same
contract:

| Aspect | Behaviour |
|---|---|
| **Discovery** | The dispatcher walks the kind directory in lexical order. Non-files and non-executable files are silently skipped (drop a `.disabled` suffix to disable a script without removing it). |
| **Stdin** | Compact JSON, one object, EOF after. |
| **Stdout** | Discarded. |
| **Stderr** | The trailing 1024 bytes are captured and surfaced via `tracing::warn` and (when fired through the relay) the JSON response. |
| **Timeout** | 5 seconds. Scripts that exceed it are killed and reported as `Timeout`. |
| **Exit code** | Aggregated. Non-zero is logged, but the dispatcher **never** propagates the failure to the calling code path — hooks are advisory. |
| **Permissions** | Must have an executable bit set. Mode `0755` is conventional. |

The dispatcher is `deputyctl/src/hooks.rs::fire_hook_in_collect`. The
relay (see [Message relay](../apis/message-relay.md)) wraps it for
external agent processes.

## The four `HookKind` variants

| Kind | On-disk dir | Fired by | Payload schema |
|---|---|---|---|
| `pre-message` | `/etc/deputyos/hooks.d/pre-message/` | `deputyctl --internal-run-relay` (agent → socket → dispatcher) | [pre-message](#pre-message) |
| `post-message` | `/etc/deputyos/hooks.d/post-message/` | Same as above. | [post-message](#post-message) |
| `cost-alert` | `/etc/deputyos/hooks.d/cost-alert/` | `cost::evaluate` when a configured budget threshold is crossed. | [cost-alert](#cost-alert) |
| `update-applied` | `/etc/deputyos/hooks.d/update-applied/` | `update::run_apply` after a staged image is verified against the signed manifest. | [update-applied](#update-applied) |

Mapping is implemented in `HookKind::dir_name` and `HookKind::parse`.

## `pre-message`

Fired by the agent (via the relay) **before** a user message is sent to
the model. Hooks may short-circuit by exiting non-zero, but deputyOS
does not enforce blocking semantics; hooks are advisory and the agent
decides whether to honour the exit code.

### Required fields

- `timestamp` — ISO-8601 UTC.
- `channel` — source channel id (Slack channel id, web session id, CLI `tty`).
- `user_id` — stable identifier for the message author within the channel.
- `message` — raw message text, before any redaction or templating.

### Optional fields

- `model_provider` — configured provider id.
- `model_id` — provider-specific model id.

`additionalProperties: true` — agents may attach extra context (e.g.
`source: "voice"` for the voice-relay's pre-message convention).

### Example payload

```json
{
  "timestamp": "2026-04-27T15:30:00Z",
  "channel": "tty",
  "user_id": "agent",
  "message": "summarize today's standup",
  "model_provider": "openrouter",
  "model_id": "anthropic/claude-sonnet-4.6"
}
```

## `post-message`

Fired by the agent (via the relay) **after** a model response has been
delivered to the user. Carries cost and latency telemetry.
`cost-alert` is its own kind; `post-message` does not double as a
threshold alert.

### Required fields

- `timestamp` — ISO-8601.
- `channel` / `user_id` — see pre-message.
- `duration_ms` — wall-clock ms from request to final-token delivery.

### Optional fields

- `tokens.{input,output,total}` — provider-reported token counts. Absent if the provider does not return them.
- `cost_usd` — computed USD cost. deputyOS uses the provider's published pricing.
- `model_provider` / `model_id` — as above.

### Example payload

```json
{
  "timestamp": "2026-04-27T15:30:04Z",
  "channel": "tty",
  "user_id": "agent",
  "duration_ms": 4123,
  "tokens": {"input": 1240, "output": 312, "total": 1552},
  "cost_usd": 0.00821,
  "model_provider": "openrouter",
  "model_id": "anthropic/claude-sonnet-4.6"
}
```

## `cost-alert`

Fired by `cost::evaluate` when a configured budget threshold is
crossed. Threshold model and emission cadence are documented in
[Operations / Cost guardrails](../../operations/cost-guardrails.md).

### Required fields

- `timestamp` — ISO-8601.
- `threshold_usd` — the budget that triggered the alert.
- `spent_usd` — cumulative spend in the current window when the alert fired.
- `window` — one of `hourly`, `daily`, `monthly`.

### Optional fields

- `provider` — provider id (omitted for cross-provider aggregates).
- `level` — `warning` (approaching) or `exceeded` (surpassed).

### Example payload

```json
{
  "timestamp": "2026-04-27T22:00:00Z",
  "threshold_usd": 5.00,
  "spent_usd": 4.87,
  "window": "daily",
  "provider": "openrouter",
  "level": "warning"
}
```

## `update-applied`

Fired by `update::run_apply` **after** the staged image has been
verified against the signed manifest. The A/B swap itself is M6
territory; this hook fires on the staging step so admin tooling can
refresh dashboards and send notifications eagerly without waiting for
the actual reboot.

### Required fields

- `kind` — const `"update-applied"`.
- `staged_at` — absolute path of the staged artefact on this host.
- `filename` — basename of the artefact.
- `sha256` — lowercase hex, validated against `^[a-f0-9]{64}$`.
- `release_version` — Y.M.D release identifier from the signed manifest.

### Example payload

```json
{
  "kind": "update-applied",
  "staged_at": "/var/lib/deputyos/staging/2026.4.27/deputyos-openclaw-rpi5-2026.4.27-stable.img.xz",
  "filename": "deputyos-openclaw-rpi5-2026.4.27-stable.img.xz",
  "sha256": "4b3c2a1f0e9d8c7b6a5f4e3d2c1b0a9f8e7d6c5b4a3f2e1d0c9b8a7f6e5d4c3b",
  "release_version": "2026.4.27"
}
```

## Source schema (`deputyctl/etc/hook-payload-schemas.json`)

Quoted verbatim — the single source of truth for the four shapes:

```json
{
  "$schema": "http://json-schema.org/draft-07/schema#",
  "$id": "https://www.deputyos.com/schemas/hook-payloads/v1.json",
  "title": "deputyOS hook payload schemas",
  "definitions": {
    "pre-message": {
      "type": "object",
      "required": ["timestamp", "channel", "user_id", "message"],
      "additionalProperties": true,
      "properties": {
        "timestamp":      {"type": "string", "format": "date-time"},
        "channel":        {"type": "string"},
        "user_id":        {"type": "string"},
        "message":        {"type": "string"},
        "model_provider": {"type": "string"},
        "model_id":       {"type": "string"}
      }
    },
    "post-message": {
      "type": "object",
      "required": ["timestamp", "channel", "user_id", "duration_ms"],
      "additionalProperties": true,
      "properties": {
        "timestamp":   {"type": "string", "format": "date-time"},
        "channel":     {"type": "string"},
        "user_id":     {"type": "string"},
        "duration_ms": {"type": "integer", "minimum": 0},
        "tokens": {
          "type": "object",
          "properties": {
            "input":  {"type": "integer", "minimum": 0},
            "output": {"type": "integer", "minimum": 0},
            "total":  {"type": "integer", "minimum": 0}
          }
        },
        "cost_usd":       {"type": "number", "minimum": 0},
        "model_provider": {"type": "string"},
        "model_id":       {"type": "string"}
      }
    },
    "cost-alert": {
      "type": "object",
      "required": ["timestamp", "threshold_usd", "spent_usd", "window"],
      "additionalProperties": true,
      "properties": {
        "timestamp":     {"type": "string", "format": "date-time"},
        "threshold_usd": {"type": "number", "minimum": 0},
        "spent_usd":     {"type": "number", "minimum": 0},
        "window":        {"enum": ["hourly", "daily", "monthly"]},
        "provider":      {"type": "string"},
        "level":         {"enum": ["warning", "exceeded"]}
      }
    },
    "update-applied": {
      "type": "object",
      "required": ["kind", "staged_at", "filename", "sha256", "release_version"],
      "additionalProperties": true,
      "properties": {
        "kind":            {"const": "update-applied"},
        "staged_at":       {"type": "string"},
        "filename":        {"type": "string"},
        "sha256":          {"type": "string", "pattern": "^[a-f0-9]{64}$"},
        "release_version": {"type": "string"}
      }
    }
  },
  "oneOf": [
    {"$ref": "#/definitions/pre-message"},
    {"$ref": "#/definitions/post-message"},
    {"$ref": "#/definitions/cost-alert"},
    {"$ref": "#/definitions/update-applied"}
  ]
}
```

## See also

- [Reference / APIs / Message relay](../apis/message-relay.md) — the
  Unix-socket protocol that turns these payloads into hook invocations.
- [How-to / Add a hook](../../how-to/add-a-hook.md) — step-by-step
  recipe with a working example script.
- [Operations / Cost guardrails](../../operations/cost-guardrails.md) —
  how `cost-alert` integrates with the cost ledger.
- [Operations / Update and rollback](../../operations/update-and-rollback.md) —
  where `update-applied` fits in the update flow.
- [Reference / System / Filesystem layout](../system/filesystem-layout.md) —
  `/etc/deputyos/hooks.d/` directory layout.
