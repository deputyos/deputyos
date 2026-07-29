# 14 — Limitations (the awareness map)

A semi-technical user who hits an unexplained failure thinks the project is broken. A user told upfront "Pi 4 4GB cannot run a local LLM, here's why" feels respected and trusts every subsequent recommendation. **Trust scales; surprise does not.** This doc is the canonical map of what deputyOS cannot do, why, and where the user finds out.

## Operating principles

1. **No silent degradation.** Every refusal includes a reason in the same UI surface where the user clicked.
2. **Headroom is shown before it's spent.** Wizard calculates RAM/disk/cost cost of each toggle and shows it inline.
3. **Refuse with a fix.** Every "won't work" message includes "what would unblock it" — a hardware suggestion, a different channel, a smaller model.
4. **Documentation is a backup, not the primary surface.** Users find limitations in the picker, wizard, `deputyctl`, and PWA *first*; this doc and the troubleshooting one are for verification.

## Where awareness is surfaced

| Touchpoint | What it shows | Implementation milestone |
|---|---|---|
| **Picker page** ([deputyos.com](https://www.deputyos.com)) | Per-target capability badges and dis-able reasons before download | M2 |
| **`deputyos.yaml` pre-flash** | Comments explain limits relevant to chosen target | M2 |
| **Wizard (web on `:8088`)** | Live headroom calculation; confirms before enabling tight combos | M3 |
| **`deputyctl init` (TTY)** | Same as web wizard | M3 |
| **`deputyctl limits`** (new command) | Enumerates everything this device cannot do, with reasons | M3 |
| **`deputyctl doctor`** | Reports current pressure / approaching limits | M1 |
| **PWA dashboard "Your device" card** | Permanent visible summary of capabilities and active warnings | M3 |
| **Channel toggle** | Disabling a channel prints budget impact; enabling shows cost | M3 |
| **Update flow** | Refuses if new image rev wouldn't fit on this disk | M4 |
| **Provider switch** | Warns if new model has larger context needs than RAM allows for local cache | M4 |
| **Backup setup** | Confirms quota math and retention impact | M5 |
| **Docs (this file + 10-troubleshooting + 12-bundled-software + 13-memory-pressure)** | Reference; cross-linked from every UI message | M0 (now) |

## The `deputyctl limits` command (spec)

```
$ deputyctl limits
Device:   Raspberry Pi 4 (4 GB RAM, SD card storage)
Image:    deputyos-openclaw-rpi4-2026.04.27-stable

What this device CAN do:
  ✓ Run OpenClaw or Hermes against any cloud model provider
  ✓ Telegram, Slack, Discord, Matrix, IRC, Email channels
  ✓ Browser automation via Camoufox (one tab, ~500 MB peak)
  ✓ OCR via Tesseract (English)
  ✓ Daily on-demand virus scan via Magika + clamscan
  ✓ Backups to Backblaze B2 / Cloudflare R2

What this device CANNOT do (and why):
  ✗ Run a local LLM
       Reason:  insufficient RAM. Llama-3.2-1B Q4 needs ~1 GB working
                set; Pi 4 4GB has ~1.5 GB headroom after agent + browser.
       Unblock: upgrade to Pi 4 8GB, Pi 5 8GB+, or Oracle ARM Free 24GB.

  ✗ Voice input/output (whisper.cpp + Piper)
       Reason:  thermal + CPU budget would block other components.
       Unblock: Pi 5 8GB+ or x86 mini-PC.

  ✗ Run ClamAV daemon (clamd) persistently
       Reason:  clamd holds ~900 MB RSS. Replaced by on-demand clamscan
                triggered by Magika hints; daily full scan still runs.
       Unblock: same coverage on this device; persistent on Pi 4 8GB+.

  ✗ Enable both WhatsApp Cloud webhook AND Discord with voice
       Reason:  combined RSS exceeds memory budget.
       Unblock: pick one, or upgrade RAM.

Airgap support: yes (max tier: lean)
  build with: make build TARGET=rpi4 TIER=lean AIRGAP=1

Connected drives + shares:
  (none configured — see `deputyctl mounts add --help`)

Network egress: open

Currently active warnings:
  (none)

Run `deputyctl doctor --memory` for live pressure metrics.
```

Output is colour-coded; non-zero exit if any *active* limit is being violated (rare; mostly informational).

## The PWA "Your device" card

Always-visible card on the dashboard:

```
┌── Your device ──────────────────────────────────┐
│ Pi 4 (4 GB RAM, SD card)                        │
│                                                 │
│ Active:  OpenClaw + Telegram + Slack + Camoufox │
│ Memory:  62% (zram 2.1×, no disk swap-in)       │
│                                                 │
│ Not available on this device:                   │
│   • Local LLM (needs 8GB+)                      │
│   • Voice (needs Pi 5 / x86)                    │
│                                                 │
│ Tip: USB SSD would unlock NVMe-fast updates.    │
└─────────────────────────────────────────────────┘
```

Tapping any line opens the relevant section of `docs/12-bundled-software.md` or `docs/13-memory-pressure.md`.

## Limitations by hardware target

### Pi 4 (4 GB)
- No local LLM (RAM).
- No voice (CPU + RAM).
- No persistent `clamd`; on-demand `clamscan` only.
- Heavy channels (WhatsApp Cloud, Discord+voice) default off; opt-on with warning.
- SD-only boot is supported but slow under update or backup load; USB SSD recommended.
- Browser is single-tab, idle-reaped after 60 s.

### Pi 4 (8 GB)
- No local LLM (CPU still too slow for usable interactive UX even though RAM fits).
- Voice limited to Whisper `tiny.en`; spoken interaction noticeably less accurate than Pi 5.
- Active cooling required for sustained browser sessions.

### Pi 5 (8 GB)
- Local LLM limited to 1B Q4 (Llama-3.2-1B, Qwen2.5-1.5B); 3B will swap heavily.
- Voice up to Whisper `base.en`; `small.en` thermal-throttles.
- mmap'd LLM models on SD card thrash; NVMe via PCIe HAT or USB3 SSD strongly recommended.

### Pi 5 (16 GB)
- Voice up to `base.en`; `small.en` still thermal-throttles. (We could ship `small.en` for x86; we don't for rpi5.)
- No verified boot; bootloader does not support secure boot in the Coreboot/UEFI sense.
- No TPM by default; `--tpm-seal` flag for secrets is unavailable on this hardware.

### `arm64-generic` (Radxa, Orange Pi, Khadas, Le Potato, …)
- Best-effort. Per-board quirks the user inherits: device-tree overlay, audio routing, video output, thermal envelope.
- The wizard asks for the user's board's overlay; if unrecognised, voice and audio-related features are disabled until the user provides the correct overlay.
- Hailo / Coral / OpenVINO not bundled; user can sideload runtimes.

### `x86_64-mini-pc` (Beelink, MeLE, NUC class)
- Voice up to Whisper `small.en` (thermal headroom permits).
- 3B local LLM at workable speed; 7B is feasible but unsupported (no quality SLA from us).
- iGPU acceleration via OpenVINO; no NVIDIA / AMD discrete-GPU tooling unless the box has one (custom build needed).

### `wsl2`
- **No audio.** WSL2 by default does not pass `/dev/snd`. Voice features are disabled.
- mDNS does not advertise outside the WSL2 internal network. `deputyos.local` works on the same Windows host but not from a phone on the LAN; the PWA is reachable at `localhost:8088` from Windows only.
- systemd-on-WSL must be enabled; if it isn't, the wizard explains how.
- WSL dynamically manages memory for one shared utility VM. deputyOS can quiesce, reclaim, and cooperatively suspend each distro, but cannot set an isolated live balloon target for one distro; `.wslconfig` memory remains global.
- Updates do not A/B; user re-imports the new tarball.

### `macos-qemu`
- Demo-only target. Voice disabled, NPU disabled, performance throttled by qemu emulation.
- UTM can saved-suspend an instance and its QEMU Guest Agent can execute the typed resident-agent protocol, but UTM does not expose a supported live memory-balloon target.
- Multiple instances use their distinct shared-network guest IPs for local access. External access uses each image's authenticated outbound tunnel; there is no arbitrary localhost port remap in `utmctl`.
- A/B not available; rollback is host snapshot.

### `digitalocean`
- No audio; voice features disabled.
- A/B not available; uses DO snapshot rollback instead.
- Bandwidth costs apply on DO's side — unrelated to our B2 egress savings.

### `oracle-arm-free` (24 GB Ampere)
- "Always Free" tier may be reclaimed by Oracle if idle for ~7 days (per their published policy at time of writing). deputyOS includes a tiny keepalive that touches a benign endpoint every 4 days; this is opt-in and disclosed in the wizard.
- 4 OCPUs is fine for 3B local LLM but slower than rpi5 for short tasks; comparable on long ones.
- No audio; voice disabled.

### `hetzner` / `vultr` / `linode` (cloud-init recipe)
- We don't ship signed images for these; we publish a `cloud-init` userdata recipe and a verified Ubuntu 24.04 base SHA. The user reads the recipe before applying.
- Updates are full re-image cycles (delete + redeploy with new manifest version). Less ergonomic than DO 1-Click.

### `fly-machines`
- Ephemeral by default. The data partition is a Fly volume; if the volume is deleted, agent state is lost (backups still in user's bucket).
- Voice disabled.
- Cold-start latency on free Machines is several seconds; not ideal for low-latency voice channels.

### `fly-machines+gpu`
- Paid tier; running cost is non-trivial.
- vLLM and CUDA only; no AMD ROCm support on Fly today.

### `proxmox` / `unraid` / `truenas` (community templates)
- Templates wrap our qcow2; we don't actively maintain platform-specific quirks (USB passthrough, NIC bonding, ZFS dataset tunings). Users follow our `docs/01-getting-started.md` for the qcow2 path.

## Limitations by feature

### Model providers
- **No OAuth flows** for any provider in the wizard, except the optional Cloudflare-bucket-provisioning OAuth (which is for storage, not model access). [ADR-0006](adr/0006-no-oauth-in-wizard.md). The user always gets, holds, and rotates an API key.
- **Cost guardrails depend on the provider exposing per-call usage.** OpenRouter, Anthropic, OpenAI report this; some smaller providers don't. We estimate from a cached rate table for those, accuracy ±20%.
- **AWS Bedrock** is the only multi-field provider; user must paste AWS access key + secret + region.
- **Local Ollama** ships only on offline-build images; default builds skip it because Pi-class inference is slow and sets unrealistic UX expectations.

### Channels
- **WhatsApp Cloud API** requires Meta Business verification — out of our hands; documented in the wizard.
- **iMessage** requires either an Apple device on the same iCloud account running BlueBubbles or an Apple-provided business account. Not a self-contained on-Pi feature.
- **Discord with voice** is RAM-heavy; not enabled by default on Pi 4.
- **Some regional channels** (Zalo, WeChat, Feishu, QQ) may have country-restrictions or registration steps the user handles outside deputyOS.
- **WebRTC voice over Telegram/WhatsApp** is not bundled; voice is on-device wake-word only at M6.

### Browser automation
- **Camoufox is Firefox-based.** A small fraction of sites use Chrome-specific rendering or anti-Firefox heuristics; for those, only the `x86_64-mini-pc` build (which carries `chromium-headless-shell`) succeeds. Documented per-channel.
- **Browserbase paid path** for Hermes is supported but not required; we default to local Camoufox.

### Voice
- **English-only by default.** Other Whisper languages and Piper voices are sideloadable via `deputyctl voice add-language <code>` (M6+) — they're not in the base image because per-language assets add ~25 MB each.
- **Wake-word** is `hey-molty` (OpenClaw) or `hey-hermes` (Hermes). Custom wake-words require user-supplied training data.
- **No noise cancellation** beyond what the audio stack provides; far-field voice in noisy rooms degrades.

### Local LLMs
- **Quality gap with Claude/GPT-4-class models is wide.** We never make local LLM the default chat model on Pi-class hardware. It's a fallback when cloud is unreachable, plus an embedding/reranking helper.
- **Context length** is pinned at 4096 on rpi5-8gb (1B model) and 8192 on rpi5-16gb (3B model). Longer contexts blow the KV cache budget; surfaced as a wizard knob.
- **No vision/multimodal locally** until LLaVA-class models are practical at Pi-class speeds. Cloud multimodal models work fine.

### Security
- **AppArmor confines, doesn't isolate at hardware level.** Kernel CVEs that bypass AppArmor exist; we patch them via image revs (M4) within 7 days of disclosure but the window is non-zero.
- **ClamAV is signature-based.** Zero-day malware not in signatures isn't caught. Magika catches some content-type spoofing but is also heuristic.
- **No verified boot on Pi.** Pi firmware does not implement secure boot in the Coreboot/UEFI sense. A user with physical access can swap the SD/NVMe.
- **No full-disk encryption by default.** Opt-in at M6+ via wizard; default is unencrypted because a stolen Pi with default credentials is a small attack value-target.
- **Cost guardrails are hints, not hard guarantees.** A misbehaving channel that bursts before the cap-check runs may overshoot by tens of cents to a few dollars; we cap aggressively but cannot promise zero overshoot.

### Updates
- **Full image swap, no deltas.** A 1.5–4 GB download per update. We accept this for the [ADR-0002](adr/0002-zero-first-boot-network-installs.md) invariant.
- **A/B partition rollback only on hardware that supports it.** Cloud targets use snapshot rollback (DO) or re-deploy (Hetzner/Vultr/Linode/Fly).
- **Update applied while charging-thin power supply** can brick a Pi mid-write. We surface the warning; we cannot detect undervoltage in advance reliably enough to refuse.
- **First-time update across a manifest schema bump** prompts the user explicitly and may require a one-time re-flash if they skip multiple major image revs. (This shouldn't happen often; it's been called out as a possibility.)

### Backups
- **Whole-snapshot restore only.** No granular "restore just one conversation" — that would couple us to each profile's data schema. Hermes' FTS5 makes selective recovery possible but unsupported.
- **Pre-update snapshots are best-effort.** If the network is down, the snapshot is skipped and the update proceeds (configurable to "refuse update if backup fails").
- **B2 / R2 outage** means no backups happen until restored. We do not run a local fallback.

### Networking
- **mDNS may be blocked** on enterprise / locked-down Wi-Fi. Wizard prints IP-form URL on HDMI as fallback.
- **Inbound webhook channels need Cloudflare Tunnel or Tailscale Funnel** when behind NAT/CGNAT. Outbound-only channels (Telegram, Discord, Slack RTM, Matrix client) work without.
- **IPv6-only LANs** untested per channel; Telegram/Slack work, others vary.

### Drive mounting (M3.5)
- **Every mount lives under `/mnt/deputyos/`** so AppArmor's per-profile rules can confine it. Paths outside this prefix are refused.
- **Removable auto-mount is opt-in.** Default policy ships `removable.enabled=false`. Plugging a USB stick on a fresh image does nothing until the user (or wizard) flips the policy.
- **LUKS / encrypted volumes are refused** until the user unlocks them manually via `cryptsetup luksOpen`. The agent never holds disk keys.
- **Unknown filesystems get `nosuid,nodev,noexec`** mount options regardless of mode.
- **Network shares (SMB / NFS) store credentials in `/etc/deputyos/secrets.env`** mode 0600 — never in the policy file. Backups of the policy are therefore safe to share.
- **`deputyctl mounts` and the PWA "Mounts" card surface every active mount** with a one-click revoke. No mount is invisible.

See the [how-to: mount drives](../documentation/docs/how-to/operate/mount-drives.md) for the operator walkthrough and the [mounts-policy schema](../documentation/docs/reference/schemas/mounts-policy.md) for the policy file shape.

### Air-gapped tier (M4.5)
- **`AIRGAP=1` is per-target gated.** `roles/deputyos/files/limits.<target>.json` carries `airgap_supported` and `airgap_max_tier`. The picker, wizard, and `deputyctl limits` refuse air-gapped builds where the target can't run any LFM2 tier (e.g. `fly-machines`).
- **rpi4 + airgap is lean-only.** Cortex-A72 cannot run LFM2-1.2B at usable t/s; the bake refuses `TIER>lean` with a clear reason.
- **No on-device package upgrades.** apt sources point at the baked file:// mirror; security patches arrive via signed image rebuilds, not in-place updates.
- **Cloud-API LLM providers are unreachable.** Wizard hides them; provider-key fields disappear. The baked LFM2 (or any GGUF the user `deputyctl model register`s later) is the only path.
- **Egress posture is reversible.** `deputyctl network unlock` flips `mode=airgap` → `mode=open` and reloads nftables. The middle ground (`mode=whitelist`) lights up in M5.5.

### Hardware accelerators
- **Hailo runtime** ships only on `rpi5+hailo` build variant.
- **Coral runtime** ships only on `rpi5+coral` build variant.
- **OpenVINO** ships only on `x86_64-mini-pc`.
- Switching to a different accelerator at runtime is not supported; pick the right build at flash time.

## Limitations by version

`/etc/deputyos/build.json` records what *this* image supports. `deputyctl version` prints it. Deltas relative to current capability set:

- **Image revs before M3 (current → M3)** lack the web wizard, QR provisioning, and built-in private web chat. Limitations: no first-boot UX without SSH; manual `deputyctl init` on TTY only.
- **Image revs before M4** lack `deputyctl update`. Limitations: re-flash to update.
- **Image revs before M5** lack cost guardrails, quiet hours, factory reset.
- **Image revs before M6** lack on-device voice, A/B watchdog auto-rollback, full-disk-encryption opt-in.
- **Image revs before M7** lack SLSA L3 attestations; reproducibility is best-effort, not third-party-verified.

The picker page filters to the latest stable manifest by default; users who pin to an older version see the limitations of that version inline.

## What we won't tell users (and why)

These are **not** limitations users need to think about; we mention them for contributor clarity:

- We don't surface "AppArmor is in enforce mode" to end users — they expect security to be on, not advertised as a feature.
- We don't show bytes-per-channel cost estimates — too noisy; the calculated headroom in MB / channel is enough.
- We don't surface CPU governor, sysctl tuning, BBR, etc. — these are infrastructure facts, not user choices.

Awareness ≠ overload. Surface what affects user decisions; don't drown the user in implementation detail.

## Cross-references

- [12 — Bundled software](12-bundled-software.md) — what's in each image
- [13 — Memory pressure](13-memory-pressure.md) — why limits are sized as they are
- [10 — Troubleshooting](10-troubleshooting.md) — what to do when a limit bites
- [05 — Model providers](05-model-providers.md) — provider-specific limits
- [06 — Storage and backup](06-storage-and-backup.md) — backup/restore boundaries
- [07 — Networking](07-networking.md) — channel exposure constraints
- [09 — Security](09-security.md) — what we don't try to defend against
