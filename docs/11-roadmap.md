# 11 — Roadmap

The work splits into seven parallel swimlanes that run concurrently across phases. Each phase has explicit per-lane checklists and an exit gate that all lanes must satisfy before the next phase begins. Effort estimates assume 2–3 contributors fanning out across lanes; a solo build sequences lanes within each phase.

## Swimlanes

| Lane | Owner role | Scope |
|---|---|---|
| **A — Manager binary (`deputyctl`)** | Rust dev | The CLI/web UI binary, wizard flows, profile orchestration, doctor, update client. |
| **B — Image build pipeline** | Build/SRE | pi-gen + Packer ARM + Packer DO, the shared Ansible role with hardware variant gates, QEMU smoke harness, hermetic builds. |
| **C — Wizard & first-boot UX** | Frontend / UX | Axum + HTMX web wizard, mDNS `deputyos.local`, QR provisioning, built-in `/chat`, voice on-device, cost-guardrail UI. |
| **D — Release & update infra** | Build/SRE | GH Actions release-tracker bot, B2 + Cloudflare hosting, minisign + cosign signatures, signed `manifest.json`, A/B image swap, watchdog rollback. |
| **E — Security & hardening** | Security | AppArmor profiles, ufw/fail2ban, sysctl tuning, ClamAV integration, Magika integration, secrets storage, threat model, external audit. |
| **F — Profiles & content** | Integrations | Profile manifest schema + concrete profiles (OpenClaw, Hermes, third), channel templates, skill/template registry mirror. |
| **G — Site / docs / community** | Tech writer / DevRel | docs site, picker page, threat model & privacy policy publishing, contribution guide, profile marketplace, showcase gallery. |

## Timeline at a glance

```
                 M0   M1     M2  M2.5  M3  M3.5  M4a M4b M4.5 M5  M5.5 M6   M7  M8  M9
A deputyctl        .    █████  ████  .   ████ ███   ██  ███ ███  ███  ██   ████ ███ ████
B image build     .    ████   ██████ .  ██   ██    .   ███ ████ ██   .    ████ ██  .
C wizard/UX       .    ..     ███   ██  █████ ███  ██  ..  ██   ███  ███  ████ █   ██
D release/update  .    ██     ███   .   .    .     ████ ████ ██  .   .    █████ ███ ████
E security        ██   ████   ████  .   ███  ██    ██  ..  ███  ███  ████ ████ █████ ████
F profiles        ██   ███    ███   .   ██   .     .   .   ██   .    .    .    ████ .
G site/docs       █████ ██    ████  ██  ██   ██    ████ ██  ██   ███ ███  ███  ██  ███
                  ──────────────────────────────────────────────────────────────────────
                   docs skeleton matrix launch wizard mounts websites CDN airgap mkt egress voice plugin accts console
```

## Status as of 2026-06-23

- **Tests**: 337 across 6 workspace crates (deputyctl 162, deputywizard 52, deputypwa 33, deputyos-track 17, deputyos-desktop 55, deputyos-console 18).
- **Hardware targets wired**: 14 (qemu-aarch64, qemu-x86_64, rpi5, rpi4, arm64-generic,
  x86_64-mini-pc, digitalocean, oracle-arm-free, hetzner-cloud, vultr, linode,
  fly-machines, wsl2, macos-qemu) + 3 community templates (proxmox, unraid, truenas).
- **Profiles**: OpenClaw, Hermes, Khoj. M7 plugin-model acceptance test passed
  (Khoj added with zero Rust changes — manifest + Ansible bake recipe + AppArmor
  + systemd unit + stub fallback only).
- **`make ci`**: green; `make matrix` builds qemu-aarch64 + qemu-x86_64 with smoke
  gates; `make manifest` + `make publish-local` + `make verify VERSION=` work
  end-to-end against a local file:// CDN.

What still needs work (split by where blocking is):
- **In-flight on this plan, sandbox-doable**:
  - M2.5 desktop launcher (existing, ongoing)
  - **M3.5 drive mounting** — **DONE** (host-FS, removable, SMB/NFS; wizard Drives step + SMB/NFS form, PWA mounts card, profile `[mounts]` section, policy schema-v1, boot-enabled materialiser; www marketing page still open)
  - **M4a public sites + GitHub release** (new — Astro `www`, Vue `api`, `deputyos.com` swap done in code)
  - **M4.5 air-gapped fat tier** — **DONE** in-sandbox (`AIRGAP=1` knob → Packer → airgap-baseline + llm-airgap roles; authoritative Hugging Face LFS SHA-256 pins; `deputyctl model register`/`network {mode,allow,unlock,status}`; wizard Local-LLM default + no-key path; PWA Airgap badge; `update --from` sneakernet; network-policy-v1 schema + reference; threat-model delta; profile `[airgap] default_provider`; airgap-builds doc + nav. Open: apt-mirror population; live `-net none` boot smoke; `www.deputyos.com/picker/` toggle is the sibling repo.)
- **Sandbox-doable, parked**:
  - (none currently — M5.5 landed; see below)
- **Landed this cycle (M8)**: accounts + managed backup/restore + integrated tunnel — substantially built across `api-deputyos-com` / `www-deputyos-com` / `deputyOS`. Device-code auth, integrated WebSocket tunnel, age-encrypted cloud backup/restore, PWA tunnel/account cards, and the accounts threat model all landed. Managed schema-v3 backups now use an independent exportable recovery key, quiesce the resident workload, verify locally before upload, and store catalogs/lifecycle records only in object storage. API tokens remain revocable credentials and are no longer encryption keys. The hermetic E2E round-trip test (`restore::tests::age_round_trip_backup_to_restore_is_byte_identical`) confirms backup→encrypt→decrypt→extract→place reproduces the source device byte-for-byte. Open follow-ups: external audit (Lane E, procurement), SES email delivery, `deputyos.com` domain + R2/B2 + TLS, the live two-device cloud round-trip (`[partial:bucket]`).
- **Landed this cycle (M9 v1)**: desktop console — login + local multi-instance + fleet + remote management. New `deputyos-console` crate (Tauri v2 + system webview): device-code login, OS-keychain token store, and a named-instance lifecycle over `deputyos-desktop`'s new `Driver::*_with(&InstanceConfig)` (M9.1 — multiple local agents with distinct ports/cache dirs coexist on one host; existing single-instance CLI unchanged). The GUI is gated behind a `gui` feature so the testable core builds without webview deps (`make console` builds the GUI; needs `libwebkit2gtk-4.1-dev libgtk-3-dev librsvg2-dev` on Linux). Sibling `api-deputyos-com/api/src/tunnel.rs` now keys the live WebSocket channel + the proxy path by `device_id` (was `account_id`) so one account runs many devices, and the proxy requires a valid account JWT whose `sub` owns the device (non-owner 403, no credential 401, unknown device 404) — the `?token=` form the console uses is accepted. `deputywizard` gains `AuthMode::AccountOwner` (RS256 JWT validated against the API pubkey embedded at `/etc/deputyos/api-pubkey.pem`, `sub` matched to `account.json`): the load-bearing fix for remote wizard access, since the in-VM launch token never leaves the appliance. `account.json` now stores `account_id`. Local drivers exist for Linux/qemu, Windows/WSL2, and macOS/UTM; remote management is pure HTTP/WS and works everywhere. Open follow-ups: none remain in M9 (M9.4/M9.5/M9.6 all landed); the only noted follow-up is the wizard apply-step hot-swap into `deputyos-remote-wizard` at registration time (M9.6), which is a UX nicety, not a blocker. v2 app-provisioned cloud VMs (Hetzner orchestrator) outlined under M9, not v1 build scope.
- **Landed this cycle (M11 — cross-platform desktop app, console-first)**: making the **deputyOS Console** (Tauri) a real double-click app, with the launcher as its engine. **Linux GUI, proven end-to-end**: fixed the load-bearing "open wizard/remote" bug (the frontend called `window.__TAURI__.core.window.open`, not a real Tauri v2 API — now a Rust `open_url` command reusing `deputyos_desktop::browser::open_url`); added live per-instance status polling (`status_instance` on a timer, not the stale registry `last_status`) and a host-prereq banner (`host_prereq` surfaces the driver's `check_prereq` hint); generated the icon set and enabled `bundle.active` so `cargo tauri build` / `make console-bundle` produce AppImage/deb (and dmg/msi on their runners). **Packaging/CI**: new `console-bundle` matrix job (ubuntu/macos/windows) attaches installers to the release (unsigned for now — Gatekeeper/SmartScreen bypass documented in `how-to/operate/desktop-console.md`); the `launchers` job now feeds its mac/win binaries into `build/` before sign+manifest so `desktop_launchers` advertises **all 5 triples** (was Linux-only); the `wsl2` base-rootfs SHA512 is pinned (unblocks the Windows tarball artefact). **mac/win drivers hardened**: Windows gains the per-instance host-port remap (`netsh portproxy` to give each WSL distro a distinct `localhost` port) the roadmap had flagged as follow-up; macOS/UTM (which genuinely can't remap) now warns loudly on non-default ports instead of silently colliding; new arg-builder unit tests lock the command lines. **Honest boundary:** the mac/win drivers + installers are compiled/packaged by CI runners but the real "double-click on a Mac/PC and manage agents" smoke is **pending hardware** — this pass proves Linux end-to-end and makes mac/win one hardware-test away.
- **Landed this cycle (M10 — rebrand + 4-repo split)**: the project rebranded from **agentOS → deputyOS** on the **deputyos.com** domain, and is being reorganised from three `Skelf-Research/*` repos into four repos under the `deputyos` GitHub org: `deputyos/deputyos` (this open-source core), `deputyos/api-deputyos-com`, `deputyos/app-deputyos-com` (login/account frontend, carved out of the www dashboard), `deputyos/www-deputyos-com` (pure SEO/AI-search Astro site). Phase 1 (this repo) is a deep, clean-cutover rename — crates (`agentctl→deputyctl`, `agentwizard→deputywizard`, `agentpwa→deputypwa`, `agentos-{desktop,console,track}→deputyos-*`), systemd units (`agentos-*.service→deputyos-*`), baked FS paths (`/etc/agentos→/etc/deputyos`, +`/opt`/`/var/lib`/`/run`/`/mnt`), env vars (`AGENTOS_*→DEPUTYOS_*`), schema id (`agentos.limits/v1→deputyos.limits/v1`), mDNS (`agentos.local→deputyos.local`), Tauri bundle (`com.deputyos.console`), SLSA builder ids, and domains — with **no back-compat shims** (no external users yet; images rebuild fresh). Deliberately preserved: the `agent` Unix user/group + `/home/agent` + profile data dirs, and the upstream profile/gateway names (`openclaw`/`hermes`/`khoj`). Open: sibling-repo rebrand (api/www/app — Phases 2–3), the GitHub org move with fresh history (Phase 5), and `deputyos.com` DNS + Cloudflare/CDN/R2 cutover (Phase 6). The cross-platform desktop-app build-out (mac/win drivers, richer local openclaw+hermes management) is the next milestone after the split lands.
- **Landed this cycle (M5.5)**: egress whitelist guardrail — substantially built and now live. `Mode::Whitelist` + the nftables generator (per-host resolved-IP `accept` under `policy drop`) were already in `deputyctl/src/network.rs`; this cycle closed the gaps: routed the policy path through `paths::network_policy_file()` (env-overridable), seeded `allow_hosts` from the per-profile `network-defaults.json` on `mode whitelist` (idempotent), wired the wizard `Step::Egress` choice into `apply.rs` (dev-gated), added the always-on `deputyos-network-apply.service` boot oneshot (skips `open` so ufw inbound isn't wiped), added the `deputyctl doctor network-policy` drift check, added the profile manifest `[default_egress]` section, and shipped the `threat-model-egress.md` (Lane E — DNS-only-not-SNI, resolver-trust, IPv6 failure modes) + how-to + schemas. Open follow-ups: per-channel-on-enable auto-defaults; PWA one-click revoke + blocked-recently log; `network-policy-schema-version` release-manifest field; live boot smoke (out-of-sandbox).
- **Landed this cycle (M2.5 Lane D)**: launcher-binary self-update. The `desktop_launchers` schema field shipped in `deputyctl::release` but was never consumed — `deputyos-desktop` could refresh the VM image (`update`) but not itself. This cycle adds `deputyos-desktop self-update`: it looks up `desktop_launchers[<host-triple>]` in the verified manifest, sha256-compares the entry to the running binary, then downloads + sha256 + minisign verifies the new launcher blob against the same embedded pubkey and atomically swaps it into place (POSIX `rename` + `chmod 0o755`; Windows `.exe.old` move-aside). The running process is never replaced in memory — the new launcher takes effect on the next launch. `update` now also prints a one-line "run self-update" hint when a newer launcher is advertised. Side fix: factored a portable `sha256_hex` (sha2) out of `verify_sha256`, retiring the `sha256sum` coreutils shellout and fixing a latent Windows portability bug in the image install/update path. Open: code-signing the launcher binary (Lane E — in procurement); cross-compile matrix CI (Lane B — `make desktop-launcher` target exists); picker "Download for …" row (Lane G — sibling repo); real Win/Mac/Linux double-click smoke (Exit gate — out-of-sandbox).
- **Landed this cycle (M2.5 dev-UX)**: run-the-whole-stack-locally loop. `make desktop-local-build && make cdn-up && make desktop-local` drives a fully-local end-to-end deputyOS: a Docker-hosted "remote" — a `cdn` nginx service (`docker-compose.dev.yml`, `:8090`) serving the signed manifest + qcow2 + launcher alongside the existing `api` (`:3000`) + `www` (`:4321`) — that the desktop installer pulls from, booting a real `qemu-x86_64` VM whose in-VM agent is wired at the local API via a cloud-init seed (`DEPUTYOS_API_BASE=http://10.0.2.2:3000`) instead of `api.deputyos.com`. Pieces: `scripts/manifest.sh` now emits `desktop_launchers`; `scripts/sign.sh` signs launcher binaries; `make desktop-launcher-release` + `scripts/host-triple.sh` stage the host-triple launcher; `make desktop-local-build` symlinks the Packer output name to the manifest-conforming name (bridging a pre-existing build↔manifest naming gap); `scripts/desktop-local.sh` builds the NoCloud seed and drives `deputyos-desktop install && start`; and the LinuxDriver gained an opt-in `DEPUTYOS_DESKTOP_SEED_ISO` cloud-init drive-attach path (extracted into a testable `qemu_argv`). See `documentation/docs/how-to/develop/run-locally.md`. Open: the aarch64 qemu user-net host gateway differs (loop is x86_64-first); real Win/Mac host drivers; live boot smoke (out-of-sandbox — the sandbox lacks minisign/Packer/ISO builders).
- **Out-of-sandbox infra** (tracked at end of doc): hardware soak, real CDN,
  external audit, code-signing certs, marketplace submission, accounts.deputyos.com hosting.

## Phase exit gates

| Phase | Exit gate |
|---|---|
| **M0** | Docs reviewable on paper: a semi-tech reader can predict the appliance behaviour and a contributor can predict where to add a profile. |
| **M1** | OpenClaw runs under systemd on a manually-built rpi5 image with security baseline live and `deputyctl doctor` green. |
| **M2** | All hardware matrix targets build reproducibly and pass QEMU smoke; Hermes profile parity with OpenClaw; picker page returns the right artefact for any device. |
| **M2.5** | Non-technical user on Win/Mac/Linux double-clicks one binary and sees the wizard in their browser within 2 minutes. |
| **M3** | Fresh boot → flash device → scan QR → web wizard → Telegram round-trip in ≤10 minutes for a non-technical user with no prior context. |
| **M3.5** | Agent can read/write user-selected files: WSL2/qemu/UTM virtiofs share mounted; bare-metal USB auto-mounts under `/mnt/deputyos/<label>`; SMB/NFS share configurable from wizard; `deputyctl mounts` and PWA card surface every active mount with a one-click revoke. |
| **M4a** | Public domain `deputyos.com` resolves; `www.deputyos.com` (Astro) ships with landing + docs + picker + blog; `api.deputyos.com` (Vue 3 + Tailwind) renders a live build-status matrix; first public release `v0.3.0` tagged on GitHub with signed artefacts. |
| **M4b** | Upstream releases auto-PR a version bump, CI builds all targets, signed manifest publishes to `cdn.deputyos.com`, `deputyctl update` applies cleanly on a real device. |
| **M4.5** | `make build TARGET=<hw> TIER=<t> AIRGAP=1` produces an image that boots with `-net none` and serves a working chat against a baked-in LFM2 model (size per tier); `deputyctl model set` swaps among baked GGUFs without network. |
| **M5** | DO 1-Click listing approved; cost-guardrail trips in a test scenario and the user sees a clear notification + agent auto-pause. |
| **M5.5** | Wizard offers three egress modes (open / whitelist / airgap) backed by nftables; per-channel/per-provider hostname allow-list reasonable defaults shipped per profile; refusals surface "to enable, run `deputyctl network allow add <host>`". |
| **M6** | A/B rollback verified by deliberately shipping a broken image: device auto-recovers within 5 min; voice wake-word demo runs in <2 s on rpi5 8GB. |
| **M7** | Third profile ships from a manifest + bake recipe alone (no Rust changes); SLSA L3 attestations verified by a third party. |
| **M8** | Free-tier account at `accounts.deputyos.com` lets a user back up an OpenClaw install on device A and restore on device B from a fresh image; client-side encryption keys never leave the device; full local-first operation continues to work without an account. |

## Phase checklists

### M0 — Foundations (2–3 days)

- **Lane A — deputyctl**
  - [x] Frozen command surface in `02-profiles.md` and an `deputyctl --help` mock.
  - [x] Profile manifest schema (TOML) drafted with `openclaw.toml` and `hermes.toml`.
- **Lane B — image build**
  - [x] `03-image-builds.md` describes the shared Ansible role + variant gates pattern.
  - [x] Build matrix table finalised.
- **Lane C — wizard/UX**
  - [x] Wizard flow described in `01-getting-started.md` and `05-model-providers.md`.
  - [x] Provider list confirmed.
- **Lane D — release/update**
  - [x] `04-release-tracking.md` describes tracker → manifest → A/B swap.
  - [x] `08-update-rollback.md` describes A/B partitions and watchdog.
- **Lane E — security**
  - [x] `09-security.md` lists every default-on control.
  - [x] Threat model draft + ADR `0008-clamav-plus-magika-baseline.md`.
- **Lane F — profiles**
  - [x] Two concrete profile manifests in `profiles/`.
  - [x] Pain-point → mitigation matrix (`10-troubleshooting.md`).
- **Lane G — site/docs**
  - [x] `README.md` finalised (with prominent per-target limitations table); `LICENSE` (Apache-2.0); ADRs 0001–0008.
  - [x] Repo scaffolding (`CONTRIBUTING.md`, `CODEOWNERS`).
  - [x] Limitations map ([`14-limitations.md`](14-limitations.md)) — canonical per-target / per-feature / per-version constraints with surfacing plan threaded into wizard, picker, `deputyctl`, PWA.

**Exit:** Plan + docs reviewable; alignment locked before any code lands.

### M1 — Walking skeleton (4–5 weeks)

- **Lane A — deputyctl**
  - [x] Cargo crate scaffolded; `init` (spawns wizard), `up`, `down`, `restart`, `status`, `logs`, `doctor`, `limits` working.
  - [x] Profile loader reads TOML; systemd unit template renders correctly.
  - [x] `doctor` checks every item from §9 and prints a one-liner fix (13 checks).
  - [x] `limits` enumerates per-device constraints from a build-time manifest baked into `/etc/deputyos/limits.json`; output matches the spec in [`docs/14-limitations.md`](14-limitations.md). Every refusal message in `deputyctl` references `deputyctl limits`.
- **Lane B — image build**
  - [x] Shared Ansible role created with first-class hardware variant gates.
  - [ ] Manual `pi-gen` + `packer-builder-arm` build produces a working `rpi5` `.img.xz` — PIN-config in place, untestable in sandbox (needs binfmt + real Pi). `[deferred:hardware]`
  - [ ] QEMU aarch64 boot test in CI gates merges to main — wired in Makefile via `make smoke`; CI defaults to SCAFFOLD_PHASE=1. `[wired:CI-gated-by-resources]`
  - [x] `Makefile` ships with `doctor`, `build TARGET=qemu-aarch64`, `try TARGET=qemu-aarch64`, `smoke TARGET=qemu-aarch64`, `sign-dev`. Documented in [`docs/15-local-build.md`](15-local-build.md). Linux x86_64 host first; macOS + WSL2 follow in M2.
- **Lane C — wizard/UX**
  - [ ] TTY wizard prototype: hostname/timezone/WiFi/profile/single provider.
  - [ ] Wireframe of web wizard committed (HTML mock, no backend yet).
- **Lane D — release/update**
  - [ ] GitHub repo + CI scaffolding (matrix builds, lint, fmt, test).
  - [ ] B2 buckets created (project + signing key vault); minisign keypair generated and stored offline.
- **Lane E — security**
  - [x] AppArmor profile for OpenClaw enforcing.
  - [x] ufw default-deny + fail2ban + key-only SSH baked in.
  - [x] ClamAV running with packed signature DB; daily scan timer.
  - [x] Hardened sysctl bundle applied at boot.
- **Lane F — profiles**
  - [x] OpenClaw profile end-to-end: install at build time, baked, runs on rpi5 image.
- **Lane G — site/docs**
  - [ ] Docs site live on Cloudflare Pages (rendering `docs/` markdown).
  - [ ] Public roadmap page mirrors this file with live status.

**Exit:** OpenClaw on rpi5 runs unattended for 24h; `deputyctl doctor` green; ClamAV reports healthy; Telegram round-trip confirmed manually. **Not yet met** — 24h hardware soak deferred (`[deferred:hardware]`).

### M2 — Full matrix + Hermes (6–8 weeks)

- **Lane A — deputyctl**
  - [x] `profile switch` works (stop old unit, start new, swap home symlink).
  - [x] `model list/set/test` validates keys.
  - [x] `backup now` (rclone driver) writes to user's B2/R2 bucket.
- **Lane B — image build**
  - [x] Variants implemented and CI-gated for: `rpi5`, `rpi4`, `arm64-generic`, `x86_64-mini-pc`, `wsl2`, `macos-qemu`, `digitalocean`, `oracle-arm-free`, `hetzner-cloud`, `fly-machines`, `vultr`/`linode` cloud-init, `proxmox`/`unraid`/`truenas` templates.
  - [x] QEMU aarch64 + qemu-x86_64 smoke tests gate every target.
  - [ ] Reproducible-build check: same source → same SHA256 across two builders — needs second build host. `[deferred:infra]`
  - [x] `make matrix` works on Linux x86_64, Linux arm64, macOS (Apple Silicon + Intel), Windows WSL2 — full target matrix builds locally on every supported host.
  - [x] `try.sh` quickstart and launcher defaults point at `https://cdn.deputyos.com`.
- **Lane C — wizard/UX**
  - [ ] Picker page live at `deputyos.com`, reads signed `manifest.json`. `[deferred:domain]`
  - [ ] Picker page renders per-target limitations panel (sourced from `docs/14-limitations.md`) before download — collapsible but not hideable. `[deferred:domain]`
  - [x] Wizard adds backup-bucket setup (B2/R2/Cloudflare-OAuth path).
- **Lane D — release/update**
  - [x] CI publication path targets project B2 and exposes signed artefacts through `cdn.deputyos.com`.
  - [x] CDN URL scheme stable; manifest schema v1 frozen.
- **Lane E — security**
  - [x] AppArmor profile for Hermes enforcing (Hermes needs `kernel.unprivileged_userns_clone=1`).
  - [x] Magika wired into agent file-upload path (rejects/flags mismatches).
  - [x] Secrets storage at `/etc/deputyos/secrets.env` mode 0600 verified.
- **Lane F — profiles**
  - [x] Hermes profile end-to-end on every matrix target.
  - [x] Profile manifest schema validator in CI (`deputyctl profile validate`).
- **Lane G — site/docs**
  - [ ] Picker page UX iterated; per-target install guide. `[deferred:domain]`
  - [ ] Showcase gallery scaffolded. `[deferred:domain]`

**Exit:** All targets in the matrix build reproducibly and pass smoke; Hermes parity with OpenClaw; user can pick → download → flash → boot any target. **Not yet met** — reproducible-build cross-builder verification deferred (`[deferred:infra]`); picker/CDN deferred (`[deferred:domain+CDN]`).

### M2.5 — One-click desktop launch (4-6 weeks)

A non-technical Win/Mac/Linux user downloads ONE binary, double-clicks, sees
the wizard in their browser within 2 minutes — without ever knowing what QEMU is.

**Architecture**: mandate the platform-native virtualization. No bundled QEMU
(GPL distribution complexity, 80-100MB per platform, version-drift maintenance).
Each platform leverages best-in-class native tooling:

- **Windows**: WSL2 (mandatory; Microsoft's `wsl --install` is one PowerShell
  command on Win10 21H2+ / Win11). Launcher imports `deputyos-wsl2-<v>.tar.gz`.
- **macOS**: UTM (mandatory; free from App Store; uses Vz.framework for
  near-native speed on Apple Silicon). Launcher uses `utmctl` to register +
  start a VM around `deputyos-macos-qemu-<v>.qcow2`.
- **Linux**: qemu-system-* + KVM (apt/dnf/pacman ubiquitous). Launcher spawns
  qemu directly with KVM accel against `deputyos-qemu-<arch>-<v>.qcow2`.

If the prereq is missing, the launcher prints exact install instructions per
platform and exits. No fallback to bundled QEMU — keeps the binary tiny (~5MB)
and the maintenance surface small.

| Lane | Item |
|---|---|
| **A — deputyctl** | None. Launcher reuses `deputyctl::release::{Manifest, verify_manifest_signature}`. |
| **B — image build** | New crate `deputyos-desktop` cross-compiled for x86_64-pc-windows-msvc, aarch64-apple-darwin, x86_64-apple-darwin, x86_64-unknown-linux-gnu, aarch64-unknown-linux-gnu. `make desktop-launcher TARGET=<triple>`. |
| **C — wizard/UX** | Launcher opens default browser to wizard URL after VM boots. First-run UI = terminal progress + browser open. Wizard is the visible UX. |
| **D — release/update** | Manifest extension: `desktop_launchers: { <triple>: { url, sha256, minisig } }`. `deputyos-desktop update` self-updates. GitHub Releases attaches 5 launcher binaries per release. |
| **E — security** | Apple Developer ID + notarization on macOS .app (infra-TODO). Windows EV cert (infra-TODO). Linux AppImage gpg signing (in-band). |
| **F — profiles** | None (launcher is profile-agnostic; user picks during the wizard). |
| **G — site/docs** | Picker page at deputyos.com offers "Download for Windows / macOS / Linux" prominently. README "Try it without buying a Pi" section adds the launcher row. |

**Reuses existing build outputs** — the launcher consumes 4 image targets we
already produce: `wsl2`, `macos-qemu`, `qemu-aarch64`, `qemu-x86_64`. No new
image targets needed.

**Exit:** A non-technical user on a clean Win/Mac/Linux machine downloads the
right launcher, double-clicks, sees the wizard within 2 minutes. Prereq dance
is one dialog box and one click to install (or one terminal command they can
copy-paste).

### M3 — First-boot is a delight (4–5 weeks)

- **Lane A — deputyctl**
  - [x] Axum + HTMX web wizard backend on `:8088`.
  - [x] Wizard flow: hostname/timezone/WiFi/profile/provider/channels/allowlist/backup/Tailscale (hostname/timezone/profile/provider/channels/ssh/Tailscale/CF-Tunnel/backup).
  - [x] Real round-trip key validation per provider (5s timeout + skip-validation checkbox).
- **Lane B — image build**
  - [x] Wizard binary baked; mDNS publishes `deputyos.local` (`wizard-baseline.yml` + `networking-baseline.yml`).
  - [x] Pre-warmed `/chat` web UI baked at `/var/www/chat`.
- **Lane C — wizard/UX**
  - [x] QR-code provisioning: TTY/HDMI prints QR pointing at `https://deputyos.local/wizard?token=…` (`deputywizard print-qr` + `deputyos-qr-on-tty.service`).
  - [x] Built-in private web chat at `/chat` (works without any external channel).
  - [x] Companion PWA at `/app` (`deputypwa` crate; status, logs, key rotation, push-subscribe).
  - [x] Wizard shows live headroom (RAM, disk, channel cost) at every step; refuses tight combinations with a clear reason and a one-line "unblock" hint per [`docs/14-limitations.md`](14-limitations.md). (RAM-tier channel gating from `limits.json`.)
  - [x] PWA "Your device" card permanently visible on the dashboard; lists active capabilities, unavailable features with reasons, and one-line upgrade hints.
- **Lane D — release/update**
  - [ ] Manifest grows fields for wizard version + chat-UI version.
- **Lane E — security**
  - [x] Wizard refuses to bring up channels until `deputyctl doctor` green.
  - [x] Wizard-issued tokens are single-use, mode 0600, expire in 1h.
  - [ ] Web Push outbound RFC 8030 delivery — VAPID keypair + subscription persistence wired; outbound POST stubbed. `[partial:hook-handler-wired-when-cost-alert-fires]`
- **Lane F — profiles**
  - [ ] Skill/template registry mirror live on B2; agents install skills offline from mirror. `[deferred:bucket]`
- **Lane G — site/docs**
  - [ ] "From flash to first chat in 10 minutes" guide with screenshots.

**Exit:** Non-technical tester goes from SD card in hand to first agent reply within 10 minutes, no terminal. **Not yet met** — outbound Web Push partial; skill registry mirror deferred (`[deferred:bucket]`); end-to-end SD-card test requires hardware (`[deferred:hardware]`).

### M3.5 — Drive mounting (3–4 weeks) **[DONE: all lanes except www marketing page]**

The agent must reach user files safely when deputyOS runs on user machines. Three sub-surfaces, all gated through one common `/etc/deputyos/mounts-policy.json`. AppArmor permits only `/mnt/deputyos/**`; every mount has a mode (`ro`/`rw`), a visible-to-agent path, and is revocable from CLI/PWA in one click.

- **Lane A — deputyctl**
  - [x] `deputyctl mounts {list,add,remove,apply,health}` subcommand backed by `mounts-policy.json`. Wraps the materialiser (`deputyos-mounts.service`).
  - [x] `deputyctl limits` adds "Connected drives" + "Network shares" sections; refusals carry a one-line unblock hint.
  - [x] `deputyctl doctor mounts-health` pings each share + verifies AppArmor enforcement.
- **Lane B — image build**
  - [x] New `roles/deputyos/tasks/mounts-baseline.yml` always runs; per-variant additions:
    - `variant-wsl2.yml` detects `/mnt/c`, generates per-folder systemd-mount units gated on policy.
    - `variant-rpi5.yml` / `variant-x86_64-mini-pc.yml` install the `99-deputyos-removable.rules` udev rule + `deputyos-mount-removable.sh` helper.
    - All variants get `cifs-utils` + `nfs-common` on `standard`+ tier.
  - [x] AppArmor profiles for OpenClaw / Hermes / Khoj extended with `/mnt/deputyos/** rwk` permissive entries gated by policy file.
- **Lane C — wizard/UX**
  - [x] New wizard step "Where can the agent see your files?" — per-mount toggle with capability/refusal lines from `docs/14-limitations.md`.
  - [x] PWA "Mounts" card on dashboard: per-mount mode, last-touched, one-click revoke.
  - [x] Wizard "Network shares (advanced)" tile collects host/path/credentials; credentials live in `secrets.env` (never in policy file).
- **Lane D — release/update**
  - [x] Manifest fields for `mounts-policy-schema-version`; updates that change the policy schema bump it.
- **Lane E — security**
  - [x] Mount options for unknown filesystems force `nosuid,nodev,noexec`.
  - [x] LUKS-encrypted volumes detected and surfaced as refusal until unlocked.
- **Lane F — profiles**
  - [x] Profile manifest schema gains `mounts: { default_mode, suggested_paths[] }`; profiles can default-suggest paths during the wizard.
- **Lane G — site/docs**
  - [x] `documentation/docs/how-to/operate/mount-drives.md` (new); cross-link from `14-limitations.md`.
  - [ ] Marketing page on `www.deputyos.com` showing "your files, agent's hands" demo screenshots.

**Exit:** WSL2 launcher with host folder pointed at `~/Documents`: agent reads/writes a file; `deputyctl mounts list` and PWA both show it. Pi 5 with USB stick plugged in: udev auto-mounts under `/mnt/deputyos/<label>`. SMB share configured from wizard against a local Samba container: `deputyctl doctor mounts-health` passes.

### M4a — Public domain + GitHub release + websites (3–4 weeks) **[DONE: sites live in sibling repos]**

Move deputyOS from a private/early repo into a public preview. Domain swap to `deputyos.com` (already complete in code); first signed-and-mirrored release; a marketing+docs site optimised for SEO and AI-search consumption; a read-only build-status dashboard. Keeps the "every CI step is a thin wrapper around make" rule.

- **Lane A — deputyctl**
  - [ ] CDN URL configurability hardened so a mistyped channel falls back to dev cleanly; respects `deputyos.dev` 301 → `deputyos.com` migration for any cached configs.
- **Lane B — image build**
  - [x] `deputyos-api` Rust crate (read-only Axum API): `/api/manifest`, `/api/builds`, `/api/health`, `/api/sboms/<id>`. **Extracted to sibling private repo `deputyos/api-deputyos-com`** along with account, tunnel, and backup services.
  - [x] `make publish-cdn` wraps `make publish-local` and copies to Backblaze B2 via `rclone`; `publish-r2` remains a compatibility alias. CI is a thin wrapper.
- **Lane C — wizard/UX**
  - [ ] Wizard footer renders the upstream `deputyos-api` build-status badge (when network available).
- **Lane D — release/update**
  - [x] `.github/workflows/release.yml` runs the matrix, signs a release-mode manifest, publishes to B2, and attaches all launcher/Console artefacts to GitHub Releases.
  - [ ] `deputyos.dev` legacy domain points at a Cloudflare 301 redirect rule for the (likely unused, but anyway preserved) old paths.
  - [ ] First public preview cut as `v0.3.0` after M3.5 + M4.5 land.
- **Lane E — security**
  - [ ] `SECURITY.md` published in repo root; disclosure SLA from `docs/09-security.md`. `security@deputyos.com` mailbox stood up.
- **Lane F — profiles**
  - [ ] Profile pages on `www.deputyos.com/profiles/<id>` — content sourced from `profiles/<id>.toml` + `documentation/docs/concepts/profile-class.md`.
- **Lane G — site/docs**
  - [x] `www.deputyos.com` — Astro 5 + MDX + Tailwind + Pagefind project in **sibling private repo** `deputyos/www-deputyos-com`. Landing, docs, picker, blog, RSS, llms.txt, JSON-LD SEO. Deploys to Cloudflare Pages.
  - [x] `api.deputyos.com` — Vue 3 + Vite + Tailwind status dashboard + Rust Axum API in **sibling private repo** `deputyos/api-deputyos-com`. Build matrix, health grid, manifest summary. Deploys to Cloudflare via its own CI.
  - [x] `docs.deputyos.com` — MkDocs Material from this repo's `documentation/` directory. Deploys via `.github/workflows/docs-deploy.yml`.
  - [ ] `make site` and `make status` build/serve locally from sibling repos.

**Exit:** `https://www.deputyos.com` resolves and serves landing + docs + picker; Lighthouse ≥95 SEO and ≥95 perf on `/`. `https://api.deputyos.com` shows live matrix. `deputyos.dev` 301-redirects to `deputyos.com`. `v0.3.0` tag on GitHub with signed artefacts mirrored to `cdn.deputyos.com`.

### M4b — Self-updating fleet (3–4 weeks)

- **Lane A — deputyctl**
  - [x] `deputyctl update --check / --apply` works against the manifest.
  - [x] minisign + cosign signature verification gates apply.
- **Lane B — image build**
  - [ ] NPU `+hailo` and `+coral` rpi5 build variants ship. `[deferred:hardware]`
  - [ ] Matrix size accommodates ClamAV signature freshness in every cut.
- **Lane C — wizard/UX**
  - [ ] Update prompts in PWA + TTY; "what's new" rendered from manifest.
- **Lane D — release/update**
  - [x] Release-tracker bot polls upstream every 30 min, opens version-bump PRs (`deputyos-track` crate, `*/30 * * * *` cron workflow).
  - [ ] CI promotes stable channel from beta after a soak window. `[deferred:CI-runtime]`
  - [ ] CDN warmup hook on publish. `[deferred:CDN]`
  - [x] `make verify VERSION=<v> TARGET=<hw>` rebuilds a published image and asserts SHA256 match — the user-facing reproducibility verification path.
- **Lane E — security**
  - [ ] ClamAV signature DB shipped in every image rev; out-of-band signature-only patches supported.
  - [ ] CVE-feed monitoring wired to security@ inbox.
- **Lane F — profiles**
  - [ ] Profile pinned-version field consumed by tracker; per-profile beta channels.
- **Lane G — site/docs**
  - [ ] Public release notes index; per-release diff renders. `[deferred:domain]`

**Exit:** Upstream cuts a release → automated PR → CI build → signed manifest → real Pi pulls update → A/B swap succeeds. **Not yet met** — A/B swap requires hardware (`[deferred:hardware]`); CDN/CI promotion deferred.

### M4.5 — Air-gapped fat tier (2–3 weeks)

Reuse existing `TIER=lean|standard|rich` knob + new `AIRGAP=1` flag. An airgap image bakes everything: package mirror, signature DB, plus a tier-appropriate LFM2 GGUF served by `llama.cpp`. Boots and serves the wizard + chat with `-net none`.

- **Lane A — deputyctl**
  - [x] `deputyctl model list/set/register` works against a curated catalog at `/opt/deputyos/airgap/models/catalog.json`. `register` accepts a local `.gguf` path — never reaches the network.
  - [x] New `deputyctl network {mode,allow,unlock,status}` skeleton: today only `mode=open|airgap` honored; the `whitelist` mode is reserved for M5.5 (schema designed for forward compatibility).
- **Lane B — image build**
  - [x] Makefile knob `AIRGAP ?= 0`; plumbed via Packer extra-var `deputyos_airgap`.
  - [x] New `roles/deputyos/tasks/airgap-baseline.yml`: when `deputyos_airgap | bool`, point apt to `file:///opt/deputyos/airgap/apt-mirror/`, install nftables + ufw rule denying egress except RFC1918 + mDNS, bake `/etc/deputyos/network-policy.json` with `mode=airgap`. *(Delta: nftables-only, not nftables+ufw — the dual-firewall conflict is documented in `concepts/airgap.md` §Threat-model delta.)*
  - [x] New `roles/deputyos/tasks/llm-airgap.yml` + `roles/deputyos/vars/llm-airgap.yml` with SHA-pinned GGUF URLs:
    - `lean`  → `LFM2-350M-Q4_K_M.gguf` (~250 MB)
    - `standard` → `LFM2-1.2B-Q4_K_M.gguf` (~750 MB)
    - `rich` → `LFM2-2.6B-Q4_K_M.gguf` (~1.6 GB) + `Qwen2.5-Coder-1.5B-Instruct-Q4_K_M.gguf` (~1.0 GB)
  - [x] `deputyos-llamacpp@.service` template (alongside existing `deputyos-llamacpp.service`) for parallel-port serving of multiple baked models.
  - [x] Limits JSON: extend `roles/deputyos/files/limits.<target>.json` with `airgap_supported: bool`.
- **Lane C — wizard/UX**
  - [x] Wizard hides any "API key" providers when `airgap=true`; shows "Local LLM (LFM2)" as the default + tested provider.
  - [x] PWA "Your device" card adds an "Airgap" badge with link to docs/limits.
- **Lane D — release/update**
  - [x] Update flow for airgap images: `deputyctl update --from /mnt/deputyos/<usb>/manifest.json` (sneakernet path); existing minisign + cosign verification gates still apply.
- **Lane E — security**
  - [x] Confirm nftables rules reset to declared policy on every boot (no drift).
  - [x] Document the threat model delta in `documentation/docs/concepts/airgap.md`.
- **Lane F — profiles**
  - [x] OpenClaw + Hermes + Khoj profile manifests gain `airgap_default_provider: "local-llamacpp-airgap"` so the airgap build picks the right local model automatically. *(Landed as `[airgap] default_provider` in the profile TOML — resolved by the wizard to the catalog default or a pinned `airgap-<id>`.)*
- **Lane G — site/docs**
  - [x] `documentation/docs/concepts/airgap.md` and `documentation/docs/build/airgap-builds.md` (new); cross-link from `docs/12-bundled-software.md` §5 (new section: airgap tier sizes).
  - [ ] `www.deputyos.com/picker/` adds an "Air-gapped" toggle that filters the matrix. *(Sibling repo `www-deputyos-com` — out of scope here.)*

**Exit:** `make build TARGET=qemu-x86_64 PROFILE=openclaw TIER=rich AIRGAP=1` produces an image; boots in QEMU with `-net none`; `curl localhost:8088/chat` returns a streamed completion from the baked LFM2-2.6B in <2s. Same for `lean` (LFM2-350M) and `standard` (LFM2-1.2B). `deputyctl model set LFM2-1.2B` swaps mid-flight without network.

### M5 — Marketplace + guardrails (3–4 weeks)

- **Lane A — deputyctl**
  - [x] `cost guardrails`: per-day + per-month caps with auto-pause + wizard UI (auto-pause via cost-tripped marker).
  - [x] `quiet hours` schedule; `factory-reset` (data partition wipe with typed confirmation).
- **Lane B — image build**
  - [ ] DO snapshot lineage submitted to 1-Click Marketplace. `[deferred:submission]`
- **Lane C — wizard/UX**
  - [x] Cost-graph card on PWA dashboard (data shape exposed via `deputyctl cost --json`; PWA renders).
  - [ ] "Spending paused" banner with one-tap raise-cap action.
- **Lane D — release/update**
  - [ ] DO Marketplace versioning hooked to manifest publishes. `[deferred:submission]`
- **Lane E — security**
  - [ ] `security@` inbox + 90-day disclosure SLA published. `[deferred:domain+inbox]`
  - [ ] External audit vendor selected and engaged. `[deferred:procurement]`
- **Lane F — profiles**
  - [x] Per-profile cost-tracking hooks wired (per-token spend telemetry, opt-in) — ledger schema documented.
- **Lane G — site/docs**
  - [ ] Forum + Matrix space launched; contribution guide v1. `[deferred:hosting]`

**Exit:** DO 1-Click listing live; deliberate cost-cap test trips guardrail and user sees the right UI. **Not yet met** — Marketplace submission deferred (`[deferred:submission]`); audit deferred (`[deferred:procurement]`).

### M5.5 — Egress whitelist guardrail (2–3 weeks)

Extends the M4.5 `network-policy.json` schema with a third mode `whitelist`. Today's posture is open; airgap is the all-off counterpart; whitelist sits between — open internet, but only to a curated allow-list of hostnames per profile/channel. Enforced via nftables (already pulled in by airgap-baseline) so the rule applies to every process on the device including hooks and skills.

- **Lane A — deputyctl**
  - [x] `deputyctl network` graduates `mode=whitelist` from reserved → live. `deputyctl network allow add <host>` / `remove <host>` / `status` mutate `/etc/deputyos/network-policy.json` and reload nftables atomically.
  - [ ] Refusal messages anywhere in deputyctl mention "to enable, run `deputyctl network allow add <host>`". *(Deferred — would require hooking every outbound call site to catch nft-refused connections; the doctor check + `network status` surface the posture instead.)*
- **Lane B — image build**
  - [x] Profile-specific defaults in `roles/deputyos/files/network-defaults.<profile>.json` (Telegram + Signal gateways for OpenClaw; OpenAI/Anthropic for any cloud-LLM provider; etc.).
  - [x] nftables rule generation idempotent; survives reboots; resets to policy on every `mounts-baseline.yml`-equivalent oneshot.
- **Lane C — wizard/UX**
  - [x] Wizard "Outbound network" tile with three modes (open / whitelist / airgap); per-channel sensible defaults applied automatically when enabling a channel. *(Profile-level `[default_egress]` + `network-defaults.json` seeding landed; per-channel-on-enable auto-default deferred.)*
  - [ ] PWA card lists all allow-listed hosts with one-click revoke + a "blocked recently" log. *(The `/app/network` card renders mode + allow_hosts read-only; revoke + blocked-log deferred.)*
- **Lane D — release/update**
  - [ ] Manifest schema field `network-policy-schema-version` added; updates that change the policy schema bump it. *(Deferred — the `network-policy-v1.json` schema already documents "Schema version 1; bump on breaking changes"; whitelist was already in the v1 enum so no bump was needed.)*
- **Lane E — security**
  - [x] Threat model addendum in `documentation/docs/concepts/threat-model-egress.md`; document failure modes (DNS-only allow vs full SNI inspection).
- **Lane F — profiles**
  - [x] Profile manifest schema gains `default_egress: { mode, allow_hosts[] }`.
- **Lane G — site/docs**
  - [x] `documentation/docs/how-to/operate/egress-whitelist.md`.

**Exit:** Wizard offers three egress modes; switching to `whitelist` with no allow-list refuses every outbound request; adding `api.openai.com` enables the OpenAI provider while still blocking unrelated hosts; verified by `deputyctl doctor` + an `deputyctl network status` snapshot. **Met in-sandbox** — the hermetic `network::tests` (env-override, seeding idempotence, ruleset generation) + `doctor network-policy` check + the boot oneshot assert the mechanism; live two-device boot smoke stays out-of-sandbox.

### M6 — Survivability + voice (4–5 weeks)

- **Lane A — deputyctl**
  - [ ] A/B slot manager; `deputyctl rollback` and watchdog auto-rollback — `deputyctl rollback` validates inactive-slot integrity then refuses to swap (M6 contract). `[deferred:hardware]`
  - [ ] `restore --list / --snapshot` end-to-end — implemented via rclone; needs cloud bucket to test end-to-end. `[partial:bucket]`
  - [x] On-device voice wake-word and TTS on rpi5 (whisper.cpp + Piper) — scaffolded.
- **Lane B — image build**
  - [ ] A/B partition layout standardised across all targets supporting it. `[deferred:hardware]`
  - [x] rpi5 ships with whisper.cpp small + Piper voice baked (whisper-tiny.en + Piper voice; offline-capable bake).
- **Lane C — wizard/UX**
  - [ ] Voice setup UI; wake-word picker; mic permission consent flow.
  - [x] Tailscale + Cloudflare Tunnel opt-in cards in wizard.
- **Lane D — release/update**
  - [ ] Watchdog telemetry: failed updates auto-roll back; report uploaded (opt-in).
- **Lane E — security**
  - [ ] External audit fieldwork in progress; tracking issues public. `[deferred:procurement]`
- **Lane F — profiles**
  - [x] Voice channel hooked to both OpenClaw and Hermes (gateway abstraction) — via `pre-message` hook with `source: "voice"`.
- **Lane G — site/docs**
  - [ ] Demo videos: voice wake-word, A/B rollback live. `[deferred:hardware+capture]`

**Exit:** Deliberately shipping a broken image triggers auto-rollback within 5 min; voice wake-word demo round-trips in <2 s on rpi5 8GB. **Not yet met** — A/B swap requires hardware (`[deferred:hardware]`); restore needs cloud bucket (`[partial:bucket]`).

### M7 — Plugin model proven (2–3 weeks)

- **Lane A — deputyctl**
  - [x] Hooks system: `pre-message`, `post-message`, `cost-alert`, `update-applied` (Unix-socket relay + dispatcher; `update-applied` and `cost-alert` actively fired).
  - [ ] SLSA L3 attestation verification on update — generator works; verifier is the third party. `[deferred:third-party]`
- **Lane B — image build**
  - [x] Reproducible-build attestations (SLSA L3) published per artefact (`make slsa-attest`, `make sbom` producing in-toto SLSA v1 provenance + CycloneDX 1.5).
- **Lane C — wizard/UX**
  - [ ] Marketplace listing UI in PWA; install community profile end-to-end. `[deferred:M2.5+CDN]`
- **Lane D — release/update**
  - [ ] Public rebuild instructions; third-party reproducer can match SHA256.
- **Lane E — security**
  - [ ] External audit report published; fixes landed; threat model updated. `[deferred:procurement]`
- **Lane F — profiles**
  - [x] **Third profile lands as a manifest + bake recipe (no Rust changes).** Must be in OpenClaw/Hermes class — multi-channel personal assistant with memory + skills. (Khoj — manifest + Ansible bake recipe + AppArmor + systemd unit + stub fallback only.)
- **Lane G — site/docs**
  - [ ] Marketplace contribution flow live; reviewed against profile-class rule.

**Exit:** Third profile shipped without code changes; audit report public; SLSA L3 reproducible by a third party. **Partially met** — third profile (Khoj) shipped without Rust changes; audit + third-party SLSA verification deferred.

### M8 — Accounts + managed backup/restore + integrated tunnel (6–8 weeks) **[PULLED FORWARD to M6]**

Turn deputyOS from donation-ware into a viable business. Users keep full local-first ownership; cloud is an opt-in convenience layer. **Hard rule:** every flow must continue to work without an account; the account purely adds remote access + restore-on-new-device.

The account system, tunnel relay, and backup API live in the **sibling private repo** `deputyos/api-deputyos-com` (`api/src/accounts.rs`, `api/src/tunnel.rs`, `api/src/backup.rs`). The appliance layer (deputyctl, deputywizard, deputypwa) consumes `api.deputyos.com`.

- **Lane A — deputyctl**
  - [x] `deputyctl tunnel --integrated` — WebSocket tunnel client connecting to `api.deputyos.com`. Replaces `cloudflared` dependency. Device reachable at `https://api.deputyos.com/api/v1/tunnel/proxy/<device_id>/` (path-based relay, keyed by device so one account can run many devices — see M9.3).
  - [x] `deputyctl backup now --to-cloud` — quiesced, client-side age-encrypted schema-v3 bundle uploaded to `api.deputyos.com`. Server never sees plaintext or the recovery key.
  - [x] `deputyctl restore --from cloud` — downloads opaque ciphertext, decrypts locally. Pairs with airgap tier (sneaker-net the bundle).
- **Lane B — image build**
  - [x] Account + tunnel + backup API lives in `deputyos/api-deputyos-com` (Axum + SQLite + WebSocket). Deployed at `api.deputyos.com` (not a separate `accounts.deputyos.com` — consolidated into one API).
- **Lane C — wizard/UX**
  - [x] Wizard "Account (free, unlocks remote access + backup)" step. Email + device-code registration against `api.deputyos.com`. Auto-registers device, gets tunnel + backup tokens.
  - [x] Integrated tunnel as recommended remote-access option; Cloudflare Tunnel moves to advanced.
  - [x] PWA tunnel card (`/app/tunnel`): public URL, connection status, copy-to-clipboard.
  - [x] PWA account card (`/app/account`): email, device name, tunnel/backup status.
- **Lane D — release/update**
  - [x] No third-party identity provider — own the auth surface to keep the trust chain auditable.
- **Lane E — security**
  - [x] `documentation/docs/concepts/threat-model-accounts.md` published (retrospective — written after the accounts API shipped; the doc records the Lane E ordering violation honestly).
  - [ ] Pre-launch external audit covering accounts, encryption, R2 isolation. (Folds into M5 audit procurement.)
  - [x] Server stores opaque ciphertext only; per-prefix encryption-at-rest as defence-in-depth.
- **Lane F — profiles**
  - [x] Backup bundle format extends to include profile state (channels' local DBs, secrets-env, hooks, skills) — schema versioned.
- **Lane G — site/docs**
  - [x] Pricing page on `www.deputyos.com/pricing` (free tier: 1 device, 1 GB, 30-day retention, 1 active tunnel).
  - [x] `documentation/docs/how-to/backup-and-restore-cloud.md`.

**Exit:** Set up OpenClaw on device A with backup-to-cloud enabled; flash a fresh image on device B; `deputyctl login` + `deputyctl restore --from cloud` reproduces device A's state byte-for-byte (post-decryption). `deputyctl backup` and full agent operation continue to work on a third device that never logs in. `deputyctl tunnel --integrated` gives every device a public URL (`https://api.deputyos.com/api/v1/tunnel/proxy/<device_id>/`) with zero setup.

**Earliest realistic ship:** v0.5.0 (post-v0.3.0 public preview, post-v0.4.0 audit fixes). **[PULLED FORWARD: account + tunnel infrastructure already scaffolded in api-deputyos-com.]**

### M9 — Desktop console: login + multi-agent fleet + remote management (4–6 weeks)

A proper cross-platform **desktop application** (`deputyos-console`, Tauri v2 + system
webview) that logs into the deputyOS API and runs/manages **multiple local and
cloud agents** from one UI — replacing the single-instance `deputyos-desktop` CLI
launcher driving one browser tab. The agent inside the image/VM is the thing
you tunnel in to and manage; the console is the control plane above it.

**v1 scope (this milestone):** login (device-code) + local multi-instance +
fleet + remote management of self-hosted appliances via the tunnel. **v2**
(app-provisioned cloud VMs on Hetzner) is outlined at the end and stays under
the cloud-provisioning thread, not v1 build scope.

The console lives in this repo (`deputyos-console/`); the tunnel `device_id` fix
+ proxy auth live in the sibling `api-deputyos-com`; the wizard `AccountOwner`
auth mode lives in `deputywizard`.

- **M9.1 — instance registry + per-instance driver (`deputyos-desktop`)** **[DONE]**
  - [x] `Instance { id, name, target, profile, wizard_port, gateway_port, cache_dir, runtime_dir, manifest_url, channel, created_at, last_status }` + `Registry` persisted at `config::data_dir()/instances.json` (env-overridable via `DEPUTYOS_DESKTOP_DATA_DIR`).
  - [x] `Driver::*_with(&InstanceConfig)` (default impls delegate to the single-instance path; only `drivers/linux.rs` overrides) — multiple named instances with distinct ports/cache dirs/pid files coexist on one host. Existing single-instance CLI path unchanged (`start`/`stop`/`status` delegate via `from_env()`).
  - [x] `allocate_port_pair` (bind `127.0.0.1:0` twice, 7000-series host loop). `unsafe_code = "forbid"` preserved (`/bin/kill`, no `libc::kill`).
- **M9.2 — `deputyos-console` crate: login + local dashboard** **[DONE]**
  - [x] `api_client` (ureq): device-code login, token refresh/revoke, fleet (list/register/revoke devices), device_id-keyed tunnel proxy URL builder. httpmock unit tests.
  - [x] `store`: `TokenStore` trait — 0600 JSON `FileTokenStore` (always) + OS-keychain `KeyringStore` (`gui` feature). Auto-refresh within skew of expiry.
  - [x] `instance_ops`: lifecycle wrapper over the registry + `Driver::*_with` (create/start/stop/status/install/delete).
  - [x] Tauri v2 command surface + `GuiState` (api client + ops + token store, keyring→file fallback). GUI gated behind a `gui` feature so the testable core builds without webview deps.
  - [x] Hand-rolled UI (`src/ui/`, no framework/CDN/build step — matches the embedded wizard webviews). `make console` builds the GUI (needs `libwebkit2gtk-4.1-dev libgtk-3-dev librsvg2-dev` on Linux).
- **M9.3 — tunnel `device_id` fix + proxy auth + remote wizard `AccountOwner`** **[DONE]**
  - [x] Sibling `api/src/tunnel.rs`: `handle_tunnel` keys the live WS channel by `device_id` (was `account_id`) — multiple devices per account coexist; only a reconnect of the same device evicts itself.
  - [x] `proxy()` path keyed by `<device_id>`, resolves owner via `SELECT account_id FROM devices WHERE id=? AND revoked_at IS NULL`, requires a valid account JWT (Bearer or `?token=`) whose `sub` owns the device — non-owner 403, no credential 401, unknown/revoked device 404. No back-compat shim (console is the only caller).
  - [x] `deputywizard/src/auth.rs`: `AuthMode::AccountOwner` — validates the account owner's JWT against the API's RSA256 pubkey embedded at `/etc/deputyos/api-pubkey.pem`, checks `jwt.sub == account_id` (read from `/etc/deputyos/account.json`, now storing `account_id`), issues a session cookie. The JWT is stateless/short-lived so it is not consumed. `--account-owner` flag; falls back to Token mode if the pubkey/account.json is missing (pre-first-boot). This is the load-bearing fix: the in-VM launch token never leaves the appliance, so the console's JWT is the credential that lets the owner tunnel in.
  - [x] Tests: sibling `tunnel_proxy_{requires_account_credential,rejects_non_owner_and_accepts_owner,unknown_device_is_not_found,accepts_token_query_param}`; wizard 5 `account_owner_*` auth tests (valid JWT, wrong account, other-key signature, missing credential, garbage).
- **M9.4 — deputyctl command-poller service** **[DONE]**
  - [x] `deputyctl/src/commands.rs` (`deputyctl commands poll [--loop] [--interval N] [--api-base URL]`): device-side poller draining the sibling API's async command queue. Reads `device_id` from `/etc/deputyos/account.json` + `backup_token` from `/etc/deputyos/backup-token` (both env-overridable for hermetic tests), polls `GET /api/v1/devices/:id/commands/pending` (Bearer=backup_token), executes each, acks `POST /api/v1/devices/:id/commands/:cmd_id/result` with `{result, status}`. **Allow-listed execution** — `ping` (no-op ack) + `restart-agent` (restarts the active profile's unit via the same `profile::load_active`+`systemd::run` path as `deputyctl restart`); any other command is acked `unsupported` **without executing**, so a malicious/bad enqueue can never run arbitrary code. The `Executor` trait lets tests stub execution; 11 httpmock unit tests cover fetch/ack transport, Bearer auth, drain-once, 401→error, empty-queue, identity load + the allow-list semantics. Without this, the server-side queue is dead-letter.
  - [x] `deputyos-command-poller.service` systemd unit (Ansible `wizard-baseline.yml`): `ExecStart=/usr/local/bin/deputyctl commands poll --loop --interval 30`, runs as root (backup-token is 0600 root-owned), `Restart=always`, `ConditionPathExists=/etc/deputyos/backup-token` so it's a no-op on first boot and activates on the first boot after registration. Enabled by the role.
- **M9.5 — Windows/macOS local-agent driver** **[DONE]**
  - [x] `drivers/windows.rs` (WSL2) + `drivers/macos.rs` (UTM) key the WSL distro name / UTM VM name per instance and implement typed resident-agent execution. Windows provides cooperative pause/resume (quiesce + reclaim + terminate + resumable marker) and distinct `netsh portproxy` mappings. macOS uses UTM saved suspend/resume and QEMU Guest Agent-reported shared-VLAN IPs, so multiple VMs remain locally addressable without pretending `utmctl` offers arbitrary localhost remaps. Linux additionally exposes a true per-instance virtio balloon; WSL memory remains utility-VM-global and UTM has no supported live target. Pure command/script builders and protocol parsing are unit-tested. Windows and macOS still require real-hardware smoke tests.
- **M9.6 — appliance AccountOwner service orchestration** **[DONE]**
  - [x] Bake the API RSA public key to `/etc/deputyos/api-pubkey.pem` (0644 — public, non-secret). Staged by the build pipeline from the API's signing keypair (`deputyos_api_pubkey_path`, default `{{ staging }}/api-pubkey.pem`); the role task is a no-op with a clear warn if it isn't staged, so the wizard's `--account-owner` falls back to Token mode (remote management unavailable until provisioned). The crypto + CLI flag + `account.json` `account_id` plumbing all landed in M9.3; this is the bake-side distribution.
  - [x] `deputyos-remote-wizard.service.j2` systemd unit — the post-first-boot counterpart of `deputywizard.service`. Gated on the **inverse** condition (`ConditionPathExists=/var/lib/deputyos/wizard-state.json`), so it activates only after first boot completes and stays up permanently: `ExecStart=deputywizard serve --port 8088 --bind 0.0.0.0 --account-owner --production`, runs as root with the same hardening as the first-boot unit, `Restart=always`. The two wizard units are mutually exclusive by condition, so only one binds :8088 at a time. Enabled by `wizard-baseline.yml`; the account-owner JWT (validated against the baked pubkey + matched to `account.json` `account_id`) is the credential that lets the console tunnel in. Follow-up (not blocking): the wizard's apply step could `systemctl start deputyos-remote-wizard` + stop the first-boot unit the moment registration completes, so remote management is live without waiting for the next reboot.

- **M9.7 — object-native desktop/deputy control plane** **[IN PROGRESS]**
  - [x] Ratify the hard data boundary: PostgreSQL is limited to users,
    enterprises, memberships, payments, subscriptions and entitlements.
  - [x] Define the sharded immutable-segment + numbered-manifest inventory
    schema in `docs/schemas/deputy-inventory-v1.json`.
  - [ ] Replace the eager `instances.json` registry with a paged local
    inventory cache and defined/materialised/active lifecycle.
  - [ ] Register one desktop connection and multiplex deputy commands, native
    WebUIs and terminal streams by `deputy_id`.
  - [ ] Move durable desktop registration, credential hashes, fleet snapshots,
    cost rollups, cloud runtime requests and tunnel ownership out of relational
    tables and into object manifests/events.
  - [ ] Remove relational fallback reads after migration verification and the
    rollback window.
  - [ ] Add scale tests proving control-plane work is proportional to active
    desktops/streams and batch size, never total deputy cardinality.

**Platform validation gate.** Local-agent drivers are implemented for
Linux/qemu, Windows/WSL2, and macOS/UTM. Linux is proven end-to-end; Windows
and macOS compile and have unit-tested command builders but still require real
hardware smoke tests. Remote management works on every platform the Tauri app
builds for. `cargo fmt --check && cargo clippy --workspace --all-targets -- -D
warnings && cargo test --workspace` must remain green.

**Exit (v1):** log into the deputyOS API from the console (device-code); create
two local instances, install + start both, open each wizard in its own
webview, confirm both reachable on their assigned ports, stop one and confirm
the other is unaffected; register a device on a remote appliance, see it ONLINE
in the fleet, click Open → the wizard loads through the tunnel proxy with the
AccountOwner JWT flow; stop the VM → fleet shows OFFLINE.

#### v2 — app-provisioned cloud agents (outline, not v1 build scope)

Server-side `api-deputyos-com/api/src/cloud.rs` (today `request_instance()`
only writes a `cloud_instances` row `status='requested'`, no Hetzner call): an
`orchestrator.rs` (release builds) picks the event → Hetzner `POST /servers`
with cloud-init that installs deputyos (signed-artefact flow), writes
`/etc/deputyos/{tunnel,backup}-token` (from `register_device`), enables
`deputyos-tunnel.service` + the command-poller service (M9.4) → polls
`GET /api/v1/tunnel/proxy/<device_id>/healthz` until the device checks in →
updates `status='running'`, `server_id`. Console "Create cloud agent" →
`POST /cloud/instances` → poll `list_fleet` until ONLINE → `open_remote_wizard`.
**No console-side changes beyond a button** — the v1 remote machinery is reused
verbatim. Stays under the cloud-provisioning thread, not M9 v1 build scope.

## Ongoing after M7

- Upstream tracker green continuously; weekly digest of upstream-release → image-rev latency.
- Security CVE feed → patch images within 7 days of disclosure.
- OpenTelemetry traces (opt-in) for users who want observability.
- Community-contributed profiles vetted against the profile-class rule.

## Out-of-sandbox infra (user-handled, not code-tracked)

Roadmap items above are code-doable from a contributor's laptop. These items
genuinely require external infrastructure / procurement / manual effort:

| Item | Required for | Blocking |
|---|---|---|
| B2 bucket credentials + `cdn.deputyos.com` Cloudflare route | M4 publication and launcher downloads | Code and URLs are wired; production CI still requires the repository secrets and a successful signed publication. |
| Apple Developer ID + notarization workflow | M2.5 macOS launcher | Smooth Mac UX — without it, Gatekeeper warns. |
| Windows EV cert | M2.5 Windows launcher | Smooth Win UX — without it, SmartScreen warns. |
| GitHub-hosted runners cross-compiling on macos-latest + windows-latest | M2.5 launcher cross-compile, M4 release pipeline | All 5 launcher binaries per release. |
| 24h Pi 5 hardware soak | M1 exit gate | Validates baked OpenClaw on real hardware unattended. |
| Two independent build hosts | M4 reproducible-build verification | Validates SLSA L3 byte-identical reproduction. |
| External audit vendor procurement | M5 audit | Audit report publication. |
| DO Marketplace submission | M5 marketplace listing | Discoverability for cloud users. |
| `security@deputyos.com` inbox | M5 disclosure SLA | Coordinated disclosure path. |
| Forum / Matrix room hosting | M5 community | Public Q&A surface. |

These are independent. Each can be unblocked separately.
