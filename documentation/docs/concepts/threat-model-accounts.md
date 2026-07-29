# Threat Model — Accounts, Backup, and Tunnel

This is the per-subsystem threat model for the M8 "Accounts + managed
backup/restore + integrated tunnel" work. It complements the
[overview threat model](threat-model-overview.md) (adversaries, trust
boundaries, default-on controls) by zooming into the trust boundaries that
appear *only* once a device is paired to an account and talking to
`api.deputyos.com`. Read the overview first; this page assumes it.

!!! note "Retrospective, not prescriptive"
    Roadmap lane E required this document to be written *before any account
    code landed*. In practice the accounts API, the backup/tunnel clients,
    and the wizard/PWA account surfaces were built first and are landing in
    the same batch as this page. This threat model is therefore
    **retrospective**: it describes the system as built, names the residual
    risks the build left open, and records the process deviation honestly.
    Going forward, lane E's "threat model before code" rule is enforced on
    M9+ lanes; the in-flight M8 code is grandfathered here, not excused.

## Scope

The assets, flows, and adversaries introduced by:

- **Accounts** — magic-link (web) and device-code (CLI/wizard) auth, JWT
  sessions, device registration with per-device capability tokens.
- **Managed backup** — `deputyctl backup --to cloud` / `restore --from cloud`,
  age-encrypted bundles stored in the object lake via `api.deputyos.com`.
- **Integrated tunnel** — `deputyctl tunnel --integrated`, a WebSocket relay
  through `api.deputyos.com` that exposes the device's local wizard/PWA
  to the internet under a device-scoped token.

Everything in the [overview threat model](threat-model-overview.md) still
applies — AppArmor, ufw, the `agent` user, `secrets.env` mode 0600, signed
manifests, cost guardrails. This page does not repeat those; it covers what
is *new*.

## Trust boundaries

| Boundary | Holds | Does **not** hold | Enforced by |
|---|---|---|---|
| **Client device (encryption boundary)** | Plaintext profile state, the stable backup recovery key, and the backup/tunnel API tokens (all sensitive files mode 0600). | The server's JWT signing key and other devices' credentials. | POSIX 0600, AppArmor, the `agent` user. The device is where `age` encryption/decryption runs. |
| **API server** (`api.deputyos.com`) | Token **hashes** (SHA-256) for backup/tunnel/refresh/magic-link/device-code tokens; the JWT RSA **private** key (signs only); account email; backup snapshot metadata (size, sha256, object key). | Plaintext backups (never), any token plaintext, the age key/identity. The server cannot decrypt a backup even if it wants to. | `api/src/backup.rs` upload/download store and return opaque bytes; `api/src/auth.rs` and `accounts.rs` hash every token before it touches the DB. |
| **Object lake** (B2 / R2, local-FS fallback) | Opaque `.age` bundles under `backups/{account_id}/{snapshot_id}.age`; device metadata JSON; audit warehouse events. | Plaintext backups, token plaintext. | Per-account key namespace; the API resolves `account_id` from the authenticated token's hash, so a token only ever addresses its own account's objects. |
| **Web app** (`www.deputyos.com`) | JWT access/refresh tokens in `sessionStorage` for the duration of a signed-in browser session; the device-code confirmation UI. | Long-lived device tokens (those go to the client device, not the browser), backups, the age key/identity. | Browser sessionStorage scope; tokens clear on sign-out. CORS limits credentialed calls to the configured origin (`API_CORS_ORIGIN`). |
| **Email (SES)** | The 12-char magic-link token, in transit to the account email. | Anything else. | SES with a verified domain (see open items). In dev SES is a no-op; the token is read from API logs instead. |

The defining property: **the client is the only place plaintext backups and
the recovery secret coexist.** The server and object lake hold ciphertext,
catalogs and credential hashes; compromise of either does not yield plaintext
backups without the separately exported recovery key.

## Assets

- **Account email** — PII, stored in `accounts.email`. Visible to the API
  and DB; never written to the object lake.
- **Device capability tokens** — `tunnel_token` and `backup_token`, 64-byte
  random values minted at device registration (`accounts.rs` `register_device`).
  Plaintext lives only on the client at `/etc/deputyos/{tunnel,backup}-token`
  (0600). The server stores `tunnel_token_hash` / `backup_token_hash`.
- **Backup ciphertext** — `.age` bundles in the object lake, encrypted on the
  client with an age recipient derived from the 256-bit recovery secret
  (`deputyctl/src/backup.rs` `encrypt_with_age` +
  `deputyctl/src/recovery_key.rs`). The secret is independent of the revocable
  `backup_token`, is stored at `/etc/deputyos/backup-recovery-key` (0600), and
  must be exported by the user. Token rotation therefore changes authorization
  without changing decryptability. age's passphrase mode is not used because
  it prompts on `/dev/tty` and cannot run unattended. The schema-v3 bundle
  includes profile data, hooks, secrets and channel/session databases.
- **Tunnel traffic** — HTTP the device serves locally (wizard, PWA),
  relayed over a WebSocket through the API to an internet caller. The server
  is a frame relay; with TLS terminated at the server, it can observe relayed
  frames by design.
- **Audit events** — emitted by the device over an authenticated channel into
  the audit warehouse (object lake). See [audit/compliance](../operations/monitoring-and-logs.md).

## Adversaries (additions to the overview)

The [overview](threat-model-overview.md#adversaries-we-model) already covers
opportunistic LAN/internet attackers, supply-chain, malicious skills, and
compromised provider credentials. Accounts add:

- **Compromised API server or object lake.** The headline case this subsystem
  is designed against. Server + lake together hold only ciphertext, catalogs
  and token hashes; without the recovery key they cannot decrypt backups.
- **Token theft on the client.** Reading `/etc/deputyos/backup-token` grants
  API upload/download access but not decryption. Reading
  `/etc/deputyos/backup-recovery-key` grants decryption. Full device compromise
  can obtain both (expected — the device is the encryption boundary).
- **Phishing of the magic-link token or device-code `user_code`.** A
  convincing fake site that relays these can complete auth on behalf of a
  tricked user — the same residual class as any OTP/magic-link system.
  Bounded by 15-min TTLs and single-use semantics; not eliminated.
- **Replay of a captured backup upload.** The `.age` body is ciphertext; the
  bearer is the backup token, which is long-lived until rotated or the device
  is revoked. A stolen backup token stays valid until `devices/revoke` clears
  `revoked_at` (which `auth_backup` checks). There is no automatic rotation.

## Per-threat mitigations

| Threat | Mitigation | Where |
|---|---|---|
| Server/lake compromise → plaintext backups | `age` encryption on the client; the recovery key is never uploaded; server stores/returns opaque bytes only. | `deputyctl/src/backup.rs` `encrypt_with_age`; `deputyctl/src/recovery_key.rs`; `api/src/backup.rs` |
| Backup in transit | HTTPS to `api.deputyos.com`; body is already age-encrypted before it leaves the device, so TLS termination at the server still exposes only ciphertext (defense in depth). | `deputyctl/src/backup.rs` `run_cloud_backup` |
| Token disclosure via DB dump | Every long-lived token is SHA-256 hashed at rest (`backup_token_hash`, `tunnel_token_hash`, `refresh_tokens.token_hash`, `magic_tokens.token_hash`, `device_codes.device_code_hash`). Hashes are one-way. | `api/src/backup.rs` `auth_backup`; `api/src/auth.rs` `hash_token`; `api/src/accounts.rs` `register_device` |
| Session forgery | Access token is RS256 JWT, 15-min TTL, stateless (signature-only verify). Refresh token is 64-byte random, **hashed** (not signed) and revocable, 30-day, rotated on each refresh. Private-key compromise can forge 15-min access tokens but cannot forge refresh tokens. | `api/src/middleware.rs` `create_access_token`/`decode_access_token`; `api/src/auth.rs` `issue_tokens`/`refresh`/`revoke` |
| Password breach / credential stuffing | No passwords exist. Magic-link + device-code only; no password column to leak. | `api/src/auth.rs` |
| Cross-account access (horizontal escalation) | Backup/tunnel/list endpoints resolve `account_id` from the authenticated token's hash match; object keys are namespaced `backups/{account_id}/`. A token only ever resolves to its own account. | `api/src/backup.rs` `auth_backup`; `api/src/accounts.rs` |
| Stolen / lost device | `POST /accounts/devices/revoke` sets `revoked_at`; `auth_backup` rejects tokens whose device is revoked (`revoked_at IS NULL`). | `api/src/backup.rs` `auth_backup`; `api/src/accounts.rs` `revoke_device` |
| Magic-link token reuse | `used_at` single-use, 15-min TTL, hashed at rest. | `api/src/auth.rs` `verify_magic_link` |
| Device-code guessing | `device_code` is 64-byte random (unguessable); `user_code` is 8 chars from a non-confusable alphabet (`ABCDEFGHJKLMNPQRSTUVWXYZ23456789`), 15-min TTL; confirmation requires an authenticated web session. | `api/src/auth.rs` `device_code`/`verify_device_code` |
| Tunnel abuse | WebSocket connect requires a valid `tunnel_token`; the relayed local services still enforce their own auth. The tunnel is a deliberately user-opened surface, not a new inbound port on the device. | `deputyctl/src/tunnel.rs` `run_integrated`; `api/src/tunnel.rs` |

## What's not in scope (carried from the overview)

[Physical access](threat-model-overview.md#whats-not-in-scope),
[kernel zero-days](threat-model-overview.md#whats-not-in-scope),
[shared-host side channels](threat-model-overview.md#whats-not-in-scope),
and [targeted nation-state adversaries](threat-model-overview.md#whats-not-in-scope)
remain out of scope. Specifically for this subsystem:

- **A device the attacker physically holds** can be reset to exfiltrate its
  `data_dir` and `/etc/deputyos/*-token` files. Full-disk encryption is a
  deferred milestone (see overview). The backup token on the device *is* the
  decryption key for that account's backups; physical device compromise is
  full backup compromise by design.
- **A phishing site the user voluntarily interacts with** is not defeated by
  the device-code/magic-link flows beyond their short TTLs and single-use
  semantics. This is the residual risk of any link/code-based auth.

## Open items (honest, tracked)

- **SES is not wired.** `api/src/email.rs` is a no-op until SES is configured
  with a verified domain and credentials. Until then, magic-link auth is
  dev-only (the 12-char token is read from API logs) and account creation is
  not usable end-to-end in production. This is the largest real-world blocker
  for account use; it is not an M8 checkbox.
- **External security audit (lane E) not procured.** This document is an
  internal artifact. It is not a substitute for an independent audit; lane E's
  audit checkbox stays unchecked until one is commissioned and completed.
- **Backup token recovery / escrow.** The backup token is the single point of
  recovery for an account's backups. Losing it (and the device it lived on)
  means backups are irrecoverable — by design, since the server cannot
  decrypt them. There is no recovery-key or escrow mechanism. Flagged as a
  product trade-off, not a vulnerability.
- **Tunnel token revocation check.** `auth_backup` checks `revoked_at IS
  NULL`; the tunnel WebSocket auth path should be verified to apply the same
  check so a revoked device's tunnel also drops. (Verification item for the
  M8 close-out, not a known gap.)
- **Audit event integrity.** Audit events are emitted over an authenticated
  channel and stored append-only in the warehouse; historical events are not
  individually signed. Tamper-evidence beyond transport auth is an open item.
- **Object-lake encryption-at-rest (EAS).** Per-prefix EAS is a backend
  deployment concern, not enforced by code. The `.age` ciphertext is
  independent of it; EAS is a second layer the operator may enable.

## Where to go next

- [Overview threat model](threat-model-overview.md) — adversaries and trust
  boundaries that apply to every image, account or not.
- [Security → Secrets storage](../security/secrets-storage.md) — the
  `/etc/deputyos/secrets.env` and token-file 0600 contract.
- [How-to → Operate → Cloud backup/restore](../how-to/backup-and-restore-cloud.md)
  — the operator runbook for `backup --to cloud` / `restore --from cloud`,
  including the age requirement and airgap pairing.
- [How-to → Operate → Set up tunnel](../how-to/set-up-tunnel.md) — the
  integrated and cloudflared tunnel setup paths.
