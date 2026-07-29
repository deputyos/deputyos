# Restrict outbound traffic with the egress whitelist

deputyOS gives a device three outbound-egress postures, all enforced by a
single nftables `output` chain — so they apply to **every** process on the
device (root, the `agent` user, hooks, skills), not just the agent binary:

| Mode | Egress | `allow_hosts` | When |
|---|---|---|---|
| `open` | unrestricted (ufw still protects inbound) | ignored | the default for connected tiers |
| `whitelist` | only the hosts in `allow_hosts` | consulted | open internet, but only to a curated list |
| `airgap` | deny everything except RFC1918 + mDNS + local DNS | ignored | true air-gap (see [Air-gapped builds](../../concepts/airgap.md)) |

`whitelist` sits between `open` and `airgap`: the device reaches the public
internet, but only to hosts you allow. Read the
[egress threat model](../../concepts/threat-model-egress.md) first — it is a
**guardrail, not a hard boundary** (DNS-only allow, not SNI inspection).

The authoritative policy is `/etc/deputyos/network-policy.json`
([schema](../../reference/schemas/network-policy.md)). Mutate it with
`deputyctl network`; render + reload nftables with `deputyctl network apply`.

## Switch to whitelist

```sh
sudo deputyctl network mode whitelist
sudo deputyctl network apply
sudo deputyctl network status --json
```

When you switch to `whitelist` with an empty `allow_hosts`, the list is
**seeded** from `/etc/deputyos/network-defaults.json` — the per-profile curated
list baked at image time (LLM providers + chat gateways + package indices the
profile needs). The seed is idempotent: a list you've already curated is never
clobbered.

## Add or remove a host

```sh
sudo deputyctl network allow add api.openai.com
sudo deputyctl network allow remove api.openai.com
sudo deputyctl network apply      # re-render + reload nftables
```

`allow` mutates the policy file; `apply` is what makes it live (it resolves
each hostname to its current IPv4 and pins those IPs — see the limitation
below). Removing a host drops it from the allow-list at the next `apply`.

## The wizard + PWA surfaces

The first-boot wizard's **Egress** step offers the three modes and pre-selects
the profile's recommended default (`[default_egress]` in the profile manifest).
Pick `whitelist` there and the wizard runs `deputyctl network mode whitelist`
+ `deputyctl network apply` for you at the end.

The PWA **Network** card (`/app/network`) shows the current mode + the
allow-listed hosts. It's read-only on the card; mutate via `deputyctl network`.

## Boot self-heal

`deputyos-network-apply.service` re-renders `/etc/nftables.conf` from the policy
on every boot, so a rule a compromised process smuggled in at runtime does not
survive a reboot. In `whitelist` mode the boot re-apply **also re-resolves each
allow-host** — this is how the allow-list follows DNS rotation. In `open` mode
the oneshot is a no-op (ufw owns inbound; `deputyctl network apply`'s
`flush ruleset` would otherwise wipe ufw's inbound default-deny).

## Verify

```sh
sudo deputyctl doctor              # the network-policy row: green = policy ⇔ ruleset agree
sudo nft list ruleset              # confirm the deputyos table + policy drop
```

`deputyctl doctor`'s `network-policy` check is **green** for `open`, or when
the live ruleset contains the `deputyos` table with `policy drop` for
`whitelist`/`airgap`. It **warns** on drift (policy says drop but the kernel
isn't enforcing it) — fix with `deputyctl network apply`.

## Limitation: DNS-only, not SNI

`deputyctl network apply` resolves each `allow_hosts` hostname to its **current
IPv4 addresses** and pins those IPs in nftables. It does **not** inspect TLS
SNI. Consequences (see the [threat model](../../concepts/threat-model-egress.md)):

- A host whose DNS rotates (CDNs, round-robin, failover) silently drops off the
  pinned set until the next `apply` — schedule a periodic re-apply (the boot
  oneshot covers reboot; add a timer for long uptimes) or accept the false
  negatives (availability loss, not a bypass).
- IPv6 egress is denied by default (only IPv4 `ip daddr` rules are emitted) —
  a v6-only host is unreachable in `whitelist` even if listed.

SNI-based filtering (which follows the hostname regardless of IP) requires a
TLS-intercepting proxy and is **out of M5.5 scope**.

## See also

- [Egress threat model](../../concepts/threat-model-egress.md) — failure modes.
- [Air-gapped builds](../../concepts/airgap.md) — the `airgap` counterpart.
- [Network policy schema](../../reference/schemas/network-policy.md).
- [Network defaults schema](../../reference/schemas/network-defaults.md) — the seed.