# 09 — Security

deputyOS aims for a security posture that's stronger than what a semi-technical user would assemble themselves on a fresh Pi. The whole baseline is on by default — there is no "I'll harden it later" toggle. This doc enumerates every default-on control and explains the reasoning.

## Threat model (short form)

Assets we protect:

- The user's model-provider API key (and AWS / R2 / Cloudflare credentials, if configured).
- The agent's persistent memory (SQLite DB containing personal conversations).
- The user's LAN — if the device is compromised, it must not become a pivot.

Adversaries we expect:

- Opportunistic internet attackers scanning for exposed services (any device with a public IP gets several thousand of these per day).
- A misbehaving channel input — a Telegram message containing a malicious file, a Discord webhook with a payload designed to confuse the agent's tool-call parser.
- A compromised npm/pypi package upstream of an agent. (We can't catch this in the build, but we can limit blast radius.)
- Local LAN attackers (a roommate or guest device).

Adversaries we explicitly don't try to defeat:

- A user with physical access to the SD card. Disk encryption is opt-in (M6+), not default.
- A nation-state with persistent network access. Audit-log tampering is out of scope.

## Baseline (on by default)

### Authentication

- `agent` is the only user account.
- No default password. The `agent` account is locked unless the wizard collects an SSH public key (preferred) or sets a password.
- SSH key-only, root login disabled, password auth disabled.
- LAN-only `Match Address` block — SSH is only reachable from RFC1918 + tailnet CGNAT ranges by default.
- `fail2ban` watches `auth.log`; 3 failures in 10 min ⇒ 1-hour ban.

### Network

- `ufw` default-deny inbound, allow established/related, deny routed (no IP forwarding by default).
- Only the channel ports the user explicitly enabled have allow rules.
- IPv6 firewall in lockstep.
- `tcp_syncookies=1`, `rp_filter=1`, `accept_redirects=0`, `accept_source_route=0`.
- BBR + fq for performance, not security, but worth mentioning.

### Mandatory access control

- AppArmor in **enforce** mode globally.
- One profile per agent under `/etc/apparmor.d/deputyos.<id>`. Each profile restricts the agent to:
  - read+exec on `/opt/deputyos/profiles/<id>/`
  - read+write on `~/.<id>/` (its own data dir)
  - read+write on `/var/cache/deputyos/<id>/`
  - read on `/etc/deputyos/secrets.env` (mounted via systemd `EnvironmentFile=`, not direct read)
  - network egress only on the configured channel paths
  - **no** `mount`, **no** `ptrace`, **no** access to `/proc/<pid>/mem` for other processes
- The wizard (`deputyctl init`) runs under a separate, more permissive profile that ends as soon as init completes.

### Filesystem

- Slot partitions mount read-only. Writable areas are `tmpfs` overlays for `/var/run`, `/tmp`, etc.
- The data partition mounts with `nosuid,nodev`.
- `/boot/firmware` is mounted read-only after first-boot wizard completion.

### Kernel

`/etc/sysctl.d/90-deputyos.conf` (applied at boot):

```
vm.swappiness                       = 10
vm.vfs_cache_pressure               = 50
vm.dirty_ratio                      = 10
vm.dirty_background_ratio           = 5

net.ipv4.tcp_congestion_control     = bbr
net.core.default_qdisc              = fq
net.ipv4.tcp_syncookies             = 1
net.ipv4.tcp_rfc1337                = 1
net.ipv4.conf.all.rp_filter         = 1
net.ipv4.conf.default.rp_filter     = 1
net.ipv4.conf.all.accept_redirects  = 0
net.ipv4.conf.all.send_redirects    = 0
net.ipv4.conf.all.accept_source_route = 0
net.ipv6.conf.all.accept_redirects  = 0
net.ipv6.conf.all.accept_source_route = 0
net.ipv4.icmp_echo_ignore_broadcasts = 1
net.ipv4.icmp_ignore_bogus_error_responses = 1

kernel.kptr_restrict                = 2
kernel.dmesg_restrict               = 1
kernel.yama.ptrace_scope            = 2
kernel.unprivileged_userns_clone    = 1   # Hermes' command sandbox uses these
kernel.unprivileged_bpf_disabled    = 1
fs.protected_hardlinks              = 1
fs.protected_symlinks               = 1
fs.protected_fifos                  = 2
fs.protected_regular                = 2
fs.suid_dumpable                    = 0
```

`unprivileged_userns_clone` is set to 1 because Hermes' tool-execution sandbox needs it; it's a known trade-off documented in [ADR-0008](adr/0008-clamav-plus-magika-baseline.md).

### Antivirus and content scanning

- **ClamAV** (`clamd`) running by default. Watches `~/.<profile>/uploads/` via `fanotify`. Quarantine quarantines to `/var/quarantine/` (mode 0700, root-owned). Daily scheduled scan at 04:30 local. Signatures shipped in the slot image; `freshclam` is disabled.
- **Magika** (Google's open-source AI content-type detector, Apache 2.0) is invoked by the agent's file-handling path *before* a file from a channel is written. If the detected content-type doesn't match the declared extension, the file is flagged. This catches the "script disguised as a JPEG" class of attack that ClamAV signature-only scanning will miss.

The two complement each other: ClamAV gives signature coverage, Magika gives content-type ground truth. See [ADR-0008](adr/0008-clamav-plus-magika-baseline.md).

### Secrets

- `/etc/deputyos/secrets.env` is mode `0600`, root-owned, loaded into the agent unit via systemd `EnvironmentFile=`.
- `/etc/deputyos/backup.env` likewise.
- Wizard-issued single-use tokens (e.g. for the QR-code provisioning) are mode `0600`, expire in 60 minutes, and are deleted on use.
- TPM-sealed credentials (M6+) are an opt-in path for hardware that has one (DigitalOcean droplets do, Pis do not).
- Wizard never writes credential values to logs. `deputyctl logs` redacts known credential keys before display.
- `/etc/deputyos/api-base` is mode `0644`, root-owned, and **non-secret** — it holds only the API hostname the device registered against. The wizard Account step writes it when the user enters a custom/self-hosted backend (instead of the default `https://api.deputyos.com`); the integrated tunnel (`deputyctl tunnel --integrated`) and the remote command poller (`deputyctl commands poll`) read it back via `deputyctl::apibase` so they reach the same backend. Precedence: `--api-base` flag > `DEPUTYOS_API_BASE` env > `/etc/deputyos/api-base` > default. It is not written when the device uses the default backend.

### Audit and logging

- `auditd` running with a minimal ruleset:
  - `execve` under the `agent` user
  - mounts on the data partition
  - changes to `/etc/deputyos/`
- `journald` retains the last 7 days; older logs are rotated by `deputyctl prune`.
- No outbound logs by default.

### Updates and signing

- Image artefacts signed with `minisign` (project key, kept offline) and `cosign` (Sigstore, GitHub-OIDC-attested).
- Manifest signed with the same keys.
- `deputyctl update` verifies both before applying. Mismatch refuses to install — there is no `--force` flag.

### Disabled by default

- `unattended-upgrades` is **disabled**. The image manages its own updates via A/B; we don't want background `apt` activity changing state under us.
- Background `freshclam` is **disabled** (signatures in image rev).
- Cron `MAILTO`, `crashreporter`, and `apport` are disabled.
- Bluetooth and Avahi reflector are disabled. Avahi (mDNS) advertise is enabled but reflection is off.

## What we do NOT promise (yet)

- **Full-disk encryption** on the data partition — opt-in only. M6+ ships a wizard step that converts the data partition to LUKS using a TPM-sealed key (where available) or a passphrase prompted at boot (a first-time setup we'll only suggest for advanced users).
- **Reproducible builds at SLSA L3** — designed; lands at M7. M0–M6 builds are reproducible-best-effort.
- **Verified boot** on the Pi — the Pi's bootloader doesn't support secure boot in the conventional sense. We add what we can (signed image + watchdog), but a determined attacker with physical access can replace the image. Disk encryption is the answer there.

## Disclosure and audit

- `security@deputyos.com` (target) — 90-day disclosure SLA. Public-facing policy at [SECURITY.md](../SECURITY.md) (created in M5).
- External audit at M6/M7 (Trail of Bits or Cure53). Report published in full.
- Threat-model document published at M0 (this doc + ADRs); kept updated in lockstep with code.

## Doctor checks

`deputyctl doctor` verifies every default-on control listed above and exits non-zero if any of them aren't as expected. The wizard refuses to bring up channels until doctor is green — this is a deliberate guard against shipping a partially-configured device into the world.
