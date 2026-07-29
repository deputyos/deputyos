# Air-gapped (fat) tier

The air-gapped tier is the truest expression of the *batteries-included*
rule. The image works without ever reaching the public internet:

- `apt` sources point at `/opt/deputyos/airgap/apt-mirror/`, a baked-in
  Debian/Raspbian snapshot frozen at image-bake time.
- `nftables` denies all egress except RFC1918 + mDNS + the local DNS
  resolver. Outbound chats with `api.openai.com` / `api.anthropic.com`
  /etc. are blocked.
- A tier-appropriate **LFM2** GGUF is baked under
  `/var/lib/deputyos/models/` and served by `llama.cpp` on
  `127.0.0.1:8090`. Wizard + chat default to it.

## Tier sizes

| Tier     | Default LLM                                | Compressed image |
|----------|--------------------------------------------|------------------|
| lean     | `LFM2-350M-Q4_K_M.gguf` (~250 MB)          | ~1.3 GB          |
| standard | `LFM2-1.2B-Q4_K_M.gguf` (~750 MB)          | ~2.8 GB          |
| rich     | `LFM2-2.6B-Q4_K_M.gguf` (~1.6 GB) + `Qwen2.5-Coder-1.5B-Instruct-Q4_K_M.gguf` (~1.0 GB) | ~6.5 GB |

Exact numbers in `docs/12-bundled-software.md` §5.

## Building

```bash
make build TARGET=qemu-x86_64 PROFILE=openclaw TIER=rich AIRGAP=1
```

`AIRGAP=1` plumbs through Packer (`-var airgap=1`) into the role
(`deputyos_airgap: true`). When false (the default), every airgap-related
task is a no-op so non-airgap targets pay zero baking cost.

## Unlocking the network post-boot

Air-gapped is the *posture*, not the *prison*. From the device:

```bash
deputyctl network unlock      # flip mode=airgap → mode=open, reload nftables
```

The wizard surfaces the same toggle. Going back the other way
(`deputyctl network lock --airgap`) is also supported.

A middle mode — `whitelist` — allows a curated host list instead of deny-all;
it shares the same `/etc/deputyos/network-policy.json` schema and nftables
generator. See the [egress threat model](threat-model-egress.md) and the
[egress how-to](../how-to/operate/egress-whitelist.md).

## Updates over sneakernet

`deputyctl update --from /mnt/deputyos/<usb>/manifest.json` consumes a
manifest dropped onto a USB stick. The signature path is unchanged:
minisign + cosign + SLSA verification still gate the apply step. The
device just doesn't fetch the bytes — the user does.

## Caveats

- The baked apt mirror is frozen at image bake. Security patches arrive
  via signed image rebuilds, not in-place package upgrades.
- Cloud-API LLM providers are unreachable in airgap mode by design. The
  baked LFM2 (and any GGUF you `deputyctl model register` later) is the
  only path.
- Pi 4 + airgap is supported only in the `lean` tier — the SoC can't
  run 1.2B at usable t/s.

## Threat-model delta (M4.5)

The air-gapped tier does not introduce a new threat model so much as
*remove* assumptions the connected tiers rely on. This section records the
deltas an operator or auditor should weigh against the base
[threat model](threat-model-overview.md). It is the Lane-E
threat-model-before-code record for the airgap posture.

### nftables, not ufw

Egress is enforced by **nftables alone** (`/etc/nftables.conf`, applied by
`nftables.service`). We deliberately do *not* layer `ufw` on top, even though
ufw is the friendlier UI:

- **Dual-firewall conflict.** ufw is a frontend for iptables, and on modern
  kernels iptables calls into the same nftables backend via
  `iptables-nft`. Running both means two rulesets own the same hooks; an
  `allow` in one can silently punch a hole the other intended to close, and
  `ufw reload` reordering rules has historically reordered nftables chains
  out from under a hand-authored ruleset. A single source of truth — one
  `flush ruleset` + one declared chain set — is easier to reason about than
  two reconcilers fighting.
- **No reconciliation engine to drift.** nftables is declarative from a
  file; there is no "did the GUI and the file diverge?" question. The file
  *is* the policy.

The cost is operator ergonomics: `deputyctl network mode <open|airgap>` and
the PWA Network card are the friendly surfaces, not `ufw status`. We accept
that.

### Boot-time `flush ruleset` — no drift accumulation

`/etc/nftables.conf` begins with `flush ruleset`. This is load-bearing, not
cosmetic: every boot (and every `deputyctl network` mutation) re-declares the
whole posture from scratch. Consequences:

- A rule a compromised process smuggled in at runtime does **not** survive a
  reboot — the next `nft -f /etc/nftables.conf` wipes it.
- The baked airgap ruleset cannot be "amended" into a hole by accumulated
  edits; the on-disk file is the only durable input.
- The trade-off: legitimate runtime-added rules (e.g. a transient allow for a
  one-off fetch) are also wiped on reboot. That is the intended posture —
  airgap is the default, openness is an explicit, logged, reversible act via
  `deputyctl network unlock`.

### Secrets stay local — no credential egress path

The wizard's airgap provider step collects **no API key** (the model is
baked, served on loopback by `deputyos-llamacpp@`). With no key collected,
there is no `secrets.env` entry for a cloud provider and no token to exfiltrate
over an egress the firewall wouldn't allow anyway. The airgap posture and
the no-account M8 rule reinforce each other: nothing the agent holds can
reach a cloud API, because (a) the agent holds no cloud credential and
(b) the path to one is closed.

The one credential surface that remains — `deputyctl model register`'s
GGUF + per-instance env file under `/opt/deputyos/airgap/models/` — is not a
secret (it names a port and a filename). It carries no key material.

### Sneakernet does not weaken the update trust chain

`deputyctl update --from <usb>/manifest.json` reuses the *same* minisign +
cosign + SLSA verification the connected path uses; the only thing that
changes is who fetches the bytes (the operator, via USB, instead of the
device via HTTPS). In fact the airgap posture *removes* one trust assumption
the connected tier makes — there is no outbound TLS connection to a CDN whose
cert/CA the device must trust at apply time. The trust root is the minisign
public key baked into the image (`/etc/deputyos/deputyos-pubkey.pub`), and the
chain of custody for the manifest on the USB is the operator's to vouch for
(out of band). The signature gate is unchanged; the *transport* is.

### Residual risks (honest)

- **USB as a supply-chain vector.** The sneakernet manifest is signed, but the
  *medium* is physical. A hostile USB can still DoS (corrupt the manifest) or
  attempt to abuse any parser bug in the verify path. Mitigation: the apply
  step refuses unsigned/changed-signature manifests outright; it never runs
  payload before the signature check passes.
- **No revocation channel.** A baked key or a baked apt mirror cannot be
  revoked over the air. Rotation is a signed image rebuild + re-sneakernet.
  Operators should treat the baked minisign pubkey's rotation as a physical
  event.
- **Local LLM output is untrusted-by-default.** The baked model has no
  alignment filter the operator didn't bake themselves. Treat agent output as
  you would any other tool output: validate before acting. The airgap
  removes the *network* exfiltration path, not the local-LLM prompt-injection
  surface.
