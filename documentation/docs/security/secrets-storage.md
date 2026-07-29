# Secrets storage

## What this guide does

Document deputyOS's **secrets storage contract** — where each kind of
secret lives, who can read it, what permissions enforce that, how
secrets are preserved across updates, and how `deputyctl factory-reset`
clears them while keeping the structure intact.

deputyOS holds three classes of secret data, kept separate by design:

1. **Provider keys** (`secrets.env`) — model API keys, Tailscale auth
   key, backup-bucket creds, optionally Cloudflare creds.
2. **Wizard session** (`__Host-deputyos-session` cookie) —
   short-lived, in-process; not persisted.
3. **Push subscriptions** (`push-subscriptions.jsonl`) — VAPID-signed
   browser subscription endpoints, owned by the PWA.

## `/etc/deputyos/secrets.env` — the provider key store

### Contract

- **Path**: `/etc/deputyos/secrets.env`.
- **Format**: KEY=VALUE shell-style, one var per line. Values may be
  quoted with `"` or `'`. Comments allowed (`#`-prefixed). `export` prefix
  tolerated. Parsed by `deputyctl::model::load_secrets`.
- **Permissions**: `0600`, owner `agent:agent`. Group `agent` is
  empty by convention (the agent user is alone in its group), so this
  is effectively owner-only.
- **Lifetime**: persisted; survives reboots; preserved across updates.

### What lives there

A typical populated file:

```sh
# /etc/deputyos/secrets.env — managed by deputyctl model set
OPENROUTER_API_KEY=sk-or-v1-...
ANTHROPIC_API_KEY=sk-ant-...
TAILSCALE_AUTHKEY=tskey-auth-...
RCLONE_PASSWORD=...           # if rclone.conf is encrypted
```

### Who can read it

- The active profile's gateway, via systemd's
  `EnvironmentFile=-/etc/deputyos/secrets.env`. The leading `-` makes
  it tolerant of "file missing" (first boot before the wizard
  finishes); systemd loads the env at unit start and exposes vars
  to the process.
- The AppArmor profile for the gateway grants `r,` on this exact
  path — see
  [Reference → System → AppArmor profiles](../reference/system/apparmor-profiles.md).
- `root`, transitively (root can read anything; root access is the
  topmost concern documented in
  [Concepts → Threat model overview](../concepts/threat-model-overview.md)).

### Atomic write contract

`deputyctl model set --provider <id> --key-from-stdin` and the PWA's
`POST /app/keys/rotate` share the write path:

1. Read existing file into memory (or empty map if absent).
2. Replace or append the `<KEY_ENV_VAR>=<value>` line.
3. Write to `/etc/deputyos/secrets.env.tmp.<pid>` with mode `0600`.
4. `rename(2)` over the real file — atomic on POSIX.
5. `chown agent:agent` (best-effort; non-root invocations can't, and
   that's logged as a debug warning, never an error).

If killed between steps 3 and 4, the original file is intact and the
tmp file is cleaned up on the next run.

### Preservation across updates

The active profile manifest declares
`[upgrade].preserve_dirs = ["~/.<id>"]`. `secrets.env` itself is NOT
in any profile's preserve_dirs (it lives at `/etc/deputyos/`, not in a
home directory) — but updates today are staged-only (M6), and even
when M6 lands, the A/B swap preserves `/etc/deputyos/` intact (only the
agent code partition swaps).

### Factory reset behaviour

`deputyctl factory-reset` truncates `secrets.env` to zero bytes while
preserving the mode bits (`0600`) and ownership (`agent:agent`). This
is intentional: the file's permissions are part of the security
contract, and re-creating it later (during the next wizard run) needs
those bits to land correctly.

## Wizard session cookie

The wizard's auth model is a single `__Host-deputyos-session` cookie:

- Set on first GET to any wizard route during a session that doesn't
  have it.
- Lifetime: 1 hour.
- Cookie attributes: `__Host-` prefix (forces `Secure`, `Path=/`, no
  `Domain`), `HttpOnly`, `SameSite=Strict`.
- Storage: in-process state in `deputywizard::state::AppState`. Not
  persisted to disk; restarting the wizard invalidates all sessions.
- Validated by `deputywizard::auth`.

After the wizard's apply step, the cookie loses its purpose — the
wizard transitions out of "first-boot" mode and the PWA takes over
visibility. The wizard process exits cleanly; future visits hit the
finished-message page.

## Push subscriptions — `/var/lib/deputyos/push-subscriptions.jsonl`

The PWA's `/app/push/subscribe` endpoint persists VAPID push
subscriptions one-per-line in JSONL format. Each entry is a browser-
generated push subscription object (`endpoint`, `keys.p256dh`,
`keys.auth`).

- **Path**: `/var/lib/deputyos/push-subscriptions.jsonl`.
- **Permissions**: `0600`, owner `agent:agent`.
- **Lifetime**: persisted; survives reboots and updates. Cleared by
  `deputyctl factory-reset`.

The VAPID **keypair** lives at `/etc/deputyos/vapid.json` (mode `0600`,
owner `agent:agent`). It's generated on first PWA start if absent;
never shipped in the image.

## What does not live in any of these

- **The release-signing key** — never on a device. Signing happens at
  release-time on the deputyOS infra side. The pubkey at
  `/etc/deputyos/pubkey.minisign` is just the verifying side.
- **rclone destination credentials** — those live in
  `/etc/deputyos/rclone.conf` (mode `0600`), not in `secrets.env`.
- **Channel-specific tokens** (Telegram bot token, Slack signing
  secret) — those live in `/etc/deputyos/<profile>/channels.d/<channel>.env`
  (mode `0600`). The wizard's channels step writes them; the upstream
  agent's startup reads them.

The separation is intentional: rotating a model API key shouldn't
touch the rclone backup config; rotating Telegram shouldn't touch
provider keys.

## Verification

```sh
# Permissions
ls -l /etc/deputyos/secrets.env /etc/deputyos/rclone.conf /etc/deputyos/vapid.json
ls -l /etc/deputyos/<profile>/channels.d/

# AppArmor allows the gateway to read, denies others
sudo aa-status | grep <profile>
sudo cat /etc/apparmor.d/deputyos.<profile> | grep secrets.env
```

## Troubleshooting

!!! warning "`secrets.env` is mode 0644 after manual edit"
    A `vim`/`nano` save without `:set ft=sh` plus a sudo umask of 0022
    can drop the file to 0644. Restore with `chmod 0600
    /etc/deputyos/secrets.env`. Use `deputyctl model set` instead — it's
    atomic and preserves mode.

!!! warning "Push subscriptions persist a stale endpoint"
    Browsers periodically rotate push subscription endpoints. The PWA
    receives a 410 from the push service on a dead endpoint and lazily
    prunes — but `push-subscriptions.jsonl` may carry stale rows
    until the next push attempt. This is fine; the file is small and
    self-healing.

!!! danger "Backing up `secrets.env`"
    Don't include `secrets.env` in routine backups. The rclone-driven
    `deputyctl backup now` only sweeps the data dir
    (`~/.<profile>/`), not `/etc/deputyos/`. If you must back up keys,
    use a password manager / Vault — not a generic backup bucket.

!!! tip "Use a secrets manager for the value, not the file"
    Keep the **canonical** value (the API key) in your password
    manager / Vault / Doppler. Drive `deputyctl model set
    --key-from-stdin <<< "$(vault kv get ...)"` from that. Then
    `secrets.env` is a derivable cache, not a master copy.

## Related

- [How-to → Rotate keys](../how-to/rotate-keys.md)
- [Reference → System → Filesystem layout](../reference/system/filesystem-layout.md)
- [Reference → System → AppArmor profiles](../reference/system/apparmor-profiles.md)
- [Security → Default-on controls](default-on-controls.md)
- [Security → Update trust chain](update-trust-chain.md)
