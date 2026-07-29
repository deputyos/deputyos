# 12 — Bundled software (deep analysis per target)

This doc enumerates every piece of software we *could* bake into the appliance image, scores it on cost vs benefit per hardware target, and pins down a per-target inclusion matrix. The goal is a defensible answer to "why does my Pi 5 16GB image weigh 3.5 GB and my DigitalOcean snapshot weigh 1.3 GB" and "why don't I have a local LLM on my Pi 4".

## 1. Framing

Two pressures pull in opposite directions:

**Bigger images mean a richer first-boot experience.** A user who flashes the image and immediately can do voice in/out, browse a webpage, OCR a screenshot, run a small local LLM — that user is delighted in the first 10 minutes. They don't have to figure out which extra packages to add.

**Bigger images cost more to flash, store, and update.** Every GB compressed is roughly 5–10 minutes of `dd` to an SD card, a noticeable B2 storage line item, and longer A/B update downloads. On bandwidth-constrained connections (cellular, hotel WiFi during travel) a 4 GB update is annoying; an 8 GB update is hostile.

The way out is **per-target tiers**: ship what's *useful and performant* on a given target, and nothing else. A Pi 4 4GB has no business carrying a local LLM model file — it can't run it usefully. A 24 GB Oracle ARM Free instance has plenty of room for Llama-3.2-3B and the user benefits from offline-capable answers. Same Ansible role, different `when:` gates.

Hard constraint that overrides everything else: **none of this software is fetched at first boot.** Per [ADR-0002](adr/0002-zero-first-boot-network-installs.md), every byte that's "in" must be in the slot image at bake time.

## 2. Performance and size budgets per target

| Target | RAM | Storage floor | CPU class | Compressed image budget | What "performant" looks like |
|---|---|---|---|---|---|
| `rpi4-4gb` | 4 GB | 8 GB SD | Cortex-A72 4× | ≤ 1.2 GB | Cloud-LLM agent only; light tools; no voice; no local model |
| `rpi4-8gb` | 8 GB | 16 GB SD | Cortex-A72 4× | ≤ 1.6 GB | Cloud-LLM agent + browser + OCR; voice optional and constrained |
| `rpi5-8gb` | 8 GB | 32 GB SD/NVMe | Cortex-A76 4× | ≤ 2.4 GB | Cloud-LLM agent + voice (tiny.en) + browser + light local 1B |
| `rpi5-16gb` | 16 GB | 32 GB NVMe | Cortex-A76 4× | ≤ 3.6 GB | Cloud-LLM agent + voice (base.en) + browser + 3B local |
| `rpi5+hailo` | 16 GB | 32 GB NVMe | + Hailo-8L NPU | ≤ 3.8 GB | Above + Hailo-accelerated whisper / image models |
| `rpi5+coral` | 16 GB | 32 GB NVMe | + Edge TPU | ≤ 3.8 GB | Above + Coral-accelerated small models |
| `arm64-generic` | varies | 16 GB | varies | ≤ 1.6 GB | Conservative defaults; user opts into voice/LLM via post-flash add-ons |
| `x86_64-mini-pc` | 8–32 GB | 64 GB+ | x86-64-v3 + iGPU | ≤ 4.0 GB | Voice (small.en) + browser + 3B local + OpenVINO iGPU accel |
| `wsl2` | host RAM | host disk | host CPU | ≤ 1.2 GB | Lean — host already has plenty; cloud-LLM oriented |
| `macos-qemu` | host alloc | host disk | qemu emul. | ≤ 1.0 GB | Lean — demo-only; cloud-LLM only |
| `digitalocean` | 1–32 GB | 25–500 GB | Intel x86-64 | ≤ 1.4 GB | Cloud-LLM agent + browser + OCR; no voice (no audio) |
| `oracle-arm-free` | 24 GB | 200 GB | Ampere arm64 4× | ≤ 3.6 GB | Big-RAM cloud → 3B local + browser + OCR |
| `hetzner` / `vultr` / `linode` | varies | varies | varies | recipe only | Same defaults as DigitalOcean |
| `fly-machines` | varies | small | varies | ≤ 1.0 GB OCI | OCI-tight; cloud-LLM only |
| `fly-machines+gpu` | varies | varies | NVIDIA L40S | ≤ 6 GB OCI | + vLLM + CUDA |
| `proxmox` / `unraid` / `truenas` | varies | varies | varies | reuse `macos-qemu` qcow2 | Lean base; user grows it |

These budgets aren't theoretical — every line item below maps to an actual size cost.

## 3. Software categories — what they are, what they cost, what they buy us

### 3.1 Always-on core (every target)

| Item | Compressed cost | Why |
|---|---|---|
| Base OS + kernel + userspace | ~500 MB | Necessary. |
| Node 24 + npm offline cache for active profile | ~150 MB | OpenClaw runtime. |
| Python 3.11 + uv + wheel cache for active profile | ~120 MB | Hermes runtime. Cache is profile-specific so we ship one runtime fully populated, the other tooling-only. |
| Pinned agent code | 100–300 MB | OpenClaw is heavier than Hermes; varies. |
| `deputyctl` Rust binary | ~10 MB | Single-binary management surface. |
| ClamAV signature DB + clamscan binary | ~250 MB | Per [ADR-0008](adr/0008-clamav-plus-magika-baseline.md). The `clamd` daemon runs only on RAM ≥ 8 GB targets; smaller targets use on-demand `clamscan` invoked by Magika hints. See [13-memory-pressure.md §4](13-memory-pressure.md). |
| Magika model + Rust CLI | ~10 MB | Per [ADR-0008](adr/0008-clamav-plus-magika-baseline.md). |
| AppArmor, ufw, fail2ban, auditd | ~10 MB | Security baseline. |
| Bubblewrap (`bwrap`) | ~1 MB | Tool-execution sandbox; lighter than firejail; exactly fits Hermes' unprivileged-userns sandbox model. |
| Caddy 2.x | ~25 MB | Auto-TLS reverse proxy fronting wizard, `/chat`, and the PWA. Lets us terminate TLS on `deputyos.local` with a self-signed cert (or with Let's Encrypt when the user wires Cloudflare Tunnel). |
| rclone | ~50 MB | Backup transport (B2 + R2 + S3). |
| restic | ~25 MB | Alternative incremental backup store; opt-in via wizard. |
| minisign + cosign + age | ~30 MB | Signature verification + at-rest encryption helpers. |
| The CLI utility belt | ~80 MB | See §3.2. |

**Subtotal core: ~1.0 GB compressed.** Floor for every target.

### 3.2 The CLI utility belt (every target)

Pre-installed tools the agents will reach for. None of these alone is large; together they remove the "agent decided to try a tool that isn't installed" failure mode.

`jq`, `yq`, `curl`, `wget`, `git`, `ripgrep`, `fd-find`, `fzf`, `bat`, `sqlite3`, `ffmpeg`, `imagemagick`, `libvips-tools`, `poppler-utils`, `pandoc`, `7zip`, `unzip`, `unrar-free`, `bc`, `tree`, `htop`, `btop`, `iotop`, `iftop`, `nethogs`, `ncdu`, `iputils-ping`, `traceroute`, `mtr`, `dnsutils`, `tcpdump`, `tmux`, `mosh-server`, `sox`, `yt-dlp`, `dasel`, `xidel`, `shellcheck`.

**~80 MB compressed.** Excellent ROI; agents call into these constantly.

### 3.3 Browser automation tier

| Item | Compressed cost | Notes |
|---|---|---|
| **Camoufox** (Firefox fork w/ engine-level fingerprint spoofing) | ~150 MB | ~40 MB idle RAM, ~200 MB peak. Drop-in Playwright-compatible. Recommended over full Chromium because it's smaller and more anti-detection-friendly for the agents' real-world tasks. |
| Playwright Python + Node bindings | ~30 MB | Both agents use these abstractions. |
| `chromium-headless-shell` (smaller than full Chromium) | ~140 MB | Optional alternate; ships only on `x86_64-mini-pc` where some sites require Chrome's exact rendering. |

**Tier subtotal:** Camoufox-only ≈ 180 MB; Camoufox + chromium-headless-shell ≈ 320 MB.

This tier matters because both OpenClaw and Hermes have non-trivial browser-automation features. Without local browser tooling, those features fall back to paid services (Browserbase) — a UX cliff for self-hosted users.

### 3.4 Voice tier (STT / TTS / wake word)

| Item | Compressed cost | Notes |
|---|---|---|
| `whisper.cpp` (Pi-tuned arm64 build) | ~5 MB | Engine. |
| Whisper `tiny.en` model | ~40 MB | Real-time on Pi 4 (tight) and Pi 5; modest accuracy; English-only. |
| Whisper `base.en` model | ~80 MB | Real-time on Pi 5 with active cooling; viable daily; English-only. |
| Whisper `small.en` model | ~250 MB | Real-time on x86 mini-PC and Oracle ARM 24G. Pi 5 thermal-throttles with `small.en`. |
| Piper TTS engine | ~10 MB | |
| Piper voice (en, low-quality, ~32 kHz) | ~25 MB | Pi 4 default. |
| Piper voice (en, medium, libritts) | ~70 MB | Pi 5+ default. |
| `openWakeWord` engine | ~5 MB | Open-source, beats Porcupine on accuracy in the open benchmarks. |
| Wake-word models (`hey-molty`, `hey-hermes`, `alexa`, `hey-google` for testing) | ~30 MB | Small per-word; we ship a couple defaults plus the user can add. |
| Silero VAD | ~5 MB | Voice activity detection / endpointing. |
| ALSA + PipeWire userspace | ~15 MB | Audio plumbing. |

**Tier subtotal:** Pi-4 minimal (~80 MB) → Pi-5 standard (~180 MB) → x86-class (~370 MB).

Per benchmarks, on Pi 5 whisper.cpp `base.en` is real-time-capable with active cooling and `small.en` thermal-throttles. We pick model size per target accordingly. Pi 4 4GB skips voice entirely; Pi 4 8GB gets `tiny.en` only. There is **no audio device** on a typical DigitalOcean droplet — voice tier is omitted on cloud targets except Oracle ARM Free where the user is likely using it as a remote agent and might pipe audio over a Tailscale connection from a phone (we ship the engines but not autostart the voice service).

### 3.5 Local LLM tier

| Item | Compressed cost | Pi 4 8GB | Pi 5 8GB | Pi 5 16GB | Oracle 24GB | x86 mini-PC | Cloud GPU |
|---|---|---|---|---|---|---|---|
| `llama.cpp` arm64 / x86_64 build with the right `-march` | ~5 MB | – | ✓ | ✓ | ✓ | ✓ | ✓ |
| Llama-3.2-1B Q4_K_M | ~750 MB | – | ✓ (12–18 t/s) | – | – | – | – |
| Llama-3.2-3B Q4_K_M | ~2.0 GB | – | – | ✓ (4–7 t/s) | ✓ | ✓ | – |
| Qwen2.5-Coder-1.5B Q4_K_M (small coding model) | ~1.0 GB | – | – | ✓ | ✓ | ✓ | – |
| `vLLM` + CUDA wheels | ~3.5 GB | – | – | – | – | – | ✓ |
| `bge-small-en-v1.5` embedding model | ~30 MB | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ |
| `bge-reranker-base` reranker | ~80 MB | – | – | ✓ | ✓ | ✓ | ✓ |

Local LLM lets the device respond when the cloud model is unreachable, and lets latency-sensitive tools (RAG re-ranking, tool-call routing) run without a round trip. We never make a local LLM the default *chat* model on Pi-class hardware — quality gap with Claude Sonnet etc. is too wide. We do use it for:

- **Embedding** for memory recall (`bge-small-en-v1.5` is excellent for size).
- **Reranking** RAG hits where we have a 16GB+ device.
- **Offline chat fallback** when the configured cloud provider returns 5xx for >30 s.

The 1B/3B size split mirrors the benchmarks: Pi 5 8GB can comfortably hold Llama-3.2-1B Q4 plus the agent and Chromium; 3B is workable on Pi 5 16GB and excellent on Oracle 24GB. Pi 4 doesn't carry a local LLM — token rates are too low for usable interactive UX (under 2 t/s), and the RAM is needed for the agent + browser.

### 3.6 OCR / vision tier

| Item | Compressed cost | Notes |
|---|---|---|
| Tesseract 5 + English language pack | ~50 MB | Solid OCR, no GPU needed. |
| Additional Tesseract language packs | ~5–15 MB each | Wizard offers a checkbox per language. None ship by default; user picks. |
| `libvips` (already in CLI belt) | – | Image manipulation. |
| `libheif` + `libwebp` | ~5 MB | Modern image formats agents will encounter. |
| OpenCV Python (CPU build) | ~120 MB | **Opt-in only** — too big for default. We ship a profile flag for users who need vision-heavy workloads. |
| `rpicam-apps` | ~10 MB | Pi-only; unlocks "the agent can see what the camera sees". |

**Tier subtotal default:** ~60 MB. With OpenCV opt-in, ~180 MB.

### 3.7 RAG / vector store tier

| Item | Compressed cost | Notes |
|---|---|---|
| `sqlite-vec` extension | ~2 MB | Lightweight vector store inside the SQLite the agent already uses. |
| FAISS CPU build | ~15 MB | Alternative for larger collections. |
| `bge-small-en-v1.5` (already in 3.5) | – | Embedding source. |

Always shipped — costs nothing meaningful and lets the agents do "remember last week's chat" with persistent vector recall.

### 3.8 NPU runtime (variant builds only)

| Item | Compressed cost | Targets |
|---|---|---|
| Hailo HailoRT + drivers + sample models | ~80 MB | `rpi5+hailo` only |
| Edge TPU `libedgetpu` + `pycoral` | ~50 MB | `rpi5+coral` only |
| OpenVINO runtime + small INT8 models | ~150 MB | `x86_64-mini-pc` only |

NPUs accelerate STT (whisper-on-Hailo is a known win), small classifier models, and image/audio processing the agent might invoke. We ship only the runtime and a couple of demo models; advanced users can sideload their own model files into the data partition.

### 3.9 Networking extras

| Item | Compressed cost | Default |
|---|---|---|
| Tailscale binary (not running) | ~30 MB | Every target — wizard can opt-in. |
| `cloudflared` binary (not running) | ~25 MB | Every target — wizard can opt-in. |
| Mosh (`mosh-server`) | ~3 MB | Every target. |
| WireGuard userspace `wireguard-go` | ~10 MB | Every target as Tailscale fallback. |
| Caddy (already in core) | – | – |

Extras that *don't autostart* but are present so wizard opt-ins are instant.

### 3.10 Observability

| Item | Compressed cost | Default |
|---|---|---|
| `vector` (datadog/vector log shipper) | ~25 MB | Off by default; opt-in for fleets. |
| `node_exporter` | ~5 MB | Off by default; opt-in. |
| `otel-collector` (small build) | ~30 MB | Off by default; opt-in. |
| `prometheus` (full server) | ~80 MB | **Not** baked — fleet-only, lives on a separate device. |

Observability is an opt-in PWA toggle once an OTLP endpoint is configured. Off by default to honour the "no telemetry without consent" promise from [09-security.md](09-security.md).

### 3.11 Code-execution sandbox extras

The agents run user-supplied or LLM-generated code. We harden this:

| Item | Compressed cost | Notes |
|---|---|---|
| Bubblewrap `bwrap` (already in core) | – | The sandbox primitive. |
| Pre-baked seccomp profiles | <1 MB | One per agent. |
| Pre-baked AppArmor profiles for sandboxed code (already in core) | – | – |
| `nsjail` | ~3 MB | Alternative we ship for users who want it. |
| Disposable user namespaces helpers | – | Pi 5 unprivileged userns enabled per [09-security.md](09-security.md). |

### 3.12 Smart-home / IoT (opt-in profile flag)

If the user enables a smart-home profile flag at build time, the image grows by ~80 MB to include:

- `mosquitto` (MQTT broker, ~5 MB)
- `mosquitto-clients` (~1 MB)
- Zigbee2MQTT prerequisites (Node packages already prebuilt in cache)
- A Tasmota / ESPHome flasher

This is **not** a profile in the agent sense (deputyOS profiles are personal-AI-assistants per [CONTRIBUTING.md](../CONTRIBUTING.md#profile-class)). It's an image build option — the agent calls into MQTT as a tool when present. Off by default.

## 4. Per-target inclusion matrix

Read across to see what's in a given image. ✓ = baked, opt = available but off, – = not present.

| Category / item | rpi4-4gb | rpi4-8gb | rpi5-8gb | rpi5-16gb | rpi5+hailo | rpi5+coral | x86-mini-pc | arm64-generic | wsl2 | macos-qemu | DO | Oracle 24GB | fly | fly+gpu |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| Core (always-on) | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ |
| CLI utility belt | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ |
| ClamAV + Magika | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ |
| Camoufox | – | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ | – | ✓ | – | ✓ | ✓ | – | ✓ |
| chromium-headless-shell | – | – | – | – | – | – | ✓ | – | – | – | – | – | – | – |
| Voice — `tiny.en` | – | ✓ | – | – | – | – | – | – | – | – | – | – | – | – |
| Voice — `base.en` | – | – | ✓ | ✓ | ✓ | ✓ | – | – | – | – | – | – | – | – |
| Voice — `small.en` | – | – | – | – | – | – | ✓ | – | – | – | – | ✓ | – | – |
| Wake-word (`openWakeWord`) | – | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ | – | – | – | – | ✓ | – | – |
| Piper TTS (low) | – | ✓ | – | – | – | – | – | – | – | – | – | – | – | – |
| Piper TTS (medium) | – | – | ✓ | ✓ | ✓ | ✓ | ✓ | – | – | – | – | ✓ | – | – |
| Llama-3.2-1B Q4 | – | – | ✓ | – | – | – | – | – | – | – | – | – | – | – |
| Llama-3.2-3B Q4 | – | – | – | ✓ | ✓ | ✓ | ✓ | – | – | – | – | ✓ | – | – |
| Qwen2.5-Coder-1.5B Q4 | – | – | – | ✓ | ✓ | ✓ | ✓ | – | – | – | – | ✓ | – | – |
| vLLM + CUDA | – | – | – | – | – | – | – | – | – | – | – | – | – | ✓ |
| `bge-small` embed | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ | – | ✓ | ✓ | ✓ | ✓ |
| `bge-reranker-base` | – | – | – | ✓ | ✓ | ✓ | ✓ | – | – | – | – | ✓ | – | ✓ |
| sqlite-vec + FAISS | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ |
| Tesseract 5 + en | – | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ | – | – | – | ✓ | ✓ | – | – |
| OpenCV (opt) | opt | opt | opt | opt | opt | opt | opt | opt | opt | – | opt | opt | – | opt |
| Hailo runtime | – | – | – | – | ✓ | – | – | – | – | – | – | – | – | – |
| Coral runtime | – | – | – | – | – | ✓ | – | – | – | – | – | – | – | – |
| OpenVINO runtime | – | – | – | – | – | – | ✓ | – | – | – | – | – | – | – |
| Tailscale + cloudflared | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ |
| Mosh + WireGuard userspace | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ |
| OTel collector (opt) | opt | opt | opt | opt | opt | opt | opt | opt | opt | – | opt | opt | opt | opt |
| Smart-home (build flag) | flag | flag | flag | flag | flag | flag | flag | flag | – | – | flag | flag | – | flag |
| **Approximate compressed size** | **1.0 GB** | **1.5 GB** | **2.3 GB** | **3.5 GB** | **3.6 GB** | **3.6 GB** | **3.9 GB** | **1.2 GB** | **1.1 GB** | **0.9 GB** | **1.3 GB** | **3.5 GB** | **0.9 GB** | **5.5 GB** |

Each cell maps to a `when:` gate in `roles/deputyos/tasks/`. Adding a new gate is one line of Ansible plus a CI matrix tweak.

## 5. Air-gapped tier (M4.5 add-on)

A separate axis from the per-target matrix: `AIRGAP=1` produces a variant
of any (target × tier) tuple that bakes a tier-appropriate **LFM2** GGUF
served by `llama.cpp`, points apt at a baked-in mirror, and locks egress
at nftables. The image works with `-net none` from first boot.

### 5.1 Per-tier LLM defaults

| Tier     | Default LLM (GGUF, Q4_K_M)            | Approx model size | Approx image size delta |
|----------|---------------------------------------|--------------------|-------------------------|
| lean     | `LFM2-350M-Q4_K_M.gguf`               | ~250 MB            | +~300 MB                |
| standard | `LFM2-1.2B-Q4_K_M.gguf`               | ~750 MB            | +~800 MB                |
| rich     | `LFM2-2.6B-Q4_K_M.gguf` + `Qwen2.5-Coder-1.5B-Instruct-Q4_K_M.gguf` | ~2.6 GB combined | +~2.8 GB |

Resulting compressed image sizes (rough): lean-airgap ~1.3 GB,
standard-airgap ~2.8 GB, rich-airgap ~6.5 GB. SHAs are pinned in
`roles/deputyos/vars/llm-airgap.yml` and the role refuses to bake on
mismatch.

### 5.2 Per-target airgap support

Authoritative per-target defaults live in
`roles/deputyos/files/limits.<target>.json` under `airgap_supported` +
`airgap_max_tier`:

| Target              | airgap_supported | max airgap tier |
|---------------------|------------------|-----------------|
| rpi5                | yes              | rich            |
| rpi4                | yes              | **lean only** (Cortex-A72 cannot run LFM2-1.2B at usable t/s) |
| arm64-generic       | yes              | standard        |
| x86_64-mini-pc      | yes              | rich            |
| wsl2                | yes              | standard        |
| macos-qemu          | yes              | rich            |
| digitalocean        | yes              | rich            |
| oracle-arm-free     | yes              | standard        |
| hetzner-cloud / linode / vultr | yes   | rich            |
| fly-machines        | **no**           | —               |

### 5.3 Egress posture

Airgap images bake `/etc/deputyos/network-policy.json` with `mode=airgap`.
nftables denies all output except RFC1918 + mDNS + the local DNS
resolver. `deputyctl network unlock` flips the mode to `open` post-boot.
The `whitelist` mode lights up in M5.5 — schema is forward-compatible.

### 5.4 Updates over sneakernet

`deputyctl update --from /mnt/deputyos/<usb>/manifest.json` consumes a
manifest dropped onto a USB stick (auto-mount under `/mnt/deputyos/<label>`
when the M3.5 mounts policy permits). Signature verification (minisign +
cosign + SLSA) gates apply unchanged.

## 6. Excluded by design (and why)

| Excluded | Why |
|---|---|
| Full Chromium browser | Camoufox is smaller, faster idle, and better against bot detection for the agents' real workloads. |
| Full Rust / Go toolchain | Hundreds of MB. Users who need them install them on a build host; the agent doesn't need a full toolchain to *call* them. |
| Jupyter / IPython kernel server | Heavy and overlaps with the agent's own code-execution path. |
| Home Assistant | Per the profile-class rule — not a profile, not in scope. Users who run HA call it as an MQTT consumer instead. |
| Docker / Podman daemon | [ADR-0004](adr/0004-systemd-not-docker-on-device.md). |
| Avahi reflector | Security: don't relay mDNS across L2 boundaries by default. |
| Bluetooth | Disabled. Pi has it; we don't ship the daemon enabled. |
| GUI desktop | This is an appliance. PWA at `/app` is the UI. |
| `apt` mirrors / signed third-party repos | We don't run `apt` at boot; preconfigured mirrors are noise. |
| `freshclam` autostart | Per [ADR-0008](adr/0008-clamav-plus-magika-baseline.md), signatures travel in the slot image. |
| Telemetry agents (Datadog, NewRelic) | Not on by default; opt-in via OTel only. |
| Crash reporting daemons | `apport`, `whoopsie` disabled — no callbacks home without consent. |

## 7. Maintenance over time

The matrix above is the M2 target — by the end of M2 every column is built, smoke-tested, and published. Categories evolve over time:

- **Voice models**: as `whisper.cpp` adds quantised variants (e.g. `large-v3-turbo` Q4), the x86 / Oracle 24GB images can take a step up. Pi 5 stays on `base.en` until thermal characteristics improve.
- **Local LLM**: a new generation of small models (e.g. Llama-3.3-1B, Phi-4-mini, Qwen2.5-1.5B) lands maybe twice a year. We treat the bundled model as part of the image manifest; updates ship in regular A/B image revs.
- **NPU runtimes**: Hailo and Coral have their own software cadence; we float pinned versions in the variant Ansible task and bump them in release-tracker PRs the same way profiles get bumped.
- **CLI utility belt**: rare changes; only when a new tool earns its way in by being something the agents reach for repeatedly.
- **OpenVINO / vLLM**: x86 and GPU targets respectively; pinned per image rev.

## 8. Open questions to resolve before M2

1. **Should `Llama-3.2-1B` be the default offline-fallback chat model on rpi5-8gb?** Token rate is 12–18 t/s (acceptable for short replies). Tradeoff: ~750 MB image growth. Default to *yes* unless user surveys say flash time matters more.
2. **Default `bge-reranker-base` shipping cutoff.** Current call is "16GB+ only". Could justify it on 8GB if reranker isn't loaded persistently. Decision deferred to M2 when we measure real RAM use.
3. **Camoufox vs chromium-headless-shell** — is Camoufox enough for both agents on every channel they use? If yes, drop chromium-headless-shell from the x86 image (saves ~140 MB).
4. **Per-language Tesseract packs** in the wizard — should we ship `eng` plus the system locale's primary language by default? Lean toward yes for usability; cost is small.
5. **Smart-home build flag** — should this be a separate bake variant (e.g. `deputyos-<profile>-<hw>-smart-home-<channel>`) rather than a build flag? Flag keeps the matrix small; variant gives users a download choice without flag knowledge.

These get resolved during M1/M2 with empirical benchmarking on real hardware.

## 9. Summary

The image is not "one size fits all". It is "the right size for *your* hardware":

- **Pi 4 4GB**: lean, cloud-LLM only.
- **Pi 4 8GB**: + browser + OCR.
- **Pi 5 8GB**: + voice (`tiny.en`) + small local LLM.
- **Pi 5 16GB**: + voice (`base.en`) + 3B local LLM + reranker.
- **Pi 5 NPU**: + Hailo / Coral acceleration.
- **x86 mini-PC**: + voice (`small.en`) + 3B local + OpenVINO.
- **Cloud (DO / Hetzner / Vultr / Linode / WSL2 / macOS / fly)**: lean; cloud-LLM oriented (no audio devices).
- **Oracle ARM Free 24GB**: rich; comparable to Pi 5 16GB.
- **fly+gpu**: + vLLM + CUDA — the only image with cloud-class local inference baked.

Every line in the matrix is a `when:` gate the shared Ansible role honours. New targets are PRs that flip a small number of gates; no role fork.
