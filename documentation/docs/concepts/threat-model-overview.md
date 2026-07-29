# Threat Model — Overview

This page is the short orientation. The depth is in the
[Security](../security/default-on-controls.md) section, which enumerates
each default-on control, the secrets-storage contract, and the update
trust chain. Read this page first, then follow the cross-links.

## Adversaries we model

deputyOS images run in a few well-defined places — a Pi on a home LAN, a
mini-PC in a closet, a $5 cloud VM behind a single public IP, a desktop
launcher on a developer laptop. The threats we design against, and that
the [default-on controls](../security/default-on-controls.md) are sized
for, are:

- **Opportunistic LAN-side attackers.** Anyone else on the same Wi-Fi or
  VLAN as the device — a hostile guest, a compromised IoT camera, a
  laptop on a co-working network. They can probe ports, attempt brute
  force on SSH, and try to reach the wizard or the PWA.
- **Opportunistic internet-side attackers**, when the device is exposed
  by user choice (a tunnel, a public IP, a Fly.io machine). They cannot
  reach the device at all by default; if the user has set up a tunnel
  they can reach exactly the surface the user opened.
- **Supply-chain attacks on upstream agents.** A compromised
  `openclaw`, `hermes`, or `khoj` release would make its way through
  the [release loop](architecture.md#the-release-loop) unless we catch
  it. We catch it by pinning upstream tags, by ClamAV / Magika scans
  during bake, by signing what we ship, and by giving the user a third-party
  way to verify (`make verify VERSION=<v>`).
- **Malicious skills and agent-generated code.** Hermes can write
  skills at runtime; agents can be talked into running tools the user
  did not intend. The defenses are the AppArmor profile, the
  unprivileged-userns sandbox Hermes uses for command execution, and
  the cost ledger / quiet hours that bound runaway tool calls.
- **Compromised model-provider credentials.** A leaked OpenRouter or
  Anthropic key is the most common real-world incident. The defenses
  are mode 0600 on `secrets.env`, key rotation via `deputyctl model set`,
  cost caps that blunt blast radius, and CostAlert hooks that surface
  unusual spend immediately.

## Trust boundaries

The system has a small number of distinct trust layers; understanding
where they sit makes the rest of the security documentation easier to
read.

| Layer | What it protects | Who enforces it |
|---|---|---|
| **Signed manifest (cryptographic root)** | The integrity of every image and the legitimacy of every update. | minisign verification in `deputyctl update --check` and the desktop launcher. Public key baked into every image. See [Security → Update trust chain](../security/update-trust-chain.md). |
| **Image bake (build-time integrity)** | What ends up at `/opt/deputyos/profiles/<id>/`, `/var/lib/clamav/`, `/var/cache/deputyos/`. | The CI pipeline; reproducibility via `make verify VERSION=<v>`. SBOM at `/etc/deputyos/sbom.json`. |
| **AppArmor (process)** | What each gateway process can read, write, exec, and connect to. | The kernel, with profiles at `/etc/apparmor.d/deputyos.<id>`. See [Reference → System → AppArmor profiles](../reference/system/apparmor-profiles.md). |
| **ufw (network)** | What inbound traffic the device accepts. Default-deny; allow rules per channel and for the LAN. | The kernel netfilter chain. See [Security → Default-on controls](../security/default-on-controls.md). |
| **`agent` user (filesystem)** | The data partition is owned by `agent`, not root. The gateway runs as `agent`. | POSIX permissions; systemd `User=agent`. |
| **`secrets.env` (mode 0600)** | Provider keys, tunnel tokens, backup credentials. Readable only by root and (via systemd credential forwarding) the gateway. | POSIX permissions, systemd. See [Security → Secrets storage](../security/secrets-storage.md). |
| **`deputyctl` operator surface** | Every command is non-destructive by default; destructive ones (`factory-reset`, `rollback`, `update --apply`) prompt unless `--yes`. | The CLI itself; documented in [Reference → CLI → deputyctl](../reference/cli/deputyctl.md). |

Each layer assumes the layers below it are intact. A kernel-level
compromise breaks AppArmor and the user-id boundary; a manifest-signing-key
compromise breaks every layer below it.

## What's not in scope

Documenting limits explicitly is part of the
[awareness-of-limitations principle](../glossary.md#l). The following
are *not* threats deputyOS attempts to defend against, and the user
should know that:

- **Physical attacks.** A person with the device in their hands can
  swap the SD card or NVMe, attach a JTAG, or read the data partition.
  deputyOS does not enable full-disk encryption by default; opt-in
  encryption is a deferred milestone.
- **Kernel zero-days.** AppArmor is a confinement, not a hypervisor.
  A kernel bug that lets a process escape AppArmor breaks our process
  boundary. We patch via image revs as upstream patches land.
- **Side-channel attacks on shared cloud hosts.** A neighbour on a
  shared Hetzner or Vultr host running Spectre-class attacks is out of
  scope; the cloud provider owns that boundary.
- **Compromise of the user's model-provider account by other means.**
  If your OpenRouter account is breached upstream, your key on the
  device is irrelevant.
- **Covert tracking by the user's ISP.** DNS, NTP, and provider HTTPS
  traffic are visible to the network. deputyOS does not route through
  Tor or a VPN by default.
- **Targeted nation-state adversaries.** This system is sized for the
  threats a self-hosting hobbyist faces, not for high-stakes
  high-resource attackers.

## Layered defenses summary

The full enumeration is in [Security → Default-on controls](../security/default-on-controls.md);
the table below is the orientation map.

| Layer | What it protects | How to verify |
|---|---|---|
| Signed manifest | Every download and update | `deputyctl version` shows the manifest signature and the public key fingerprint; `deputyctl update --check` re-verifies on every update. See [Security → Update trust chain](../security/update-trust-chain.md). |
| Reproducible bake | What's actually in the image | `make verify VERSION=<v>` rebuilds the image and asserts the SHA256 matches. SBOM at `/etc/deputyos/sbom.json`. |
| AppArmor enforce | Process-level confinement | `aa-status` shows every profile in enforce mode. Doctor verifies on boot. See [Reference → System → AppArmor profiles](../reference/system/apparmor-profiles.md). |
| ufw default-deny | Inbound network surface | `ufw status verbose` lists the deny-by-default chain and per-channel allows. |
| fail2ban | Brute-force resistance on SSH and the wizard | `fail2ban-client status` shows the active jails. |
| Hardened sysctls | Kernel attack surface | `/etc/sysctl.d/90-deputyos.conf` is the source of truth. Doctor verifies the values are applied. |
| ClamAV + Magika | Inbound file scanning | `clamdscan --version` (or `clamscan` on RAM-constrained targets); Magika is invoked by the gateway pre-write. |
| `agent` user | Filesystem isolation | `id agent`, `ps -u agent`. The gateway is never run as root. |
| `secrets.env` mode 0600 | Provider key isolation | `stat -c '%a %U' /etc/deputyos/secrets.env` returns `600 root`. |
| Key-only SSH | Remote-access posture | `sshd -T \| grep -E '^(passwordauthentication\|permitrootlogin)'`. |
| ZRAM + cgroup limits + systemd-oomd | Graceful pressure response | `swapon`, `oomctl status`, journal entries. See [Operations → Monitoring and logs](../operations/monitoring-and-logs.md). |
| Cost guardrails | Bounded financial blast radius | `deputyctl cost` shows the ledger; CostAlert hooks fire on threshold breaches. See [Operations → Cost guardrails](../operations/cost-guardrails.md). |

## Where to go next

- [Security → Default-on controls](../security/default-on-controls.md) —
  every default-on control, in detail.
- [Security → Secrets storage](../security/secrets-storage.md) — the
  `/etc/deputyos/secrets.env` contract, how it survives updates, what
  factory-reset does to it.
- [Security → Update trust chain](../security/update-trust-chain.md) —
  the minisign trust root, the CDN URL contract, and the third-party
  verification path.
- [Security → Reporting vulnerabilities](../security/reporting-vulnerabilities.md)
  — the private-disclosure path and the SLA we promise.
- [Concepts → Architecture](architecture.md) — the system view that
  this threat model is layered onto.
