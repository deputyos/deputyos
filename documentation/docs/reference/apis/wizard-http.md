# deputywizard HTTP API

The first-boot wizard is an [axum](https://docs.rs/axum) web app served
by the `deputywizard` binary. Routes are registered in
[`deputywizard/src/routes.rs`](https://github.com/deputyos/deputyos/blob/main/deputywizard/src/routes.rs);
auth lives in `auth.rs`.

The wizard is a **state machine** rendered as nine sequential steps,
each with a GET (render the form, optionally with an error) and a POST
(validate, mutate state, redirect to the next step). State is
persisted as `/var/lib/deputyos/wizard-state.json` after every transition.

This page documents every route — method, path, auth, request shape,
response shape, side effects.

[TOC]

## Authentication

The wizard auth model is **single-use launch token → session cookie**:

1. The unit (or `make wizard`) starts the server with a hex token.
   Production: the bake role writes the token to
   `/run/deputyos/wizard.token` (mode 0600). Dev: `--no-token` accepts
   everything.
2. The first request must present the token via `?token=<hex>` query
   string or `Authorization: Bearer <hex>` header.
3. On match, the token is **consumed** (cleared from server memory)
   and a session cookie is minted.
4. Subsequent requests carry the session cookie. Sessions expire 1h
   after issue.

| Cookie attribute | Production | Dev |
|---|---|---|
| Name | `__Host-deputyos-session` | `deputyos-session` |
| `Secure` | yes | no |
| `HttpOnly` | yes | yes |
| `SameSite=Strict` | yes | yes |
| `Path=/` | yes | yes |
| `Max-Age` | 3600s | 3600s |

The `__Host-` prefix forbids `Domain=` and forces `Secure`+`Path=/` —
so the wizard drops it in dev mode where `Secure` would prevent the
cookie from being set over plain HTTP.

Constant-time token compare (`auth::constant_time_eq`) prevents timing
side-channels.

## Public routes (no auth)

| Method | Path | Response |
|---|---|---|
| GET | `/healthz` | `200 ok` (text/plain). Used by load balancers and `deputyctl status`. |
| GET | `/static/style.css` | Bundled CSS. |

## Protected routes (cookie or token gated)

All return HTML; redirects are issued as `302` with `Location:
/wizard/<step>` to drive the linear flow.

| Method | Path | Step | Side effects |
|---|---|---|---|
| GET | `/` | – | Redirects to the current step (read from `wizard-state.json`). |
| GET | `/wizard` | – | Same redirect. |
| GET | `/wizard/system` | 1 — System | Renders hostname + timezone form. |
| POST | `/wizard/system` | 1 | Validates, persists, redirects to `/wizard/profile`. |
| GET | `/wizard/profile` | 2 — Profile | Renders profile picker (lists installed `profiles/<id>.toml`). |
| POST | `/wizard/profile` | 2 | Validates, persists, redirects to `/wizard/provider`. |
| GET | `/wizard/provider` | 3 — Provider | Renders provider picker from `providers.json`. |
| POST | `/wizard/provider` | 3 | Validates key (round-trip optional via `Skip validation`), buffers in memory, redirects. |
| GET | `/wizard/channels` | 4 — Channels | Renders intersection of profile.channels.supported × limits.channels_disabled_by_ram. |
| POST | `/wizard/channels` | 4 | Validates, persists, redirects to `/wizard/ssh`. |
| GET | `/wizard/ssh` | 5 — SSH keys | Renders SSH keys textarea. |
| POST | `/wizard/ssh` | 5 | Validates each key prefix, persists, redirects to `/wizard/tailscale`. |
| GET | `/wizard/tailscale` | 6 — Tailscale | Renders Tailscale auth-key form (skippable). |
| POST | `/wizard/tailscale` | 6 | Buffers key in memory, persists "enabled" flag, redirects to `/wizard/cloudflare-tunnel`. |
| GET | `/wizard/cloudflare-tunnel` | 7 — Cloudflare Tunnel | Renders three-way choice (skip / quick / named). |
| POST | `/wizard/cloudflare-tunnel` | 7 | If named, parses + buffers credentials JSON, persists tunnel name, redirects to `/wizard/backup`. |
| GET | `/wizard/backup` | 8 — Backup | Renders four-way choice (skip / b2 / r2 / s3). |
| POST | `/wizard/backup` | 8 | Validates per-kind required fields, buffers in memory, redirects to `/wizard/review`. |
| GET | `/wizard/review` | 9 — Review | Renders summary of all answers. |
| POST | `/wizard/review/apply` | 9 | Calls `apply::apply` (writes secrets.env, active-profile, active-provider, ufw rules, systemd ops…). On success redirects to `/wizard/done`. |
| GET | `/wizard/done` | – | Final page. |
| GET | `/chat` | – | Built-in private web chat surface. Renders chat history. |
| POST | `/chat/message` | – | Forwards user message to the active profile's gateway over loopback. |

## Per-route detail

### Step 1 — `POST /wizard/system`

Form body:

```
hostname=<dns-label>&timezone=<IANA-tz>
```

Validation:

- `hostname`: required; ≤63 chars; only `[a-z0-9-]`; no leading/trailing
  hyphen.
- `timezone`: non-empty; no whitespace.

Response: `302 Location: /wizard/profile` on success; otherwise re-renders
step 1 with a `<div class="error">` block and HTTP 200.

State written: `wizard-state.json` with `answers.hostname`,
`answers.timezone`, `step="profile"`.

### Step 2 — `POST /wizard/profile`

Form body: `profile=<id>` (one of the installed profile ids).

Validation: `id` must be in the loaded `profiles/<id>.toml` set.

State written: `answers.profile`, `step="provider"`.

### Step 3 — `POST /wizard/provider`

Form body:

```
provider=<id>&api_key=<value>[&skip_validation=1]
```

Validation:

- Provider id must be in `providers.json`.
- `api_key` non-empty.
- Unless `skip_validation=1`, runs `provider_check::check` on a
  blocking thread (fork-safe with the tokio runtime). The check is
  `kind`-dispatched — see [providers.json reference](../schemas/providers-json.md#kind-enum-values).
- If the check fails, re-renders step 3 with the HTTP error or
  network error message and a hint to retry or tick "Skip validation".

State written: `answers.provider`, `step="channels"`. The api_key is
buffered in `pending_secret` (in-memory, never written to
`wizard-state.json`).

### Step 4 — `POST /wizard/channels`

Form body: repeated `channels=<id>&channels=<id>&…`.

The handler hand-parses the form (Axum's default `Form` collapses
duplicates) into a `Vec<String>`.

Validation:

- Each `id` must be in the active profile's `[channels].supported`.
- No `id` may be in the device's
  `limits.capabilities.channels_disabled_by_ram`. If any are, returns
  `422 Unprocessable Entity` with the step re-rendered.

State written: `answers.channels`, `step="ssh"`.

### Step 5 — `POST /wizard/ssh`

Form body: `ssh_keys=<one-key-per-line>`.

Validation: each line must begin with one of:

- `ssh-rsa `, `ssh-ed25519 `, `ssh-ecdsa `
- `ecdsa-sha2-nistp{256,384,521} `
- `sk-ssh-ed25519@openssh.com `
- `sk-ecdsa-sha2-nistp256@openssh.com `

…and contain at least two whitespace-separated tokens (algorithm + key).

State written: `answers.ssh_keys` (Vec<String>), `step="tailscale"`.

### Step 6 — `POST /wizard/tailscale`

Form body: `authkey=<tskey-...>` or `skip=1`.

If `skip` is set or `authkey` is empty: enabled=false, no key buffered.

Validation: when enabling, `authkey.len() >= 8` (a tiny sanity check —
Tailscale key formats change, so we don't pin a regex).

State written: `answers.tailscale_enabled`, `step="cloudflare-tunnel"`.
Key buffered in `pending_tailscale`.

### Step 7 — `POST /wizard/cloudflare-tunnel`

Form body: `choice=<skip|quick|named>` and (for named)
`credentials=<json>`.

For `named`, parses the credentials JSON (the file Cloudflare's
`cloudflared` produces); requires `TunnelName` or `TunnelID`.

State written: `answers.cloudflare_tunnel_choice`,
`answers.cloudflare_tunnel_name`, `step="backup"`. Credentials buffered
in `pending_cloudflared`.

### Step 8 — `POST /wizard/backup`

Form body shape varies by `kind`:

| kind | Required fields |
|---|---|
| `skip` | – |
| `b2` | `b2_account_id`, `b2_application_key`, `b2_bucket` |
| `r2` | `r2_account_id`, `r2_access_key`, `r2_secret_key`, `r2_bucket` |
| `s3` | `s3_endpoint`, `s3_access_key`, `s3_secret_key`, `s3_bucket` |

For `r2`, the handler synthesises an `endpoint` of
`https://<account_id>.r2.cloudflarestorage.com`.

State written: `answers.backup_kind`, `answers.backup_meta` (only
non-secret fields), `step="review"`. Credentials buffered in
`pending_backup`.

### Step 9 — `POST /wizard/review/apply`

No form body — POST is a confirmation. The handler:

1. Drains the four `pending_*` mutexes (provider key, tailscale key,
   cloudflared credentials, backup credentials).
2. Looks up the active manifest by id.
3. Builds an `apply::ApplyExtras` struct.
4. Calls `apply::apply(mode, dev_out, &answers, provider_secret,
   &ports, unit, &extras)`. This writes:
   - `/etc/deputyos/active-profile` (one-line profile id)
   - `/etc/deputyos/active-provider` (one-line provider id)
   - `/etc/deputyos/secrets.env` (mode 0640, owner root:agent)
   - `/etc/deputyos/rclone.conf` (when backup kind != skip)
   - SSH `~/.ssh/authorized_keys` for the agent user
   - Tailscale `tailscale up --authkey=...` (production) / dev-out
     marker (dev)
   - Cloudflare Tunnel `cloudflared service install` (production) /
     dev-out marker (dev)
   - ufw rules for the chosen channel ports
   - `systemctl enable --now <profile>-gateway.service`
5. On success: persists `step="done"` and `completed_at` (RFC 3339);
   redirects to `/wizard/done`.
6. On failure: re-renders the review page with the error.

The wizard's `apply_mode` is `Production` for the systemd unit;
`Dev` when run via `make wizard` (writes to `dev-out/` instead of
`/etc/`, etc.).

### `/chat` — built-in web chat

| Method | Path | Description |
|---|---|---|
| GET | `/chat` | Renders chat history from
[`chat_history_path`](https://github.com/deputyos/deputyos/blob/main/deputywizard/src/routes.rs#L957)
(env-overridable via `DEPUTYWIZARD_CHAT_HISTORY`; falls back to
`<data_dir>/chat-history.jsonl` for the active profile). |
| POST | `/chat/message` | Forwards the user's message to the agent's gateway over loopback (URL derived from the active manifest's `[health].http_check`), records both turns in the JSONL history, returns the rendered messages partial. |

Agent address resolution: [`agent_base`](https://github.com/deputyos/deputyos/blob/main/deputywizard/src/routes.rs#L989).

## Side-effect map

| Operation | Files written/read | Systemd touched |
|---|---|---|
| Each step's POST | `wizard-state.json` write | – |
| Step 3 (provider) | `pending_secret` in memory | – |
| Apply | `secrets.env`, `active-profile`, `active-provider`, `rclone.conf`, `~/.ssh/authorized_keys`, ufw rules, optionally `cloudflared` config | `systemctl enable --now <unit>` (production); `systemctl restart` to pick up env vars |
| `/chat/message` | `<data_dir>/chat-history.jsonl` append | – |

## See also

- [Reference / CLI / deputywizard](../cli/deputywizard.md) — the
  `serve`, `print-qr` subcommands, command-line flags, env vars.
- [Reference / Schemas / Profile manifest](../schemas/profile-toml.md) —
  step 4 reads `[channels].supported`.
- [Reference / Schemas / Providers](../schemas/providers-json.md) —
  step 3 renders this catalogue.
- [Reference / Schemas / Limits](../schemas/limits-json.md) — step 4
  intersects `channels_disabled_by_ram`.
- [Reference / System / systemd units](../system/systemd-units.md#deputywizardservice) —
  the unit gating + hardening.
- [Reference / System / Filesystem layout](../system/filesystem-layout.md) —
  every path the apply step touches.
- [How-to / Add a wizard step](../../how-to/add-a-profile.md) — adjacent
  to the profile-add recipe.
- [Security / Default-on controls](../../security/default-on-controls.md) —
  the auth model.
