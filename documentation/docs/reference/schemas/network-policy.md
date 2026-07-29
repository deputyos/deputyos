# Network policy (`network-policy.json`)

`/etc/deputyos/network-policy.json` is the **single source of truth** for the
device's network-egress posture. It is read and mutated by
[`deputyctl network`](../cli/deputyctl.md), rendered into an nftables ruleset
(`/etc/nftables.conf`) by the Ansible role, and surfaced on the PWA dashboard's
"Your device" + "Network" cards.

The on-disk JSON Schema is at
[`docs/schemas/network-policy-v1.json`](https://github.com/deputyos/deputyos/blob/main/docs/schemas/network-policy-v1.json)
(also served at `https://www.deputyos.com/schemas/network-policy-v1.json`,
the `$schema` URL written into every policy file). The Rust struct is in
[`deputyctl/src/network.rs`](https://github.com/deputyos/deputyos/blob/main/deputyctl/src/network.rs).

!!! info "Air-gapped builds bake this file"
    On an `AIRGAP=1` build the airgap baseline drops a policy file with
    `mode=airgap`, `set_at_build_time=true`, and the bake-time `tier`/`hw`/
    `profile` populated — so a freshly-baked air-gapped device boots into
    deny-by-default egress without the operator touching anything. See
    [Air-gapped builds](../../concepts/airgap.md).

[TOC]

## Resolution order

`deputyctl/src/network.rs::read`:

1. An explicit `path` argument (used by tests).
2. `/etc/deputyos/network-policy.json`.

If the file does not exist (e.g. on a non-airgap dev host), `read`
synthesises a default `open` policy — so the wizard and PWA always have a
mode to render.

## Top-level shape

| Field | Type | Required | Default | Description |
|---|---|---|---|---|
| `$schema` | string | no | `https://www.deputyos.com/schemas/network-policy-v1.json` | JSON Schema URL. Written so editors + future consumers can validate. The Rust struct defaults it when absent. |
| `mode` | enum | yes | — | Egress posture: `open`, `whitelist`, or `airgap`. See below. |
| `allow_hosts` | array of strings | no | `[]` | Hosts permitted in `whitelist` mode. Ignored in `open` and `airgap`. |
| `set_at_build_time` | boolean | no | `false` | True if written by the image bake; flipped to `false` on the first operator mutation, so the dashboard can tell baked from hand-edited. |
| `tier` | string \| null | no | `null` | Bake-time tier (lean / standard / rich). Informational. |
| `hw` | string \| null | no | `null` | Bake-time hardware target (e.g. `rpi4`, `qemu-aarch64`). Informational. |
| `profile` | string \| null | no | `null` | Active profile id the policy was baked for. Informational. |

## Modes

| Mode | Egress | `allow_hosts` | When |
|---|---|---|---|
| `open` | unrestricted | ignored | Today's default for non-airgap builds. |
| `whitelist` | only the hosts in `allow_hosts` | consulted | Allow only a curated host list, enforced by an nftables `output` chain `policy drop` plus a resolved-IP allow rule per host. See the [egress threat model](../../concepts/threat-model-egress.md) for the DNS-only (not SNI) limitation and the [egress how-to](../../how-to/operate/egress-whitelist.md). |
| `airgap` | deny everything except RFC1918 + mDNS + the local DNS resolver | ignored | An air-gapped build, or an operator who has run `deputyctl network lock --airgap`. |

`mode` is the only required field — every other field has a sensible default.

## CLI

```sh
deputyctl network status --json         # print the current policy (JSON)
deputyctl network mode whitelist        # switch mode (open | whitelist | airgap)
deputyctl network allow add api.openai.com   # add a host to the allow-list
deputyctl network apply                 # render + reload nftables from the policy
deputyctl network unlock                # → open
deputyctl network lock                  # → airgap
```

Mutations atomic-rename onto `/etc/deputyos/network-policy.json`; `deputyctl network
apply` renders the ruleset (`generate_nftables_ruleset`) to `/etc/nftables.conf`
and shells out to `nft -f`. The `deputyos-network-apply.service` oneshot re-applies
from the policy on every boot. `set_at_build_time` is cleared on the first
mutation.

## Example — air-gapped build (baked)

```json
{
  "$schema": "https://www.deputyos.com/schemas/network-policy-v1.json",
  "mode": "airgap",
  "allow_hosts": [],
  "set_at_build_time": true,
  "tier": "standard",
  "hw": "rpi4",
  "profile": "openclaw"
}
```

## Example — operator allow-list (whitelist)

```json
{
  "$schema": "https://www.deputyos.com/schemas/network-policy-v1.json",
  "mode": "whitelist",
  "allow_hosts": ["api.anthropic.com", "api.openai.com"],
  "set_at_build_time": false
}
```

## See also

- [Concepts / Air-gapped builds](../../concepts/airgap.md) — the airgap
  baseline that bakes this file.
- [Reference / CLI / deputyctl](../cli/deputyctl.md) — the `network` subcommand.
- [Reference / Schemas / limits.json](limits-json.md) — capabilities the
  policy posture intersects with (e.g. `channels_disabled_by_ram`).
- [Reference / System / systemd units](../system/systemd-units.md) —
  `nftables.service`, which enforces the rendered ruleset.