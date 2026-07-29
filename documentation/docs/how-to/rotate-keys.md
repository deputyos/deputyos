# Rotate API keys

## What this guide does

Rotate a model-provider API key on a running deputyOS device. deputyOS
stores all provider keys in **`/etc/deputyos/secrets.env`** (mode
`0600`, owner `agent:agent`). Two paths to rotate:

- **CLI** — `deputyctl model set --provider <id> --key-from-stdin`. The
  preferred path on a server / SSH session.
- **PWA** — the `/app/keys` page in the always-on dashboard. The
  preferred path on a device you reach over the LAN / Tailscale.

Both paths do the same atomic write underneath.

## Prerequisites

- A running deputyOS device with `secrets.env` already populated (the
  wizard initialised it during first boot).
- The new key from the provider.
- Either SSH access **or** PWA access (Tailscale, LAN with the
  gateway-allowlist set, or Cloudflare Tunnel).

## The recipe — CLI

### 1. Rotate via stdin (preferred — keeps the key out of shell history)

```sh
sudo deputyctl model set --provider openrouter --key-from-stdin --yes <<< 'sk-or-v1-...'
```

`--key-from-stdin` reads exactly one line. The flag is mutually
exclusive with the interactive prompt. `--yes` skips the
"are-you-sure" confirmation — useful in scripted rotation.

### 2. Verify with a single-token round-trip

```sh
sudo deputyctl model test --provider openrouter
```

`deputyctl model test` POSTs a 1-token request to the provider's
endpoint. Exits 0 on success (key works, model responds). Exits
non-zero with a clear "401 Unauthorized" / "DNS failure" / "invalid
model id" diagnostic on failure.

### 3. Restart the gateway so the change takes effect

```sh
sudo deputyctl restart
```

The systemd unit reads `/etc/deputyos/secrets.env` via
`EnvironmentFile=-/etc/deputyos/secrets.env`, which is loaded only at
unit start. The agent process needs a restart to pick up the new key.

## The recipe — PWA

### 1. Open `/app/keys`

```
http://deputyos.local/app/keys
```

(or the Tailscale / Cloudflare Tunnel URL.)

The page lists every configured provider with a "Rotate" button.
Authentication is the device's session cookie set during the wizard.

### 2. Paste the new key

The form posts to `POST /app/keys/rotate` with a CSRF token from the
session. The server invokes the same atomic write underneath.

### 3. PWA shows a flash message + redirects to `/app/keys`

The flash slot is single-shot per request. The new key is now in
effect for the **next** message — the agent reads `secrets.env`
on its next restart cycle (RestartSec=5).

## Atomic write contract

`deputyctl model set` and the PWA's `/app/keys/rotate` handler share the
same atomic write:

1. Read `/etc/deputyos/secrets.env` into memory.
2. Replace or append the `<KEY_ENV_VAR>=<value>` line.
3. Write to `/etc/deputyos/secrets.env.tmp.<pid>` with mode `0600`.
4. `rename(2)` the tmp file over the real one — atomic on POSIX.
5. `chown agent:agent` (best-effort; root only).

If the process is killed between steps 3 and 4, the original file is
intact. The tmp file is cleaned up on next run.

## Active provider switching

Setting a new key does **not** automatically activate that provider.
The active provider is recorded in
`/etc/deputyos/active-provider`; switch with:

```sh
sudo deputyctl model set --provider <id>          # interactive (prompts for key + activates)
sudo deputyctl model set --provider <id> --yes    # without re-prompting; activates only
```

The wizard's review step writes both files in lockstep. Manual
rotation post-wizard typically only changes the key, leaving the
active provider untouched.

## Verification

```sh
# 1. Permissions are correct.
ls -l /etc/deputyos/secrets.env
# -rw------- 1 agent agent ... /etc/deputyos/secrets.env

# 2. The key is the new one (sanity grep — careful with shell history).
sudo grep '^OPENROUTER_API_KEY=' /etc/deputyos/secrets.env | head -c 30; echo

# 3. The agent picked it up.
sudo deputyctl status
sudo deputyctl model test --provider openrouter
```

## Troubleshooting

!!! warning "`secrets.env` mode drifted"
    `deputyctl factory-reset` truncates while preserving mode (0600).
    A user-edited file may end up at 0644. Fix:
    `sudo chmod 0600 /etc/deputyos/secrets.env`. Without this, the file
    is world-readable on the local filesystem and the next AppArmor
    profile reload may flag it.

!!! warning "Old key is cached in the running process"
    Provider SDKs sometimes cache the bearer token for the duration of
    the process. `deputyctl restart` is required after rotation —
    `model test` runs in a separate `deputyctl` invocation and isn't a
    proxy for the long-lived gateway.

!!! danger "Pasting the key into a shared shell"
    Use `--key-from-stdin <<< 'sk-...'` (heredoc) or
    `read -s key && echo "$key" | deputyctl model set --key-from-stdin
    --provider ...`. **Do not** put the key in a positional argument —
    it shows up in `ps`, in `~/.bash_history`, in `journalctl` for
    every command run.

!!! tip "Rotate on a schedule"
    A simple systemd timer that runs `deputyctl model set
    --key-from-stdin <<< "$(curl ... vault ...)"` weekly is enough for
    routine rotation. Source the new key from your secret store of
    choice; the atomic write makes it safe to do unattended.

## Related

- [How-to → Add a model provider](add-a-model-provider.md)
- [Reference → CLI → deputyctl](../reference/cli/deputyctl.md) (`model` subcommand tree)
- [Reference → APIs → PWA HTTP](../reference/apis/pwa-http.md)
- [Security → Secrets storage](../security/secrets-storage.md)
