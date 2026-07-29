# Threat Model — Egress Whitelist

This is the per-subsystem threat model for the M5.5 "Egress whitelist guardrail"
work. It complements the [overview threat model](threat-model-overview.md) and
the [airgap threat-model delta](airgap.md) by zooming into the trust boundary
that appears in the middle mode — `whitelist` — which sits between `open`
(unrestricted egress) and `airgap` (deny-all except RFC1918 + mDNS + local DNS).
Read the overview and `airgap.md` first; this page assumes them.

!!! note "Retrospective, not prescriptive"
    Roadmap lane E required this document to be written *before any egress code
    landed*. In practice the nftables generator, `Mode::Whitelist`, the wizard
    `Step::Egress`, the PWA `/app/network` card, and the per-profile
    `network-defaults.<profile>.json` seed files were built first and are
    landing in the same batch as this page. This threat model is therefore
    **retrospective**: it describes the system as built, names the residual
    risks the build left open, and records the process deviation honestly.
    The headline residual risk — **DNS-only allow, not SNI inspection** — is a
    deliberate scope decision documented below, not an oversight.

## Scope

The `whitelist` mode is a *guardrail*, not a *hard boundary*. Its job is to make
the common exfiltration paths — a compromised process or a hijacked hook/skill
phoning home to an attacker host — fail by default, while still permitting the
curated outbound hosts a profile genuinely needs (LLM providers, chat gateways,
package indices). It is enforced by **nftables** on the `output` chain, so it
applies to *every* process on the device — root, the `agent` user, hooks, skills,
cron — not just to the agent binary. There is no per-app proxy.

Everything in the [overview threat model](threat-model-overview.md) still
applies — AppArmor, ufw inbound, the `agent` user, `secrets.env` 0600, signed
manifests. This page covers what is *new* or *different* under `whitelist`.

## Assets and trust boundary

| Asset | Held by | Trust property |
|---|---|---|
| `/etc/deputyos/network-policy.json` | device (root) | the authoritative policy: `{ mode, allow_hosts[], set_at_build_time }`. Mode `whitelist` + the hostname list is the rule set. Tamper via `deputyctl network` only (root). |
| `/etc/deputyos/network-defaults.json` | device (root) | read-only seed copied at bake from `roles/deputyos/files/network-defaults.<profile>.json`. Curated per-profile host list. Not trusted to *be* the policy — only to *seed* it when `allow_hosts` is empty. |
| `/etc/nftables.conf` | device (root) | regenerated from the policy by `deputyctl network apply` / the boot oneshot. Begins with `flush ruleset`; runtime-injected rules do not survive reboot. |
| The DNS resolver (`127.0.0.53` / system) | device | trusted to resolve allow-listed hostnames to the *correct* IP at apply time. See residual risk below. |

The trust boundary is the `output` chain hook: any process that wants to reach
the public internet must pass an `ip daddr <allow-listed IP>` rule, else
`counter drop`. Loopback, established/related, RFC1918, link-local, and mDNS
are allowed unconditionally (same baseline as airgap).

## Threats and mitigations

| Threat | Mitigation | Where |
|---|---|---|
| Compromised process exfiltrates via an unapproved host | `output` chain `policy drop`; only resolved IPs of `allow_hosts` pass. Applies to every process incl. root. | `deputyctl/src/network.rs` `generate_nftables_ruleset`; `/etc/nftables.conf` |
| Hijacked hook/skill phones home | Hooks and skills run as the `agent` user under the same `output` chain — no per-app bypass exists. AppArmor confines the *filesystem* surface; nftables confines the *network* surface. | nftables output chain; `roles/deputyos` AppArmor profiles |
| Policy/ruleset drift (policy says drop, kernel allows) | `deputyctl doctor network-policy` check compares the on-disk policy mode to `nft list ruleset`; warns on drift. Boot oneshot re-applies from policy every boot. | `deputyctl/src/doctor.rs`; `roles/deputyos/tasks/network-baseline.yml` |
| Profile-defaults file ships an over-broad host | The seed is *not* the policy: `set_mode(whitelist)` only seeds when `allow_hosts` is empty (idempotent); a curated list is never clobbered. Operators audit `/etc/deputyos/network-defaults.json` at bake. | `deputyctl/src/network.rs` `set_mode`; `roles/deputyos/files/network-defaults.*.json` |
| Runtime rule injection by a compromised process | `flush ruleset` at the top of every generated `/etc/nftables.conf` (and every boot) wipes injected rules. The on-disk file is the only durable input. | `deputyctl/src/network.rs` `generate_nftables_ruleset` |

## Residual risks (honest) — the Lane E deliverable

These are the failure modes lane E required documented. They are accepted
trade-offs for a guardrail-class control, not bugs to be fixed in M5.5.

### DNS-only allow, not SNI inspection — *the headline caveat*

`deputyctl network apply` resolves each `allow_hosts` hostname to its current
**IPv4 addresses** (`network.rs` `resolve_host`, via `ToSocketAddrs`) and pins
*those IPs* into nftables (`ip daddr <addr> accept`). It does **not** inspect
TLS SNI. Consequences:

- **CDN / round-robin / failover rotation.** A host whose DNS returns different
  IPs over time (most large providers — `api.openai.com`, `cdn-hosted` chat
  gateways, package indices) will silently rotate *off* the pinned set until the
  next `deputyctl network apply` (or the next boot oneshot) re-resolves. During
  that window the agent's request to a *listed, intended* host fails — a
  **false negative** (availability loss), not a bypass.
- **No hostname-level enforcement.** If an attacker who can write `allow_hosts`
  adds `evil.example.com`, every IP that hostname resolves to is allowed —
  including a CDN IP shared with a legitimate host. This is why `allow_hosts`
  is root-writable only and seeded from a baked, audited file.
- **Why not SNI?** SNI-based filtering requires a TLS-intercepting proxy
  (the device would have to MITM its own TLS, install a root CA, and terminate
  every connection) — a materially larger system with its own trust boundary
  (the proxy's CA key) and its own bypass surface. That is **out of M5.5
  scope**; flagged as a future hardening item, not a gap this milestone closes.

**Operator guidance:** under `whitelist`, schedule a periodic re-apply (the boot
oneshot covers reboot; a cron/timer covers long uptimes) so DNS rotation is
followed. `deputyctl doctor network-policy` does not warn on a stale-but-present
pin — it only checks mode/ruleset agreement.

### No DNS-server pinning

The resolver itself (`127.0.0.53`, or whatever `/etc/resolv.conf` points at) is
allowed unconditionally (it is RFC1918/loopback). A malicious or compromised
upstream resolver could return an *attacker IP* for an allow-listed hostname at
apply time, and that attacker IP would then be pinned into nftables. This is
acceptable for a guardrail (the operator chose the resolver) but means whitelist
is **not** a defense against an adversary who controls DNS. Operators in
high-threat environments should pair `whitelist` with a pinned, trusted resolver
or with airgap.

### IPv6

The generator emits IPv4 `ip daddr` rules only (`resolve_host` drops v6,
`network.rs`). A v6-only host will not resolve into a rule. Under `whitelist`
mode the output chain is `policy drop` with no v6 allow rules, so **all v6
egress is denied** — this is a feature (defence-in-depth: v6 cannot be used to
sidestep a v4-only allow-list), but it means a host reachable only over v6 is
unreachable in `whitelist` even if listed. Operators relying on v6 should treat
this as a known limitation until a v6-aware generator lands.

### Pin staleness vs. liveness

Because IPs are pinned at apply time, the pinning reflects the DNS state *at
that moment*. There is no liveness check that a pinned IP still serves the
intended host — a provider decommissioning an IP and reassigning it to a
different tenant would, until the next apply, have traffic to the old IP
allowed under a now-stale label. The blast radius is small (the agent's
outbound to one host breaks or misroutes until re-apply) but non-zero.

## Relationship to `airgap` and `open`

- `open` — `policy accept`. No egress control. The default for connected tiers.
- `whitelist` — `policy drop` + curated `allow_hosts`. The guardrail. This page.
- `airgap` — `policy drop` + RFC1918/mDNS/local-DNS only, no `allow_hosts`. The
  hard boundary. See [`airgap.md`](airgap.md).

`whitelist` and `airgap` share the same `/etc/deputyos/network-policy.json`
schema and the same nftables generator; they differ only in whether
`allow_hosts` is honoured. Switching is atomic and reversible via
`deputyctl network mode <open|whitelist|airgap>` + `deputyctl network apply`.

## Out of scope

- SNI / TLS-intercepting proxy (documented above as a future hardening item).
- v6-aware allow-list generation (documented above as a known limitation).
- Per-process / per-app egress policy (the control is device-wide by design).
- DNS-over-HTTPS / DoT pinning of the resolver itself.
- Live boot-smoke verification of the oneshot re-apply (out-of-sandbox; asserted
  via role files, flagged for a bake smoke).