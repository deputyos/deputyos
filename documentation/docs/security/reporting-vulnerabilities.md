# Reporting vulnerabilities

## What this page does

Tell a security researcher how to report a vulnerability to the
deputyOS project, what the project promises in return, and what is
out-of-scope.

## How to report

Email **`security@deputyos.com`**.

Include:

- A clear description of the vulnerability.
- Reproduction steps (an exploit script if available).
- Affected deputyOS release versions, target(s), and profile(s).
- Your preferred credit name (or "anonymous").
- Whether you intend to publish; if so, your intended date.

PGP encryption is welcome but not required. The project's PGP key is
published at `https://www.deputyos.com/.well-known/pgp-key.asc` (the
`well-known` path lands during the M5 rollout).

!!! note "M5 deferral"
    The `security@deputyos.com` inbox plus the `well-known` PGP path
    are scheduled for the M5 rollout. Until M5 lands, file an issue
    against the GitHub repository at `deputyos/deputyos` marked
    "[security]" in the title and an explicit note that you are
    requesting private disclosure. A maintainer will respond within
    72 hours with a private channel.

## What the project promises

- **Acknowledgment within 72 hours** of receiving your report.
- **Status updates every 7 days** while the report is being
  triaged or fixed.
- **Named credit** (or anonymous, your choice) in the release notes
  for the fixing release.
- **CVE assignment** for confirmed issues affecting deputyOS code.
- **Stable backport** to every supported release channel for
  high-severity fixes (today: only `dev` is supported; `beta` and
  `stable` channels land in M5).
- **Coordinated disclosure timeline** — deputyOS targets a 90-day
  disclosure window from acknowledgment. We will not blow past 90
  days without your explicit consent.

## What the project does not promise

- **No bug bounty.** deputyOS does not run a paid bounty program.
- **No coverage of upstream agent vulnerabilities.** Bugs in
  OpenClaw / Hermes / Khoj themselves go to those projects' upstream
  security contacts. deputyOS is the appliance layer; the upstream
  agent's security model is a property of that project. We will,
  however, ship configuration mitigations (e.g. tightening an
  AppArmor profile to neutralize an upstream issue) where
  appropriate.
- **No coverage of self-hosted infrastructure issues.** Misconfiguring
  a Cloudflare Tunnel, a Tailscale ACL, or your DNS is on you.
- **No coverage of provider issues.** OpenAI / Anthropic / OpenRouter
  / Bedrock outages, key leaks at those providers, or data-handling
  incidents at those providers go to those providers.

## Severity guidance

deputyOS uses CVSS v3.1 scoring with the following rough buckets, used
for prioritization not for SLA:

| Severity | Examples |
| --- | --- |
| Critical | Remote unauthenticated code execution; AppArmor confinement bypass leading to root |
| High | Authenticated path to read `/etc/deputyos/secrets.env`; signature-bypass in update path |
| Medium | Information leak from a single profile's logs; cost-cap bypass |
| Low | Hardening drift in a non-default configuration |

A "scaffold-only" issue (M-deferred functionality known to be
incomplete and visibly labelled so) is not a vulnerability — see the
roadmap. A "future-dated" issue (a vulnerability that only manifests
once an M-deferred feature lands) is welcome and we'll track it
toward that milestone.

## Public-disclosure timeline expectations

```text
Day 0     report received
Day <=3   acknowledgment + initial assessment
Day <=14  fix in development OR out-of-scope decision communicated
Day <=30  fix landed in `dev` channel
Day <=60  fix backported to `beta` / `stable` (when those exist; M5)
Day 90    public disclosure (or sooner with consent)
```

If you need to disclose before day 90 because of a third-party
constraint (CVE pre-disclosure, coordinated industry advisory), tell
us in the report. We will work with you.

## Hall of fame

Researchers who reported confirmed vulnerabilities are credited in
`SECURITY.md` (lands during M5 alongside the `security@deputyos.com`
inbox). If you'd prefer to remain anonymous, say so in your report.

## Related

- [Concepts → Threat model overview](../concepts/threat-model-overview.md)
- [Security → Default-on controls](default-on-controls.md)
- [Security → Update trust chain](update-trust-chain.md)
- [Contributing → Overview](../contributing/overview.md)
