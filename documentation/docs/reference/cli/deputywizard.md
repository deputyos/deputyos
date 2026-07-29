# deputywizard

deputyOS first-boot web wizard.

`deputywizard` is the **first-boot setup** binary of deputyOS. On a baked
image it lives at `/usr/local/bin/deputywizard` and is run by the
`deputywizard.service` systemd unit on first boot only — gated by
`ConditionPathExists=!/var/lib/deputyos/wizard-state.json`. After the
wizard's "apply" step writes `wizard-state.json`, the unit refuses to
start again on subsequent boots, leaving the path empty for the user to
re-trigger via [`deputyctl init`](deputyctl.md#init).

The companion subcommand `print-qr` is invoked by the
`deputyos-qr-on-tty.service` unit at boot to render the launch URL as a
QR code on `/dev/tty1` so the operator can scan it from a phone.

The HTTP API the wizard exposes is documented separately at
[reference/apis/wizard-http.md](../apis/wizard-http.md). This page covers
the binary's command-line interface only.

## Synopsis

```
deputywizard [-v|-vv] <command> [options]
```

## Global options

| Flag | Effect |
|---|---|
| `-v, --verbose` | `-v` = `debug`, `-vv` = `trace`. Counted; default `info`. Loses to `RUST_LOG`. |
| `-h, --help` | Print help. |
| `-V, --version` | Print version. |

## Logging

`tracing-subscriber` writes to **stderr**. Set `RUST_LOG` to override:

```
RUST_LOG=deputywizard=debug,deputyctl=debug,info deputywizard serve
```

## Environment variables

| Variable | Default | Purpose |
|---|---|---|
| `DEPUTYWIZARD_TOKEN_FILE` | `/run/deputyos/wizard.token` | Where `serve` writes the auth token (mode `0600`) so `print-qr` and operator-side helpers can read it. |
| `DEPUTYOS_*` (full deputyctl set) | — | `deputywizard` reuses [deputyctl's](deputyctl.md#environment-variables) `paths.rs`. Notably `DEPUTYOS_PROFILES_DIR`, `DEPUTYOS_PROVIDERS_FILE`, `DEPUTYOS_LIMITS_FILE`, `DEPUTYOS_SECRETS_FILE`, `DEPUTYOS_ACTIVE_PROVIDER_FILE`. |

## Exit codes

- `0` — server exited cleanly (SIGTERM / SIGINT).
- `1` — bind failed, runtime error, or unrecoverable wizard state corruption.
- `64` — usage (clap default for argument parsing failures).

## Apply mode

`serve` operates in one of two modes:

- **`production`** — write to real system paths (`/etc/deputyos/...`),
  shell out to `hostnamectl`, `timedatectl`, `systemctl`. Cookies set
  with `Secure`. This is what runs on a baked image.
- **`dev`** — auto-detected when running outside a baked image. Writes
  routed under a tempdir or `DEPUTYOS_DEV_OUT` if set. No `systemctl`,
  no `hostnamectl`. Cookies sent without `Secure` so http://localhost
  works.

Pass `--production` to force production mode regardless of detection
(used by integration tests on a Linux runner).

---

## Commands

### `serve`

Run the HTTP server.

#### Synopsis

```
deputywizard serve [--port <N>] [--bind <IP>] [--token <hex>] [--no-token] [--state-file <path>] [--production]
```

#### Options

| Flag | Default | Purpose |
|---|---|---|
| `--port <N>` | `8088` | TCP port. |
| `--bind <IP>` | `127.0.0.1` | Address to bind to. **Production bakes use `0.0.0.0`** — the default of `127.0.0.1` is for dev safety. |
| `--token <hex>` | random 32-byte hex | Pre-shared single-use token. If unset, a random one is generated and written to `DEPUTYWIZARD_TOKEN_FILE`. |
| `--no-token` | off | Disable the auth gate entirely. Used by `make wizard` and tests. |
| `--state-file <path>` | `/var/lib/deputyos/wizard-state.json` | Wizard state JSON path. |
| `--production` | off | Force production-mode apply (write `/etc/`, run `hostnamectl`/`timedatectl`/`systemctl`). |

#### Behavior

1. Initialize tracing.
2. Build the runtime state:
   - Load (or create) `wizard-state.json`.
   - Load the providers catalogue from `DEPUTYOS_PROVIDERS_FILE`.
   - Enumerate installed profiles from `DEPUTYOS_PROFILES_DIR`. Profiles
     that fail to parse are skipped with a `warn` (vs. deputyctl's hard
     error — the wizard tolerates partial state because it might be the
     thing fixing it).
   - Load `DEPUTYOS_LIMITS_FILE`. Missing → sidebar shows "limits unknown".
3. Compute the auth mode:
   - `--no-token` → `AuthMode::None`.
   - else → `AuthMode::Token`. Token is `--token` if passed, else 32
     random hex chars from a CSPRNG. The token is printed once on stderr
     and written to `DEPUTYWIZARD_TOKEN_FILE` (mode `0600`, parent dir
     created best-effort). Best-effort: if the directory cannot be
     created, the token file is silently skipped (you still see the
     token on stderr).
4. Compute the apply mode (`--production` wins, else auto-detect).
5. Bind a TCP listener on `<bind>:<port>`.
6. Print:
   ```
   deputywizard: auth token = <hex>
   deputywizard: listening on http://<bind>:<port>
   deputywizard: open http://<bind>:<port>/wizard?token=<hex>
   deputywizard: apply_mode=production (state file: /var/lib/deputyos/wizard-state.json)
   ```
7. Serve the [HTTP API](../apis/wizard-http.md) until SIGTERM/SIGINT,
   then graceful shutdown.

#### Examples

Production (what `deputywizard.service` runs):

```
deputywizard serve --bind 0.0.0.0 --port 8088 --production
```

Dev (what `make wizard` runs):

```
deputywizard serve --port 8088 --no-token
```

Bind to a token you control (CI smoke tests):

```
deputywizard serve --port 8088 --token deadbeefcafe...32chars --bind 127.0.0.1
```

#### Files read / written

- Read: `DEPUTYOS_PROFILES_DIR/*.toml`, `DEPUTYOS_PROVIDERS_FILE`, `DEPUTYOS_LIMITS_FILE`,
  `DEPUTYOS_ACTIVE_PROVIDER_FILE`, existing `DEPUTYWIZARD_TOKEN_FILE`.
- Written: `DEPUTYWIZARD_TOKEN_FILE` (mode `0600`), `--state-file` (incrementally as
  the user clicks through), and on "apply": `DEPUTYOS_SECRETS_FILE`,
  `DEPUTYOS_ACTIVE_PROVIDER_FILE`, `DEPUTYOS_ACTIVE_PROFILE_FILE`,
  `DEPUTYOS_BACKUP_CONFIG`, `DEPUTYOS_RCLONE_CONFIG`, plus shells out to
  `hostnamectl`, `timedatectl`, `systemctl` (production mode only).

#### Exit codes

- `0` — graceful shutdown.
- `1` — bind error, runtime error.

#### Related

- [Wizard HTTP API](../apis/wizard-http.md)
- [`deputyctl init`](deputyctl.md#init)
- [systemd-units reference](../system/systemd-units.md)

---

### `print-qr`

Print an ASCII QR code for the wizard launch URL.

#### Synopsis

```
deputywizard print-qr [--url <URL>] [--token <hex>] [--host <name>] [--port <N>] [--url-file <path>]
```

#### Options

| Flag | Default | Purpose |
|---|---|---|
| `--url <URL>` | computed | Override the URL to encode. Bypasses host/token resolution. |
| `--token <hex>` | content of `/run/deputyos/wizard.token` | Token to embed. |
| `--host <name>` | `deputyos.local` | Hostname to embed. |
| `--port <N>` | `8088` | Port to embed. |
| `--url-file <path>` | `/run/deputyos/wizard.url` | Also write the URL (plaintext) to this path. |

#### URL resolution

If `--url` is set, it is used verbatim. Otherwise:

```
http://<host>:<port>/wizard?token=<token>
```

If the resolved token (from `--token` or `/run/deputyos/wizard.token`) is
empty, the URL drops the `?token=` query parameter.

#### Behavior

1. Resolve the URL.
2. Render an ASCII QR via [`qrcodegen`](https://docs.rs/qrcodegen) at error
   correction level **Medium** with a 4-module quiet zone.
3. Render with `▀ ▄ █  ` half-block characters (two QR rows per terminal
   row, so the QR is roughly square at typical terminal cell aspect ratios).
4. Print the QR to stdout, followed by the URL line: `URL: <final-url>`.
5. Best-effort write of the URL plaintext to `--url-file`.

#### Boot-time usage

The `deputyos-qr-on-tty.service` unit pipes `deputywizard print-qr` to
`/dev/tty1`:

```
ExecStart=/bin/sh -c '/usr/local/bin/deputywizard print-qr > /dev/tty1'
```

So when the operator brings up a baked image with a screen attached
(monitor, HDMI, serial console), they see the QR + URL on TTY1 the moment
networking is up — no second screen needed.

#### Examples

```
$ deputywizard print-qr --host pi.local --port 8088
█████████████████████████████████
█▀▀▀▀▀▀▀█▀▀▀█  ▀ ▄▀█▀▀▀▀▀▀▀█
...
URL: http://pi.local:8088/wizard?token=a1b2c3d4...
```

```
$ deputywizard print-qr --url https://random.trycloudflare.com
... (QR for the tunnel URL) ...
URL: https://random.trycloudflare.com
```

#### Files read / written

- Read: `/run/deputyos/wizard.token` (or `--token`).
- Written: `--url-file` (default `/run/deputyos/wizard.url`). Best-effort —
  failure is logged at `debug` and ignored.

#### Exit codes

- `0` — QR rendered (and URL file written, or write skipped silently).
- `1` (anyhow) — QR data too large (extremely rare; we don't hit it for
  `deputyos.local` URLs).

#### Related

- [`deputywizard serve`](#serve)
- [systemd-units](../system/systemd-units.md)

---

## Lifecycle on a baked image

The wizard is intentionally one-shot. The systemd unit looks roughly like:

```ini
[Unit]
Description=deputyOS first-boot wizard
After=network-online.target
ConditionPathExists=!/var/lib/deputyos/wizard-state.json

[Service]
Type=exec
ExecStart=/usr/local/bin/deputywizard serve --bind 0.0.0.0 --port 8088 --production
Restart=on-failure
```

Once the user clicks "Apply" in the wizard UI, the routes layer writes
`/var/lib/deputyos/wizard-state.json` with `step: "complete"`. On the next
boot, `ConditionPathExists=!/var/lib/deputyos/wizard-state.json` evaluates
false and systemd skips the unit. To re-run:

- **Manual one-shot**: `deputyctl init` (spawns the binary directly without
  systemd).
- **Force the unit**: `sudo rm /var/lib/deputyos/wizard-state.json && sudo
  systemctl start deputywizard.service`.

The companion `deputyos-qr-on-tty.service` runs at every boot regardless;
it's gated only on the wizard URL file existing under `/run/deputyos/`. If
the wizard isn't running, the QR points at a stale URL — by design, the
boot operator can scan it and discover the wizard is already done.

## Token persistence

The auth token is **process-local** by default. Restarting `deputywizard`
generates a new random token. To pin a token across restarts (so an
operator can keep a phone bookmark valid):

```
deputywizard serve --token "$(cat /etc/deputyos/wizard-token)"
```

In a baked image the systemd unit instead generates a fresh token on every
boot and writes it to `/run/deputyos/wizard.token`. The QR-on-TTY unit
reads the same path at boot, so the displayed QR always matches the
running token.

## Apply mode detection

Auto-detection (when `--production` is **not** passed) is implemented in
`apply::ApplyMode::detect()`:

- If `/etc/deputyos/` is writable as the running user → production mode.
- Else → dev mode.

Production mode side effects (during the apply step):

- Writes to `/etc/deputyos/secrets.env`, `/etc/deputyos/active-provider`,
  `/etc/deputyos/active-profile`, `/etc/deputyos/backup.toml`,
  `/etc/deputyos/rclone.conf`.
- Shells out to `hostnamectl set-hostname <name>`,
  `timedatectl set-timezone <tz>`,
  `systemctl enable --now <unit>` for the chosen profile + tunnel +
  backup timer.

Dev mode short-circuits all of the above. The wizard "apply" still
completes (state file written) but no system changes happen.

## See also

- [deputyctl](deputyctl.md) — the management CLI; `deputyctl init` shells out to `deputywizard serve`.
- [Wizard HTTP API](../apis/wizard-http.md) — every route the server exposes.
- [systemd-units](../system/systemd-units.md) — `deputywizard.service`, `deputyos-qr-on-tty.service`.
- [How to add a profile](../../how-to/add-a-profile.md) — the wizard surfaces installed profiles for selection.
- [How to add a model provider](../../how-to/add-a-model-provider.md) — the wizard surfaces every entry in `providers.json`.
- [security/secrets-storage](../../security/secrets-storage.md) — what the wizard writes when the user enters a key.
