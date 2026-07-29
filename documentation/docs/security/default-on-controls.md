# Default-on security controls

## What this guide does

Enumerate every security control deputyOS enforces by default — what
each control protects against, how to verify it's active, and how to
disable it (with a clear "and why you usually shouldn't"). Every
control on this page is **on by default in every shipped image** and
the role's `security-baseline.yml` plus per-profile bake recipes
guarantee they remain on through normal operation.

The principle: deputyOS treats security as a property of the image, not
of the user's discipline. The user who is curious why their device is
hard to misuse should be able to point at this list and verify each
item directly.

## Control list

### 1. AppArmor enforce on the active profile

**What** — Each shipped profile (OpenClaw, Hermes, Khoj, voice-relay)
has a real-rules AppArmor profile loaded in **enforce** mode. The
profile binds to the gateway binary path and tightly restricts
filesystem reads, writes, network, and capabilities.

**Protects against** — A compromised gateway escalating to read
`/etc/shadow`, write `/etc/sudoers.d/`, ptrace other processes, or
load kernel modules.

**Verify** —

```sh
sudo aa-status                           # lists loaded profiles in enforce mode
sudo dmesg | grep -i apparmor            # any DENIED messages?
```

**Disable** — Edit
`/etc/apparmor.d/deputyos.<profile>`, change `flags=(enforce)` to
`flags=(complain)`, run `apparmor_parser -r` on the file, restart the
gateway. Don't — you lose the strongest single control on the device.

**Reference** —
[Reference → System → AppArmor profiles](../reference/system/apparmor-profiles.md).

### 2. ufw default-deny incoming

**What** — `ufw` is installed and enabled with `default deny incoming
/ allow outgoing`. Specific ports allowed: `22/tcp` (SSH), `8088/tcp`
on `lo` only (wizard / PWA), plus per-profile inbound channel ports.

**Protects against** — Unintended exposure of debug ports, agent
gateway ports, voice relay, journal listeners, and so on.

**Verify** —

```sh
sudo ufw status verbose
```

**Disable** — `sudo ufw disable`. Don't — every channel that needs
inbound traffic explicitly opens its port; the default-deny baseline
is the safety net.

**Reference** —
[Security baseline tasks][secbase].

[secbase]: https://github.com/deputyos/deputyos/blob/main/roles/deputyos/tasks/security-baseline.yml

### 3. fail2ban on sshd

**What** — `fail2ban` watches systemd-journald for sshd auth failures.
Configuration: `maxretry=5`, `bantime=10m`, `findtime=10m`. Banned
IPs are blocked at the firewall layer.

**Protects against** — Brute-force SSH credential guessing.

**Verify** —

```sh
sudo fail2ban-client status sshd
```

**Disable** — `sudo systemctl disable --now fail2ban`. Don't on any
device with public SSH; the bantime + maxretry is conservative.

### 4. Hardened sysctls

**What** — A `90-deputyos.conf` sysctl drop-in sets:

- `kernel.kptr_restrict=2` — kernel symbols hidden from unprivileged
  users.
- `kernel.dmesg_restrict=1` — `dmesg` requires `CAP_SYS_ADMIN`.
- `kernel.unprivileged_bpf_disabled=1` — BPF loading requires root.
- `kernel.yama.ptrace_scope=2` — only privileged processes can ptrace.
- `net.ipv4.tcp_syncookies=1` — SYN-flood resistance.
- `net.ipv4.conf.all.rp_filter=1` — strict reverse-path filtering.
- `net.ipv4.tcp_congestion_control=bbr` — BBR (kernel 5.4+).
- `net.ipv4.tcp_md5sig_pool_size=8` — TCP-MD5 (BGP-style auth) pool.
- IPv6 RA / redirect lockdown.
- Standard `randomize_va_space=2` ASLR confirmation.

**Protects against** — Kernel info leaks, BPF JIT spraying, ptrace
gadget chains, SYN floods, IP spoofing.

**Verify** —

```sh
sudo sysctl -a | grep -E "kptr_restrict|dmesg_restrict|unprivileged_bpf|ptrace_scope|tcp_syncookies"
```

**Disable** — Edit `/etc/sysctl.d/90-deputyos.conf`. Don't — every line
is well-trodden production guidance.

### 5. ClamAV (or on-demand clamscan timer)

**What** — On hosts that can afford it (rpi5+, x86_64-mini-pc, cloud),
`clamav-daemon` runs continuously with `freshclam` updating signatures
hourly. On RAM-constrained hosts (rpi4), an on-demand `clamscan`
systemd timer scans `/home` + `/var/lib/deputyos` daily with a 30-min
randomized delay.

**Protects against** — User-uploaded files (Khoj content dir, voice
recordings, etc.) carrying known malware.

**Verify** —

```sh
# Daemon mode
sudo systemctl status clamav-daemon

# Timer mode (rpi4)
sudo systemctl status deputyos-clamscan.timer
```

**Disable** — `sudo systemctl disable --now clamav-daemon`. Don't — a
single AV layer is the cheapest signal you'll get on a multi-channel
device.

### 6. Magika file-type detection

**What** — Google's Magika is installed in a venv at
`/opt/deputyos/magika/`. Each profile that accepts file uploads runs
the file through Magika before processing — content-type sniffing on
the bytes, not the extension. Wraps in the AppArmor profile for the
gateway.

**Protects against** — `.png` files that are actually executables,
`.txt` files containing zip-bombs, etc.

**Verify** —

```sh
/opt/deputyos/magika/bin/magika /path/to/file.txt
```

**Disable** — Skip the upload step; the upstream agent has its own
fallback. Don't — Magika is a cheap, accurate first-line filter.

### 7. SSH hardening

**What** — `/etc/ssh/sshd_config.d/10-deputyos.conf` sets:

- `Protocol 2`
- `PasswordAuthentication no`
- `PermitRootLogin prohibit-password`
- `KbdInteractiveAuthentication no`
- `MaxAuthTries 3`
- `LoginGraceTime 30`

**Protects against** — Password brute-force, root password fishing,
weak key-distribution mistakes.

**Verify** —

```sh
sudo sshd -T | grep -E "passwordauth|permitroot|maxauth"
```

**Disable** — Edit the drop-in. Don't — re-enabling
`PasswordAuthentication yes` on a public IP is the fastest way to lose
the device.

### 8. zram swap

**What** — `zram-tools` is installed and enabled.
`/etc/default/zramswap` sets `ALGO=lz4`, `PERCENT=50`. On RAM-
constrained hosts (rpi4 / rpi5-4gb), this gives ~2× effective RAM at
modest CPU cost.

**Protects against** — Service OOM-kills under transient memory
pressure (Khoj indexing a large doc set, embeddings reload, etc.).

**Verify** —

```sh
zramctl
```

**Disable** — `sudo systemctl disable --now zramswap`. Don't on
RAM-constrained hosts; the OOM-kill is a worse outcome than a brief
slowdown.

### 9. `agent` system user, no password

**What** — The `agent` user is created during the bake with no
password (`!`-locked), no shell login (`/usr/sbin/nologin`), and
membership in `agent` plus (where needed) `audio` / `systemd-journal`
groups. All gateway services run as `User=agent`.

**Protects against** — Any privilege escalation from a compromised
gateway is bounded by the agent user's actual permissions.

**Verify** —

```sh
sudo grep '^agent:' /etc/passwd
sudo grep '^agent:' /etc/shadow      # password field is `!`
```

**Disable** — Don't.

### 10. `secrets.env` mode 0600

**What** — `/etc/deputyos/secrets.env` is created with mode `0600`
owner `agent:agent`. The atomic-write contract in `deputyctl model
set` preserves this on every rotation. AppArmor allows `r,` for the
gateway binary only.

**Protects against** — Accidental leaks via world-readable mode,
backup tooling that follows symlinks, scrape attacks from other
processes.

**Verify** —

```sh
ls -l /etc/deputyos/secrets.env
```

**Disable** — Don't. See
[Security → Secrets storage](secrets-storage.md) for the full contract.

### 11. mDNS scoped to LAN (no reflector)

**What** — `avahi-daemon` is configured with
`enable-reflector=no`, `enable-wide-area=no`, `publish-hinfo=no`.
mDNS publishes `deputyos.local` on the LAN only; never bridges across
interfaces or proxies queries.

**Protects against** — A device on a public WiFi unintentionally
advertising its hostname to the broader internet via a misconfigured
reflector.

**Verify** —

```sh
sudo avahi-browse -arp | head -20
sudo grep -E "reflector|wide-area" /etc/avahi/avahi-daemon.conf
```

**Disable** — Edit the avahi config. Don't — reflector mode has
real-world security history.

### 12. Hooks dispatcher 5s timeout + bounded stderr

**What** — Every user hook runs with a 5-second wall-clock timeout
(SIGKILL on overrun) and the last 1024 bytes of stderr captured. See
`deputyctl/src/hooks.rs`.

**Protects against** — A buggy or hostile user hook hanging the
message path. A chatty hook flooding the journal.

**Verify** — Drop a hook that does `sleep 10`; observe it gets killed
at 5s and the dispatcher continues to the next hook.

**Disable** — Edit `HOOK_TIMEOUT` in `hooks.rs`. Don't — the timeout
is a load-bearing contract.

## Verification one-liner

```sh
sudo deputyctl doctor --json | jq '.checks[] | select(.status != "ok")'
```

`deputyctl doctor` walks every control above (and a handful more) and
emits a structured pass/fail per check. `jq`'s `select` filters to the
failures.

## Common drifts

The controls drift over time when:

- A user disables `ufw` to debug something and forgets to re-enable.
  `deputyctl doctor` flags this.
- A user `chmod 644` `secrets.env` to copy with non-root tools. Fix:
  `chmod 0600`.
- A user installs a different AppArmor profile that overrides the
  deputyos one (rare, but possible with the same profile name in a
  later-loaded file).
- The journal fills up and `journalctl --vacuum-time` loses some log
  history that fail2ban needed.

`deputyctl doctor` catches the first two; the others surface in
`deputyctl status` and the dashboard.

## Related

- [Reference → System → AppArmor profiles](../reference/system/apparmor-profiles.md)
- [Reference → System → systemd units](../reference/system/systemd-units.md)
- [Reference → System → Filesystem layout](../reference/system/filesystem-layout.md)
- [Security → Secrets storage](secrets-storage.md)
- [Security → Update trust chain](update-trust-chain.md)
- [Concepts → Threat model overview](../concepts/threat-model-overview.md)
