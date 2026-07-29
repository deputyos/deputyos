# AppArmor profiles

deputyOS ships **four AppArmor profiles**, one per long-lived process
that needs confinement. They are loaded by `apparmor.service` at boot
and attach by binary path — the same path the corresponding
[systemd unit](systemd-units.md) `ExecStart`s.

| Profile | Binary path | Companion unit | Source |
|---|---|---|---|
| `deputyos.openclaw` | `/opt/deputyos/profiles/openclaw/node_modules/.bin/openclaw` | [openclaw-gateway.service](systemd-units.md#openclaw-gatewayservice) | `roles/deputyos/files/apparmor/deputyos.openclaw` |
| `deputyos.hermes` | `/opt/deputyos/profiles/hermes/.venv/bin/hermes` | [hermes-gateway.service](systemd-units.md#hermes-gatewayservice) | `roles/deputyos/files/apparmor/deputyos.hermes` |
| `deputyos.khoj` | `/opt/deputyos/profiles/khoj/.venv/bin/khoj` | [khoj-gateway.service](systemd-units.md#khoj-gatewayservice) | `roles/deputyos/files/apparmor/deputyos.khoj` |
| `deputyos.voice-relay` | `/opt/deputyos/voice/voice-relay.sh` | [deputyos-voice-relay.service](systemd-units.md#deputyos-voice-relayservice) | `roles/deputyos/files/apparmor/deputyos.voice-relay` |

All four ship in `flags=(enforce)` and apply systemd hardening
**plus** filesystem-path mediation — both must allow an action for it
to succeed.

[TOC]

## Install + reload

The bake role copies each file to `/etc/apparmor.d/`:

```
/etc/apparmor.d/deputyos.openclaw
/etc/apparmor.d/deputyos.hermes
/etc/apparmor.d/deputyos.khoj
/etc/apparmor.d/deputyos.voice-relay
```

To reload after edit:

```sh
apparmor_parser -r /etc/apparmor.d/deputyos.openclaw
```

`make doctor` (and `deputyctl doctor`) check that profiles are loaded
and in enforce mode.

## Common pattern

Every profile follows the same five-section shape:

1. **Includes**: `<tunables/global>`, then per-language abstractions
   (`abstractions/base`, `abstractions/python`, `abstractions/openssl`,
   …).
2. **Interpreter + entrypoint** (`ix` exec).
3. **Install tree** (read; `mr` for native modules; `ix` for the venv's
   `bin/`).
4. **Owner-only data dir** (`rwk` so the agent can lock files).
5. **deputyOS shared config** (`/etc/deputyos/<things>` read-only).
6. **Common system readables** (`/etc/ld.so.cache`, `/proc/cpuinfo`, …).
7. **Network** rules.
8. **Capabilities** (mostly `deny`; Hermes is the exception).
9. **Risky-path denylist** (`/tmp/** wx`, `/root/** rwlk`,
   `/etc/shadow`, …).
10. **Self-only signal + ptrace**.

Differences flagged below per profile.

## `deputyos.openclaw`

Binary: `/opt/deputyos/profiles/openclaw/node_modules/.bin/openclaw`.

```apparmor
profile deputyos.openclaw /opt/deputyos/profiles/openclaw/node_modules/.bin/openclaw flags=(enforce) {
  #include <abstractions/base>
  #include <abstractions/nameservice>
  #include <abstractions/openssl>

  # Node.js — the real openclaw binary is a #!/usr/bin/env node shim.
  /usr/bin/node                                       ix,
  /usr/local/bin/node                                 ix,
  /usr/bin/env                                        ix,

  # Install tree (read+exec). Native modules ship as .node and shared objects.
  /opt/deputyos/profiles/openclaw/**                   r,
  /opt/deputyos/profiles/openclaw/**/*.node            mr,
  /opt/deputyos/profiles/openclaw/**.so*               mr,
  /opt/deputyos/profiles/openclaw/node_modules/.bin/*  ix,

  # stub fallback — remove when real openclaw bake is reliable.
  /usr/bin/bash    ix,
  /usr/bin/nc.openbsd ix,
  /usr/bin/sleep   ix,
  /usr/bin/dash    ix,

  # Data dir (read+write, owner-only).
  owner /home/agent/.openclaw/      rw,
  owner /home/agent/.openclaw/**    rwk,

  # deputyOS shared config.
  /etc/deputyos/                       r,
  /etc/deputyos/secrets.env            r,
  /etc/deputyos/profiles/              r,
  /etc/deputyos/profiles/openclaw.toml r,
  /etc/deputyos/profiles/openclaw/     r,
  /etc/deputyos/profiles/openclaw/**   r,
  /etc/deputyos/limits.json            r,

  # Common readables (truncated; see source).
  /etc/ld.so.cache                    r,
  /etc/resolv.conf                    r,
  /proc/cpuinfo                       r,
  /proc/meminfo                       r,
  …

  # Network — agent listens on channel ports and reaches out to providers.
  network inet stream,
  network inet dgram,
  network inet6 stream,
  network inet6 dgram,
  network unix stream,
  network unix dgram,
  network netlink raw,

  # Capabilities — none beyond what abstractions/base grants.
  deny capability sys_admin,
  deny capability sys_module,
  deny capability sys_ptrace,
  deny capability sys_rawio,
  deny capability dac_override,
  deny capability dac_read_search,
  deny capability mac_admin,
  deny capability mac_override,

  # Deny risky paths.
  deny /tmp/**           wx,
  deny /var/tmp/**       wx,
  deny /root/**          rwlk,
  deny /home/[^a]*/**    rwlk,
  deny /etc/shadow       rwlk,
  deny /etc/sudoers      rwlk,
  deny /etc/sudoers.d/** rwlk,

  # Signals + ptrace (self only).
  signal (send,receive) peer=deputyos.openclaw,
  ptrace (read,trace)   peer=deputyos.openclaw,
}
```

### Rationale walk

- **Node interpreter `ix`**: the real `openclaw` binary is a
  `#!/usr/bin/env node` shim, so we must allow node + env to be exec'd
  with the *inherit* flag (so they stay in this profile, not the
  unconfined one).
- **`/opt/deputyos/profiles/openclaw/**` r + `**/*.node mr + **.so* mr`**:
  read everything; only mark native shared objects executable for
  mmap (`m`).
- **Owner-only data dir**: `owner /home/agent/.openclaw/**  rwk` —
  read, write, and lock files (the `k` flag). The `owner`
  qualifier means the rule fires only if the file is owned by the same
  uid as the running process.
- **No `/home/[^a]*/**`**: the deny rule pattern blocks every home
  except the agent's. We allow `agent`'s only via the explicit
  `owner /home/agent/...` rule above.
- **All capabilities denied**: OpenClaw is a Node web server with no
  privilege-bearing operations. `dac_override` denial in particular
  ensures it can't read mode-0600 files outside its `owner` scope
  even if it briefly gains uid 0 (which it shouldn't).

## `deputyos.hermes`

Binary: `/opt/deputyos/profiles/hermes/.venv/bin/hermes`.

Two **deliberate relaxations** vs. OpenClaw:

1. **`capability sys_admin` and `sys_ptrace` granted (not denied)**:
   Hermes' skill executor spawns sandboxed children via
   `unshare(CLONE_NEWUSER)`; that needs `sys_admin` and `sys_ptrace`
   for seccomp-instrumentation of the child. The kernel further gates
   `unshare(CLONE_NEWUSER)` via `kernel.unprivileged_userns_clone`,
   which Hermes' [`[kernel].required_sysctls`](../schemas/profile-toml.md#kernel-sysctl-prerequisites-optional)
   sets to `1`. The systemd unit's `RestrictNamespaces=~mnt cgroup pid`
   keeps mount/cgroup/pid namespaces blocked.
2. **`/home/agent/.hermes/skills/** ix`**: the self-improvement loop
   exec's authored skill scripts. `/tmp/**` and `/var/tmp/**` keep the
   `deny wx` so a compromised skill can't write+exec from a
   world-writable path.

```apparmor
profile deputyos.hermes /opt/deputyos/profiles/hermes/.venv/bin/hermes flags=(enforce) {
  #include <abstractions/base>
  #include <abstractions/python>
  #include <abstractions/nameservice>
  #include <abstractions/openssl>

  # Python interpreter
  /usr/bin/python3.11                                 ix,
  /usr/bin/python3                                    ix,
  /opt/deputyos/profiles/hermes/.venv/bin/python3*     ix,
  /opt/deputyos/profiles/hermes/.venv/bin/hermes       ix,
  /usr/bin/env                                        ix,

  # Install tree
  /opt/deputyos/profiles/hermes/**                     r,
  /opt/deputyos/profiles/hermes/**.so*                 mr,
  /opt/deputyos/profiles/hermes/.venv/lib/**.so*       mr,
  /opt/deputyos/profiles/hermes/.venv/lib/**/*.so*     mr,
  /opt/deputyos/profiles/hermes/.venv/bin/*            ix,

  # Data dir
  owner /home/agent/.hermes/             rw,
  owner /home/agent/.hermes/**           rwk,
  # FTS5 SQLite session store
  owner /home/agent/.hermes/memory/      rw,
  owner /home/agent/.hermes/memory/**    rwk,
  # Skill files (Hermes authors at runtime)
  owner /home/agent/.hermes/skills/      rw,
  owner /home/agent/.hermes/skills/**    rwk,
  # Allow exec inside the skills tree (the self-improvement loop runs them).
  /home/agent/.hermes/skills/**          ix,

  # deputyOS shared config + Hermes-specific config dir
  /etc/deputyos/                          r,
  /etc/deputyos/secrets.env               r,
  /etc/deputyos/profiles/hermes.toml      r,
  /etc/deputyos/hermes/                   r,
  /etc/deputyos/hermes/**                 r,
  /etc/deputyos/limits.json               r,
  …
  /proc/sys/kernel/unprivileged_userns_clone   r,    # gate-check at runtime

  # Network
  network inet stream,
  network inet dgram,
  network inet6 stream,
  network inet6 dgram,
  network unix stream,
  network unix dgram,
  network netlink raw,

  # Capabilities — Hermes-specific grants
  capability sys_admin,
  capability sys_ptrace,
  deny capability sys_module,
  deny capability sys_rawio,
  deny capability dac_override,
  deny capability dac_read_search,
  deny capability mac_admin,
  deny capability mac_override,

  # Deny risky paths.
  deny /tmp/**           wx,
  deny /var/tmp/**       wx,
  deny /root/**          rwlk,
  deny /home/[^a]*/**    rwlk,
  deny /etc/shadow       rwlk,
  deny /etc/sudoers      rwlk,
  deny /etc/sudoers.d/** rwlk,

  # Signals + ptrace (self only — child sandboxes share the profile).
  signal (send,receive) peer=deputyos.hermes,
  ptrace (read,trace)   peer=deputyos.hermes,
}
```

## `deputyos.khoj`

Binary: `/opt/deputyos/profiles/khoj/.venv/bin/khoj`.

Strict like OpenClaw — Khoj has no skill-child sandbox. `sys_admin`
and `sys_ptrace` stay denied. Two Khoj-specific data directories:

- `owner /home/agent/.khoj/content/` — indexed PDFs, markdown, org-mode
  the user imports.
- `owner /home/agent/.khoj/skills/` — JSON persona/tool configs.
  **Note**: not `ix`-allowed. If a future Khoj release adds Python
  skill plugins, the profile will need to revisit.

```apparmor
profile deputyos.khoj /opt/deputyos/profiles/khoj/.venv/bin/khoj flags=(enforce) {
  #include <abstractions/base>
  #include <abstractions/python>
  #include <abstractions/nameservice>
  #include <abstractions/openssl>

  # Python interpreter
  /usr/bin/python3.11                                 ix,
  /opt/deputyos/profiles/khoj/.venv/bin/python3*       ix,
  /opt/deputyos/profiles/khoj/.venv/bin/khoj           ix,

  # Install tree
  /opt/deputyos/profiles/khoj/**                       r,
  /opt/deputyos/profiles/khoj/**.so*                   mr,
  /opt/deputyos/profiles/khoj/.venv/lib/**.so*         mr,
  /opt/deputyos/profiles/khoj/.venv/lib/**/*.so*       mr,
  /opt/deputyos/profiles/khoj/.venv/bin/*              ix,

  # Data dir
  owner /home/agent/.khoj/                            rw,
  owner /home/agent/.khoj/**                          rwk,
  owner /home/agent/.khoj/memory/                     rw,
  owner /home/agent/.khoj/memory/**                   rwk,
  # JSON persona/tool configs (NOT ix-allowed)
  owner /home/agent/.khoj/skills/                     rw,
  owner /home/agent/.khoj/skills/**                   rwk,
  # Indexed user content
  owner /home/agent/.khoj/content/                    rw,
  owner /home/agent/.khoj/content/**                  rwk,

  # deputyOS shared config + Khoj-specific config dir
  /etc/deputyos/profiles/khoj.toml                     r,
  /etc/deputyos/khoj/                                  r,
  /etc/deputyos/khoj/**                                r,
  /etc/deputyos/limits.json                            r,
  /etc/mime.types                                     r,
  …

  # Network
  network inet stream,
  network inet dgram,
  network inet6 stream,
  network inet6 dgram,
  network unix stream,
  network unix dgram,
  network netlink raw,

  # Capabilities — strict
  deny capability sys_admin,
  deny capability sys_module,
  deny capability sys_ptrace,
  deny capability sys_rawio,
  deny capability dac_override,
  deny capability dac_read_search,
  deny capability mac_admin,
  deny capability mac_override,

  # Risky-path denylist (same as openclaw / hermes)
  deny /tmp/**           wx,
  deny /var/tmp/**       wx,
  deny /root/**          rwlk,
  deny /home/[^a]*/**    rwlk,
  deny /etc/shadow       rwlk,
  deny /etc/sudoers      rwlk,
  deny /etc/sudoers.d/** rwlk,

  signal (send,receive) peer=deputyos.khoj,
  ptrace (read,trace)   peer=deputyos.khoj,
}
```

## `deputyos.voice-relay`

Binary: `/opt/deputyos/voice/voice-relay.sh`.

Specialised profile for the audio bridge. Required relaxations:

- **`/dev/snd/{,**} rw`**: live audio capture and playback. Without
  this, the service can't open the soundcard regardless of group
  membership.
- **`/run/deputyos/relay.sock rw`**: talks to the message-relay socket
  as a `pre-message` hook with `source=voice` in the payload.
- **`abstractions/audio` include**: ALSA's standard mediation set.
- **`network inet stream/dgram`**: kept open for future cloud-STT
  options. tiny.en is fully offline so inet is never used in the M6
  default path.

Notably: **`deny capability sys_nice`**. The audio group + `Nice=-5`
in the unit cover latency without elevating privileges. **`deny
capability sys_admin`** so the profile cannot reroute `/dev/snd` via
mount tricks.

```apparmor
profile deputyos.voice-relay /opt/deputyos/voice/voice-relay.sh flags=(enforce) {
  #include <abstractions/base>
  #include <abstractions/nameservice>
  #include <abstractions/audio>
  #include <abstractions/openssl>

  # Shell + helpers voice-relay.sh shells out to.
  /usr/bin/bash       ix,
  /usr/bin/awk        ix,
  /usr/bin/sed        ix,
  /usr/bin/jq         ix,
  /usr/bin/nc.openbsd ix,
  /usr/bin/aplay      ix,
  …

  # Voice install tree
  /opt/deputyos/voice/                  r,
  /opt/deputyos/voice/**                r,
  /opt/deputyos/voice/voice-relay.sh    ixr,
  /opt/deputyos/voice/whisper-cli       ix,
  /opt/deputyos/voice/piper             ix,
  /opt/deputyos/voice/**.so*            mr,

  # deputyOS shared config (read-only)
  /etc/deputyos/voice.toml              r,
  /etc/deputyos/secrets.env             r,
  /etc/deputyos/limits.json             r,

  # Soundcard — required relaxation
  /dev/snd/                            r,
  /dev/snd/*                           rw,
  /dev/snd/**                          rw,
  /proc/asound/                        r,
  /proc/asound/**                      r,
  /sys/class/sound/                    r,

  # Runtime state
  /run/deputyos/                        r,
  /run/deputyos/relay.sock              rw,
  owner /var/lib/deputyos/              rw,
  owner /var/lib/deputyos/**            rwk,

  # Network — unix stream for relay; inet stream/dgram for any future cloud-STT
  network unix stream,
  network unix dgram,
  network inet stream,
  network inet dgram,
  network inet6 stream,
  network inet6 dgram,
  network netlink raw,

  # Capabilities — none beyond abstractions/audio
  deny capability sys_admin,
  deny capability sys_module,
  deny capability sys_ptrace,
  deny capability sys_rawio,
  deny capability sys_nice,
  deny capability dac_override,
  deny capability dac_read_search,
  deny capability mac_admin,
  deny capability mac_override,

  # Risky-path denylist
  deny /tmp/**           wx,
  deny /var/tmp/**       wx,
  deny /root/**          rwlk,
  deny /home/[^a]*/**    rwlk,
  deny /etc/shadow       rwlk,
  deny /etc/sudoers      rwlk,
  deny /etc/sudoers.d/** rwlk,

  signal (send,receive) peer=deputyos.voice-relay,
  ptrace (read,trace)   peer=deputyos.voice-relay,
}
```

## Profile-class differences at a glance

| Concern | OpenClaw | Hermes | Khoj | voice-relay |
|---|---|---|---|---|
| Interpreter | Node | Python venv | Python venv | bash + helpers |
| Skill child sandbox | – | yes (`unshare(CLONE_NEWUSER)`) | – | – |
| `capability sys_admin` | deny | **allow** | deny | deny |
| `capability sys_ptrace` | deny | **allow** | deny | deny |
| `RestrictNamespaces` (unit) | strict | `~mnt cgroup pid` | strict | strict |
| Skills tree `ix` | – | `~/.hermes/skills/** ix` | – (JSON only) | – |
| `/dev/snd/** rw` | – | – | – | **yes** |
| Network inet stream | yes | yes | yes | yes (future cloud-STT) |
| `/etc/deputyos/<id>/` config dir | – | yes | yes | – |

## Operations

- **Set complain mode** (debug only): `aa-complain /etc/apparmor.d/deputyos.<id>` then `apparmor_parser -r`.
- **Set enforce**: `aa-enforce /etc/apparmor.d/deputyos.<id>` then `apparmor_parser -r`.
- **Inspect status**: `apparmor_status` lists profiles in enforce/complain mode and which processes they confine.
- **Bake-time mode toggle**: `deputyos_apparmor_mode` Ansible variable (default `enforce`; `complain` permitted only on `fly-machines`, where the host doesn't grant `--privileged`).

## See also

- [Reference / System / systemd units](systemd-units.md) — the unit's
  `ExecStart=` is the path each profile binds to.
- [Reference / Schemas / Profile manifest](../schemas/profile-toml.md) —
  the `[apparmor].profile` field naming the on-disk file.
- [Security / Default-on controls](../../security/default-on-controls.md) —
  AppArmor enforce as part of the security baseline.
- [How-to / Add a profile](../../how-to/add-a-profile.md) — authoring a
  new AppArmor profile.
- [Reference / System / Filesystem layout](filesystem-layout.md) —
  what each profile's data dir contains.
