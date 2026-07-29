# Network defaults (`network-defaults.json`)

`/etc/deputyos/network-defaults.json` is a **per-profile curated seed** — a
list of hostnames the profile genuinely needs outbound (LLM providers, chat
gateways, package indices). It is baked at image time from
`roles/deputyos/files/network-defaults.<profile>.json` (openclaw, hermes, khoj
ship one each) and copied to the device by
`roles/deputyos/tasks/network-defaults.yml` (`failed_when: false`, so profiles
without a defaults file are fine).

It is **read-only**: nothing mutates it at runtime. Its only consumer is the
seeding step in `deputyctl network mode whitelist` — when the live
[`network-policy.json`](network-policy.md)'s `allow_hosts` is empty, the
defaults' `allow_hosts` are copied in (sorted + deduped). A user-curated,
non-empty list is **never clobbered**.

The on-disk JSON Schema is at
[`docs/schemas/network-defaults-v1.json`](https://github.com/deputyos/deputyos/blob/main/docs/schemas/network-defaults-v1.json)
(also the `$schema` URL written into every defaults file).

## Fields

| Field | Type | Required | Default | Description |
|---|---|---|---|---|
| `$schema` | string | no | the v1 URL | JSON Schema URL, for editor validation. |
| `profile` | string | yes | — | The profile id this defaults file was curated for (e.g. `openclaw`). Informational. |
| `mode` | enum | no | — | Informational: the mode the profile recommends. Seeding only happens on a switch to `whitelist`; this field is not enforced. |
| `allow_hosts` | string[] | yes | `[]` | Hostnames permitted in `whitelist` mode. Resolved to IPv4 at apply time — see the [egress threat model](../../concepts/threat-model-egress.md) for the DNS-only (not SNI) limitation. |

## How seeding works

```sh
deputyctl network mode whitelist   # seeds allow_hosts from this file if empty
deputyctl network apply            # render + reload nftables
```

- Seeds **only** when the live policy's `allow_hosts` is empty.
- Never clobbers a list the operator has built with `deputyctl network allow add`.
- After seeding, the live `network-policy.json` is the authoritative list;
  edit it, not this file.

See the [egress how-to](../../how-to/operate/egress-whitelist.md) for the full
flow and the [egress threat model](../../concepts/threat-model-egress.md) for
the failure modes.