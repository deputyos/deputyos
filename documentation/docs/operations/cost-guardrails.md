# Cost guardrails

## What this guide does

Configure deputyOS's **cost guardrails**: per-day and per-month spend
caps, an auto-pause behaviour when caps trip, percentage-warning
thresholds, and the `CostAlert` hook that fires on either. deputyOS
ships sensible defaults (a $5 daily cap, $100 monthly cap, 80%
warning, pause-on-trip), but every device should review them during
the first day of use.

## The cost ledger contract

The cost ledger is a JSONL file at `~/.<profile>/cost-ledger.jsonl` —
one entry per LLM request, written by the **upstream agent**.
`deputyctl cost` is the **reader**: `cost ledger`, `cost --check`, and
the cost-summary block in `deputyctl status` all read this file. Only
`cost reset` writes (and only to the tripped-marker; never to the
ledger).

A typical entry:

```json
{"timestamp":"2026-04-26T12:34:56Z","provider":"openrouter",
 "model":"anthropic/claude-sonnet-4-6",
 "input_tokens":123,"output_tokens":456,"cost_usd":0.0078,
 "duration_ms":1240,"channel":"web","user_id":"u1"}
```

If `cost_usd` is missing, deputyctl falls back to the per-1M-token
rates in `/etc/deputyos/cost-defaults.json` (see the
[providers schema][providers]). The per-message ledger writeback is
the source of truth — defaults are fallback only.

[providers]: ../reference/schemas/providers-json.md

## Configuration: `/etc/deputyos/cost.toml`

```toml
[caps]
daily_usd   = 5.00       # default
monthly_usd = 100.00     # default

[behaviour]
on_cap_trip = "pause"    # pause | warn | nothing
warn_at_pct = 80         # 0..=100; fire CostAlert at this percentage

[quiet_hours]
enabled   = false        # default
start     = "22:00"      # local timezone
end       = "08:00"
behaviour = "pause"      # pause | refuse | nothing
```

Edit via the CLI (atomic write):

```sh
sudo deputyctl cost set --daily-cap 10 --monthly-cap 200 \
  --on-cap-trip pause --warn-at-pct 75
```

Or directly in the file (then `systemctl restart` the gateway).

## Behaviours

### `on_cap_trip = "pause"`

When the daily or monthly cap is exceeded, the cost-tripped marker
file `/var/lib/deputyos/cost-tripped` is written. The gateway unit's
`ExecStartPre` is a `deputyctl cost --check` invocation that exits
non-zero if the marker exists, blocking restart. The unit is therefore
**unable to start** until you run:

```sh
sudo deputyctl cost reset
```

`cost reset` clears the marker. **It does not touch the ledger** —
your accumulated spend stays counted; reset only un-pauses the unit
so future requests can continue against the existing ledger.

### `on_cap_trip = "warn"`

Fires the CostAlert hook (level=`exceeded`) but does not pause. Useful
when you want notification but not interruption.

### `on_cap_trip = "nothing"`

Fires nothing, pauses nothing. Combined with `warn_at_pct` you can
still get the warning fire — the trip itself is just silent.

## Quiet hours

A schedule that pauses or refuses messages during a window. Cross-
midnight windows (`22:00` to `08:00`) work — the comparison is
modular. Configure:

```sh
sudo deputyctl quiet-hours set --enable --start 22:00 --end 08:00 --behaviour pause
sudo deputyctl quiet-hours set --disable
```

Behaviours:

- `pause` — same as cost trip; gateway refuses to start, mark cleared
  by next `cost reset`. The active session may already be in flight;
  in-flight requests complete.
- `refuse` — gateway stays up but returns a structured "in quiet
  hours" response on every request. Channel-specific phrasing handled
  by the upstream agent.
- `nothing` — log only. Useful with hooks for ambient notification.

The quiet-hours window logic is in `deputyctl/src/quiet_hours.rs`. The
in-day vs cross-midnight branching has unit tests; behaviours are
documented per-shape.

## The CostAlert hook firing

Two trigger points, both implemented in `deputyctl::cost::evaluate`:

1. **Warning** — when `spent_today / daily_cap >= warn_at_pct/100`.
   Payload:

    ```json
    {"timestamp":"2026-04-26T12:34:56Z","threshold_usd":5.00,"spent_usd":4.20,
     "window":"daily","provider":"openrouter","level":"warning"}
    ```

2. **Exceeded** — when the cap is crossed. `level: "exceeded"`.

Both fire scripts in `/etc/deputyos/hooks.d/cost-alert/`. See
[How-to → Add a hook](../how-to/add-a-hook.md) for the canonical
Slack-webhook example.

The schemas are authoritative in
`deputyctl/etc/hook-payload-schemas.json` and walked in
[Reference → Schemas → hook payloads](../reference/schemas/hook-payloads.md).

## Production runbook: cap tripped

Symptoms: gateway unit is `failed`. `systemctl status` shows
`deputyctl cost --check` returning non-zero.

Recovery, in order:

1. **Inspect the ledger** — what tripped it?

    ```sh
    sudo deputyctl cost --json
    sudo deputyctl cost ledger --last 50
    ```

2. **Decide.** Was this a real spike (legitimate use, raise the cap),
   a misconfiguration (raise temporarily, fix the cause), or an
   incident (a runaway loop in your custom hook, a key rotation
   storm)?

3. **If raising the cap permanently** —

    ```sh
    sudo deputyctl cost set --daily-cap <new>
    ```

4. **Clear the marker.**

    ```sh
    sudo deputyctl cost reset
    ```

5. **Restart.**

    ```sh
    sudo deputyctl up
    sudo deputyctl status
    ```

6. **Confirm the underlying cause.** If the cause was a runaway loop,
   leave the cap at the old value, fix the loop, and only then raise.

## Verification

```sh
# Configuration is read correctly
sudo deputyctl cost --json

# Ledger reads
sudo deputyctl cost ledger --last 20 --json

# Force a trip in dev (set daily cap to $0.01)
sudo deputyctl cost set --daily-cap 0.01
# Send a message via the agent; the next post-message ledger write trips.

# Verify the marker exists
sudo ls /var/lib/deputyos/cost-tripped

# Reset
sudo deputyctl cost reset
sudo deputyctl cost set --daily-cap 5.00
```

## Troubleshooting

!!! warning "Cap tripped but the gateway is still running"
    `on_cap_trip` may be set to `warn` or `nothing`. Check
    `deputyctl cost --json | jq .config.behaviour.on_cap_trip`. Also
    verify the gateway unit's `ExecStartPre` line is actually
    `deputyctl cost --check` — older units predate the trip-on-start
    plumbing.

!!! warning "Cost ledger shows entries but `deputyctl cost` says $0"
    Almost always a date-math edge case. `deputyctl cost` aggregates by
    UTC midnight prefix; if your local time crossed midnight while the
    ledger has yesterday's entries, "today's spend" resets. Look at
    `cost.rs::today_total` for the prefix logic.

!!! danger "Setting daily-cap to a very low value pauses the gateway immediately"
    `deputyctl cost set --daily-cap 0.01` followed by even one message
    will trip. Test changes in dev before applying on production.

!!! tip "Tighten the warn threshold"
    Default 80% gives you 20% headroom, which on a $5 daily cap is $1
    — a single Anthropic Sonnet response can use that. For interactive
    workloads, drop to 50% so you get notified earlier.

## Related

- [Reference → CLI → deputyctl](../reference/cli/deputyctl.md) (`cost` and `quiet-hours` subcommand trees)
- [Reference → Schemas → hook payloads](../reference/schemas/hook-payloads.md)
- [Reference → Schemas → providers.json](../reference/schemas/providers-json.md) (cost-defaults)
- [How-to → Add a hook](../how-to/add-a-hook.md)
- [Operations → Monitoring and logs](monitoring-and-logs.md)
