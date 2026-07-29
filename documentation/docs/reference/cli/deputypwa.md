# deputypwa

deputyOS always-on companion PWA.

`deputypwa` is the **always-on web companion** binary of deputyOS. On a baked
image it lives at `/usr/local/bin/deputypwa` and is run by the
`deputypwa.service` systemd unit, which starts after the wizard's apply step
completes and stays running for the device's lifetime. Unlike
[deputywizard](deputywizard.md) (one-shot, gated by a state file), deputypwa is
always-on.

The PWA serves the operator dashboard at
`http://<host>:8089/app/dashboard`, registers Web Push subscriptions, and
shells out to [deputyctl](deputyctl.md) for the underlying device data
(profile status, doctor, limits, cost, quiet-hours).

The HTTP API is documented separately at
[reference/apis/pwa-http.md](../apis/pwa-http.md). This page covers the
binary's command-line interface only.

## Synopsis

```
deputypwa [-v|-vv] <command> [options]
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
RUST_LOG=deputypwa=debug,info deputypwa serve
```

## Environment variables

| Variable | Default | Purpose |
|---|---|---|
| `DEPUTYPWA_DATA_DIR` | `/var/lib/deputyos` | Where `push-subscriptions.jsonl` and `vapid.pem` live. Override for `make pwa` and tests. |
| `DEPUTYPWA_DEPUTYCTL` | first `deputyctl` on PATH, else `target/release/deputyctl`, else `target/debug/deputyctl` | Path to the deputyctl binary the route handlers shell out to. |
| `DEPUTYPWA_DEV_STUB` | unset | When `1`, route handlers render synthetic data instead of shelling out to deputyctl. Used by `make pwa` so contributors can iterate on UI without a baked image. |

## Exit codes

- `0` — server exited cleanly (SIGTERM / SIGINT).
- `1` — bind failed, runtime error, or VAPID load failure.
- `64` — usage (clap default).

---

## Commands

### `serve`

Run the HTTP server.

#### Synopsis

```
deputypwa serve [--port <N>] [--bind <IP>] [--vapid-keys-path <path>]
```

#### Options

| Flag | Default | Purpose |
|---|---|---|
| `--port <N>` | `8089` | TCP port. **Distinct from `deputywizard`'s 8088** so both can run side by side without manual port management. |
| `--bind <IP>` | `127.0.0.1` | Address to bind to. **Production bakes use `0.0.0.0`** (LAN trust). |
| `--vapid-keys-path <path>` | `<DEPUTYPWA_DATA_DIR>/vapid.pem` | Path to the VAPID PEM keypair. |

#### Behavior

1. Initialize tracing.
2. Resolve the VAPID PEM path (flag, else `<data_dir>/vapid.pem`).
3. Load or generate the VAPID keypair:
   - **If the PEM exists**: load it. We derive the public key in raw
     uncompressed form (65 bytes, prefix `0x04`) and base64url-encode for
     the browser's `applicationServerKey`.
   - **If absent and `openssl` is on PATH**: run
     `openssl ecparam -genkey -name prime256v1 -noout`, write the PEM
     (mode `0600`), then derive the public key.
   - **If absent and `openssl` is missing**: run in **push-disabled mode**.
     Subscription endpoints still respond, but the public-key route returns
     an empty placeholder so the browser short-circuits cleanly. A
     `tracing::warn!` is logged.
4. Build `AppState` with the VAPID public key.
5. Bind a TCP listener on `<bind>:<port>`.
6. Print:
   ```
   deputypwa: listening on http://<bind>:<port>
   deputypwa: open http://<bind>:<port>/app/dashboard
   ```
7. Serve the [HTTP API](../apis/pwa-http.md) until SIGTERM/SIGINT.

#### Examples

Production (what `deputypwa.service` runs):

```
deputypwa serve --bind 0.0.0.0 --port 8089
```

Dev with stub data:

```
DEPUTYPWA_DEV_STUB=1 DEPUTYPWA_DATA_DIR=/tmp/deputypwa-dev deputypwa serve --port 8089
```

Pin to a specific keypair (e.g. for testing push delivery against a known
public key):

```
deputypwa serve --vapid-keys-path /tmp/test-vapid.pem
```

#### VAPID + Web Push contract

The PWA is the **registry** for browser push subscriptions; it does not
deliver pushes itself yet (push delivery — RFC 8030 + VAPID per RFC 8292 —
lands in a future hook handler). Today:

- **Browser →** `POST /api/push/subscribe` with the
  [`PushSubscription`](https://developer.mozilla.org/en-US/docs/Web/API/PushSubscription)
  JSON shape from `PushManager.subscribe()`.
- **Server →** appends the subscription as one JSON line to
  `<data_dir>/push-subscriptions.jsonl` (mode `0600`). Append-only, never
  read+rewritten under contention.
- **Browser ←** the `applicationServerKey` value comes from
  `GET /api/push/public-key`. Empty string in push-disabled mode.

#### Push subscription on-disk shape

```json
{
  "endpoint": "https://fcm.googleapis.com/wp/...",
  "keys": {
    "p256dh": "BNc...base64url",
    "auth": "abc...base64url"
  },
  "user_agent": "Mozilla/5.0 ..."
}
```

#### Files read / written

- Read: `<vapid-keys-path>` (existing PEM, if present), existing
  `push-subscriptions.jsonl`.
- Written:
    - `<vapid-keys-path>` (mode `0600`, only if generated).
    - `<data_dir>/push-subscriptions.jsonl` (mode `0600`, append-only).
- Shells out to: `openssl ecparam`, `openssl ec -pubout`, `openssl pkey`
  (only at startup), and `deputyctl` (per-request).

#### Exit codes

- `0` — graceful shutdown.
- `1` — bind error, runtime error.

#### Related

- [PWA HTTP API](../apis/pwa-http.md) — every route the server exposes.
- [deputyctl](deputyctl.md) — the binary the PWA shells out to for live data.
- [systemd-units](../system/systemd-units.md) — `deputypwa.service`.
- [How to rotate keys](../../how-to/rotate-keys.md) — the PWA dashboard surfaces the rotate flow.

---

## Lifecycle on a baked image

`deputypwa.service` is **always-on** post-wizard. Roughly:

```ini
[Unit]
Description=deputyOS companion PWA
After=network-online.target
Wants=network-online.target

[Service]
Type=exec
ExecStart=/usr/local/bin/deputypwa serve --bind 0.0.0.0 --port 8089
Restart=on-failure
RestartSec=5s
```

The unit is enabled by the wizard's apply step (`systemctl enable --now
deputypwa.service`) so it starts immediately after first-boot setup
completes and on every subsequent boot. `deputyctl down` does **not** stop
the PWA — only the active agent profile's gateway unit. The PWA staying
up is by design: even when the agent is paused (e.g. cost-cap tripped),
the operator can still reach the dashboard and unpause from a phone.

## Dev workflow

`make pwa` runs the PWA against synthetic data:

```
make pwa
```

which expands to:

```
DEPUTYPWA_DEV_STUB=1 \
DEPUTYPWA_DATA_DIR=/tmp/deputypwa-dev \
cargo run -p deputypwa -- serve --port 8089
```

`DEPUTYPWA_DEV_STUB=1` makes route handlers render synthetic data instead
of shelling out to deputyctl. Useful for iterating on UI without
first-booting an image.

## Push delivery contract (future)

The PWA is the **registry** today. Outbound push delivery (RFC 8030 +
VAPID per RFC 8292) lands in a future hook handler. The contract that
handler will follow:

1. Read `<data_dir>/push-subscriptions.jsonl`.
2. For each subscription, build a JWT signed with the VAPID private key
   (PEM at `<vapid-keys-path>`).
3. POST to `endpoint` with the encrypted payload.
4. Treat 410 / 404 as a stale subscription; the registry is append-only,
   so stale entries accumulate (cleanup is a separate sweeper job — also
   future).

`fire_push_notification` in
[`deputypwa/src/push.rs`](https://github.com/dipankarsr/deputyOS/blob/main/deputypwa/src/push.rs) is where this lives.

## See also

- [deputywizard](deputywizard.md) — first-boot wizard. Same Axum stack, different
  port (8088 vs 8089), different lifecycle (one-shot vs always-on).
- [deputyctl](deputyctl.md) — the management CLI; the PWA invokes it for every
  read of profile / doctor / limits / cost / quiet-hours data.
- [reference/apis/pwa-http.md](../apis/pwa-http.md) — the HTTP routes.
- [security/secrets-storage](../../security/secrets-storage.md) — the PWA never reads `secrets.env` directly; rotation goes through deputyctl.
