# 13 — Memory pressure (designing for beyond-RAM operation)

This is the doc for the systems-engineering reality the rest of the docs implied but didn't make explicit:

> **On every limited-RAM target — Pi 4 4GB, Pi 4 8GB, Pi 5 8GB, even rpi5+local-LLM at peak — the working set will exceed physical RAM in normal use.** That's not a bug. It's the design point. The question is whether overshoot feels like a small latency spike (good) or a kernel livelock (catastrophic).

If we don't engineer for this, the appliance becomes the kind of device where "it works fine for a day, then the agent stops responding and only a power-cycle fixes it." Semi-technical users will not file a bug. They'll throw the Pi in a drawer and tell their friends it's broken.

This doc names every component, gives an honest resident-set budget, and lays out the OS-level levers (zram, swap, cgroup memory limits, systemd-oomd PSI thresholds, lazy loading, freezing) that turn overshoot into graceful degradation.

## 1. Honest memory budgets

Resident-set sizes for the components that run on the device. These are observed numbers, not aspirational.

| Component | Idle RSS | Working RSS | Notes |
|---|---|---|---|
| Linux kernel + base userspace | 150 MB | 150 MB | Stable. |
| systemd, journald, sshd, NetworkManager, Caddy, deputyctl | 60 MB | 80 MB | |
| ufw, fail2ban, auditd | 30 MB | 30 MB | |
| **ClamAV `clamd` (with full signature DB loaded)** | **900 MB** | **1.0–1.2 GB** | Single biggest persistent consumer when present. |
| ClamAV `clamscan` invoked on-demand | 0 MB | 800 MB transient (during scan) | Same memory cost, but only during scan. |
| Magika prefilter | 5 MB | 50 MB transient | Per-file inference is short-lived. |
| Node 24 + OpenClaw with channels active | 350 MB | 600–800 MB | Higher with browser-tool calls in flight. |
| Python 3.11 + Hermes with channels active | 250 MB | 400–500 MB | Lower than OpenClaw's Node footprint. |
| Camoufox idle (one tab) | 40 MB | 200–500 MB | Spike during navigation. |
| `chromium-headless-shell` idle | 80 MB | 300–600 MB | Heavier than Camoufox. |
| Whisper.cpp + `tiny.en` model loaded | 80 MB | 150 MB during transcription | Loaded only during voice. |
| Whisper.cpp + `base.en` model loaded | 130 MB | 250 MB during transcription | |
| Whisper.cpp + `small.en` model loaded | 280 MB | 500 MB during transcription | |
| Piper TTS + voice loaded | 30 MB | 80 MB during synthesis | |
| openWakeWord (always-listening) | 40 MB | 60 MB | Continuous audio capture loop. |
| llama.cpp + Llama-3.2-1B Q4 (mmap'd) | 100 MB anon + 750 MB page cache | 1.0 GB anon during inference | Page cache evictable under pressure. |
| llama.cpp + Llama-3.2-3B Q4 (mmap'd) | 200 MB anon + 2 GB page cache | 3 GB anon during inference | |
| `bge-small-en-v1.5` embedding model loaded | 80 MB | 100 MB | Cheap; always loaded. |
| `bge-reranker-base` loaded | 200 MB | 300 MB | Loaded only on rpi5-16gb+. |
| sqlite-vec + FAISS index at typical scale | 30 MB | 50 MB | Grows with stored vectors. |

### What that adds up to

**Pi 4 4GB, OpenClaw active, browser cold, no voice, no local LLM:**
- With clamd: 150 + 80 + 30 + 900 + 350 + 40 + 80 ≈ **1.6 GB idle, 2.5 GB working**. Headroom: ~1.5 GB. Tight when ClamAV scans run.
- Without clamd (on-demand `clamscan` instead): ~700 MB idle, ~1.6 GB working. Headroom: ~2.4 GB.
- **Decision: rpi4-4gb does NOT run clamd. On-demand `clamscan` only, gated by Magika hit.**

**Pi 4 8GB, both agents loaded one active, browser idle, no voice:**
- ~2.0 GB idle, ~2.8 GB working with clamd. Headroom: ~5 GB.
- Comfortable. clamd stays on.

**Pi 5 8GB with 1B local LLM enabled, voice (`base.en`) on, Camoufox active:**
- ~1.9 GB idle (model in page cache, not anon) + 600 MB Node + 130 MB whisper + 60 MB wakeword + 40 MB browser idle = ~2.7 GB anon idle.
- During an inference call with concurrent browser navigation: ~3.5–4 GB anon. Page cache for the 1B model gets evicted under pressure → re-read from NVMe (acceptable; ~750 MB at 800 MB/s = ~1 s) or from SD (catastrophic; ~25 s).
- **Decision: rpi5-8gb with local LLM strongly recommends NVMe boot. Wizard warns if SD-only.**

**Pi 5 16GB with 3B local LLM, voice (`base.en`) on, Camoufox active:**
- ~5 GB anon during peak (Node + browser navigating + 3B inference + whisper). Headroom: ~10 GB.
- Comfortable; mmap eviction is rare.

**Oracle ARM Free 24GB with 3B local LLM:**
- ~5 GB peak. Headroom: ~18 GB. Most comfortable target after `fly+gpu`.

**`fly+gpu` (NVIDIA L40S):** RAM rarely the constraint; VRAM is the budget. Out of scope for this doc.

## 2. Compressed RAM (zram)

zram gives us a "swap device" in compressed RAM — pages get compressed on the way out, decompressed on the way in. For an SD-booted Pi this is the difference between "swap that wears out the SD card and is 30 MB/s" and "swap that's a few hundred MB/s and writes to nothing."

### Per-target zram configuration

| Target | zram size | Algorithm | Why |
|---|---|---|---|
| `rpi4-4gb` | 50% RAM (~2 GB) | lz4 | Slow CPU; lz4 wins. |
| `rpi4-8gb` | 50% RAM (~4 GB) | lz4 | Same. |
| `rpi5-8gb` | 50% RAM (~4 GB) | zstd | A76 cores handle zstd cheaply; better ratio gives more usable headroom. |
| `rpi5-16gb` | 25% RAM (~4 GB) | zstd | Plenty of physical RAM; less zram needed. |
| `arm64-generic` | 50% RAM | lz4 | Conservative default. |
| `x86_64-mini-pc` | 25% RAM | zstd | Fast cores, lots of RAM. |
| `digitalocean` and other cloud | 0 (host swap exists) | n/a | Don't add zram — host already provides swap; double-swapping is pessimal. |
| `oracle-arm-free` | 25% RAM | zstd | Plenty of physical RAM. |

Effective compressed ratio is typically 2.5–3:1 for lz4 and 3–4:1 for zstd on real workloads. So 2 GB of zram on rpi4-4gb gives ~5–6 GB of effective swap-equivalent that costs zero disk wear.

### Why not zswap?

zswap compresses pages on their way to a real disk swap. Useful when disk swap is fast (NVMe) and we want compression as an optimisation. On SD-card-booted devices, the goal is to *avoid* hitting disk at all — zram is the better fit. On NVMe-booted Pis we still prefer zram because the bottleneck is disk wear-equivalent and CPU is plentiful, not disk speed.

## 3. On-disk swap policy

A swap *file* on the data partition, sized to RAM, with `swappiness=10` (prefer keeping pages in RAM). This is the safety net when zram is exhausted.

| Storage class | Swap policy |
|---|---|
| SD card boot | 1 GB swap file, accessed only when zram is full. `vm.swappiness=10`. SD wear is real but bounded. |
| USB3 SSD or NVMe | 2 GB swap file, `vm.swappiness=20`. Higher swappiness fine because disk is fast. |
| DigitalOcean / Hetzner / Vultr droplet | use host swap; no local swap file added. |

**Critical**: `deputyctl doctor` reports observed swap-in rate. Sustained > 1 MB/s of swap-in over 5 minutes triggers a wizard banner: "Memory pressure is high; consider disabling local LLM or reducing channels." This is the user-visible signal *before* the device starts feeling sluggish.

## 4. ClamAV: per-target daemon-vs-scan decision

ClamAV's `clamd` is the single biggest constant memory consumer when present. The decision tree:

```
Target RAM ≥ 8 GB ?
 ├── yes → clamd persistent. fanotify on uploads dir. Daily clamscan timer.
 └── no  → Magika prefilter only. clamscan invoked on-demand
            on Magika "suspicious" or "type/extension mismatch" events.
            Daily full clamscan timer runs at 04:30 with cgroup MemoryMax=900M.
```

Concrete:

| Target | clamd | clamscan-on-demand | Daily timer | Signature DB |
|---|---|---|---|---|
| `rpi4-4gb` | – | ✓ | ✓ (cgroup-bounded) | Full DB shipped, daily-only loaded |
| `rpi4-8gb` | ✓ | – | ✓ | Full DB |
| `rpi5-8gb` | ✓ | – | ✓ | Full DB |
| `rpi5-16gb` and up | ✓ | – | ✓ | Full DB |
| Cloud targets | ✓ | – | ✓ | Full DB |

For rpi4-4gb specifically:
- Magika runs cheap (~50 MB transient) on every channel-uploaded file.
- If Magika reports type/extension mismatch or matches a known-bad-class hint, `clamscan` is invoked under a cgroup with `MemoryMax=900M`. The scan completes or is killed — both are acceptable.
- Daily full-disk scan at 04:30 (idle hour) under the same cgroup, low priority.

This keeps the security baseline equivalent (signatures are the same; both are ClamAV) while dropping ~900 MB of always-resident memory on the smallest target.

## 5. Per-service cgroup limits (the firewall that prevents cascade)

Without per-service limits, one runaway component (a browser navigating to a huge page, ClamAV reloading signatures, a Node memory leak) can starve the kernel and cause global OOM. With limits, the OOM killer hits the right victim and the rest of the device keeps running.

Every systemd unit ships with `MemoryHigh` (soft cap; throttled past this) and `MemoryMax` (hard cap; OOM-killed past this). Values are templated per-target by the build pipeline based on total RAM. Concrete examples:

```ini
# /etc/systemd/system/deputyos-openclaw.service.d/limits.conf  (rpi4-4gb)
[Service]
MemoryHigh=900M
MemoryMax=1.2G
TasksMax=200

# /etc/systemd/system/deputyos-camoufox.service.d/limits.conf  (rpi4-4gb)
[Service]
MemoryHigh=400M
MemoryMax=600M
OOMPolicy=stop                # browser is restartable; let it die cleanly
TasksMax=50

# /etc/systemd/system/clamscan-on-demand@.service.d/limits.conf  (rpi4-4gb)
[Service]
MemoryHigh=700M
MemoryMax=900M
OOMPolicy=stop

# /etc/systemd/system/deputyos-llamacpp.service.d/limits.conf  (rpi5-8gb)
[Service]
MemoryHigh=1.4G
MemoryMax=1.8G
OOMPolicy=continue            # service auto-restarts; transient OOM fine
```

`OOMPolicy=stop` means the service is killed and not restarted — used for restartable, ephemeral things (browser, on-demand clamscan). `OOMPolicy=continue` means the service can be killed but systemd keeps trying — used for the agent itself.

These bounds also live in the profile manifest's `[memory]` section so users can tune them per device:

```toml
# profiles/openclaw.toml (excerpt — added in M2)
[memory]
agent.MemoryHigh        = "auto"   # computed from total RAM at first boot
agent.MemoryMax         = "auto"
browser.MemoryHigh      = "auto"
local_llm.MemoryHigh    = "auto"
```

`auto` resolves at first-boot via a small lookup table keyed on total RAM × profile.

## 6. systemd-oomd (proactive OOM via PSI)

The kernel OOM killer is reactive — it fires after the system is already thrashing. systemd-oomd uses PSI (Pressure Stall Information) to act *before* that point.

### Configuration we ship

`/etc/systemd/oomd.conf.d/deputyos.conf`:

```ini
[OOM]
SwapUsedLimit=80%
DefaultMemoryPressureLimit=60%
DefaultMemoryPressureDurationSec=20s
```

Translation: if more than 80% of zram+swap is in use, or if memory pressure (the fraction of time tasks are stalled on memory) sits over 60% for 20 seconds, oomd starts killing. It picks the cgroup with the highest reclaim activity — typically the runaway component, not the agent.

Per-cgroup overrides apply tighter limits to expendable services:

```ini
# /etc/systemd/system/deputyos-camoufox.service.d/oomd.conf
[Service]
ManagedOOMMemoryPressure=kill
ManagedOOMMemoryPressureLimit=40%
ManagedOOMSwapKill=yes
```

Browser is more aggressively killed than the agent itself.

### Why not earlyoom

earlyoom uses raw memory consumption, not PSI. It works on systems without cgroups v2 or for users who'd rather not deal with systemd's complexity. We have cgroups v2 everywhere and PSI gives us better signal. We use systemd-oomd.

### The "no swap = livelock" caveat

systemd-oomd needs swap (or zram) to work well. Without swap, pressure rises so fast that oomd can't react before the kernel does. Every deputyOS target ships with zram, exactly because of this.

## 7. llama.cpp and the mmap question

llama.cpp uses `mmap()` to load model weights by default. This is mostly a win — the kernel manages page residency, and weights that aren't currently being used can be evicted to make room for other things — but on slow storage it's a footgun.

| Storage | mmap recommended? | Behaviour under pressure |
|---|---|---|
| NVMe (Pi 5 PCIe HAT, x86 M.2) | ✓ yes | Page evicted → re-read at ~800 MB/s. Single-token stall ~1 s for a 750 MB model. Acceptable. |
| USB3 SSD | ✓ yes | ~400 MB/s; ~2 s stall. Acceptable. |
| SD card | ✗ disable mmap | ~30 MB/s; ~25 s stall per token. Catastrophic. Use `--no-mmap` and load model into anon RAM (which is honest about pressure and lets cgroup limits do their job). |

The build pipeline writes `/etc/deputyos/llamacpp.env` based on detected boot media. deputyctl's local-LLM service reads this:

```bash
# auto-generated at first boot
LLAMACPP_MMAP=1     # or =0 on SD
LLAMACPP_THREADS=4  # cores - 1 to leave headroom
LLAMACPP_CONTEXT=4096
```

**Wizard nudge**: if SD-only on a target where local LLM is enabled, the wizard prints a one-liner urging USB3 SSD or NVMe. We don't refuse to enable LLM on SD — some users will accept the latency for the privacy win — but we make the trade-off visible.

## 8. Lazy loading and idle eviction

Keeping a component resident is only worth it if it's likely to be needed soon. We aggressively unload cold things.

| Component | Lifecycle |
|---|---|
| Camoufox / chromium | Spawn on first browser-tool call; reap after 60 s idle. Idle RSS goes to zero between calls. |
| Whisper.cpp | Loaded for the duration of a voice session; unloaded 30 s after last audio. |
| Piper TTS | Same as Whisper. |
| openWakeWord | Always-on if voice is enabled; very small. |
| llama.cpp | Loaded for the duration of an inference; default 5-minute keep-alive, configurable. |
| Embedding model | Always loaded; small enough not to matter. |
| Reranker | Loaded for a query; ~30 s keep-alive between queries. |
| ClamAV daemon (where present) | Always loaded — no point in unloading; signatures take ~5 s to reload. |
| ClamAV on-demand (rpi4-4gb) | Spawned only on Magika hint; killed after scan. |

Implementation: each "tool runner" is its own systemd template unit `deputyos-tool-<name>@.service` that gets started by `deputyctl tool spawn` and has `Type=oneshot` semantics + an idle-timeout watchdog from `deputyctl`.

## 9. Pre-warming vs on-demand

For interactive latency, some things benefit from being warm:

- **Embedding model**: pre-loaded at agent start. Cost: 80 MB. Always worth it — every message hits embeddings for memory recall.
- **bge-reranker-base** (rpi5-16gb+): pre-loaded. 200 MB.
- **whisper `base.en`** (rpi5+ with voice): pre-loaded only if wake-word is enabled. Otherwise on-demand (acceptable: voice is a deliberate user interaction, ~500 ms warm-up is fine).
- **Camoufox**: NOT pre-warmed. On-demand spawn is the right call; warm-up cost is amortised across the browser session.
- **llama.cpp local LLM**: pre-warmed only if it's the configured *fallback chat model* and the cloud provider has been unreachable in the last hour.

## 10. Telemetry and warnings

`deputyctl doctor --memory` reports:

```
$ deputyctl doctor --memory
RAM:           7.6 GiB total
zram:          3.8 GiB compressed → 1.1 GiB physical (3.5x ratio)
swap (disk):   2.0 GiB free, 0 KiB/s in, 0 KiB/s out (last 5m)
Pressure:
  some 10s:  1.3%   60s: 0.8%   300s: 0.4%
  full 10s:  0.0%   60s: 0.0%   300s: 0.0%

Resident set by service:
  deputyos-openclaw.service     623 MiB / 1.5 GiB high / 2.0 GiB max
  deputyos-clamd.service        954 MiB / 1.0 GiB high / 1.2 GiB max
  deputyos-camoufox.service       0 MiB (not running)
  ...

OK
```

Warning bands:

| Symptom | Threshold | What `deputyctl doctor` says |
|---|---|---|
| Sustained `some` pressure > 30% over 5 min | warn | "Memory pressure elevated. Disable a channel?" |
| Sustained swap-in > 1 MB/s over 5 min | warn | "Heavy swapping detected. NVMe upgrade recommended." |
| Any service hits MemoryMax | warn | "Service X was OOM-killed at <time>; check `deputyctl logs`." |
| zram ratio < 2.0× | info | "Compression ratio low; workload may not benefit much from zram." |

These warnings surface in the PWA dashboard's status card so users see them without typing commands.

## 11. Wizard and channel implications

The wizard knows total RAM at first boot and asks the user about channels accordingly:

- On `rpi4-4gb` the wizard *defaults the heaviest channels off* — WhatsApp Cloud webhook, Discord with voice, anything that maintains a persistent process. User can opt them on with a "your device may run out of memory" warning.
- On any target, if the user enables both local LLM and voice on a tight target, the wizard shows a calculated headroom estimate and asks for confirmation.
- "Channels disabled by memory budget" appears explicitly in the wizard summary so the user understands *why* — never silently dropped.

## 12. Per-target memory plan summary

| Target | Strategy |
|---|---|
| `rpi4-4gb` | clamd off; on-demand clamscan with 900M cgroup. zram lz4 50%. swap 1G on disk. systemd-oomd PSI 60% / 20s. No local LLM. Browser cgroup max 600M. Wizard defaults heavy channels off. |
| `rpi4-8gb` | clamd on. zram lz4 50%. swap 1G. systemd-oomd 60%/20s. No local LLM. Browser cgroup 800M. |
| `rpi5-8gb` | clamd on. zram zstd 50%. swap 2G on NVMe (1G on SD). 1B local LLM (NVMe strongly recommended; mmap disabled on SD). Browser 600M. PSI 60%/20s. |
| `rpi5-16gb` | clamd on. zram zstd 25%. swap 2G. 3B local LLM + reranker. Browser 800M. PSI 50%/30s. |
| `rpi5+hailo` / `+coral` | as rpi5-16gb. |
| `arm64-generic` | conservative: as rpi4-8gb default but adjust to detected RAM. |
| `x86_64-mini-pc` | clamd on. zram zstd 25%. swap on internal SSD 4G. 3B local + reranker. Browser 1G. |
| `wsl2`, `macos-qemu` | host handles swap; no zram. clamd on. cgroup limits relaxed. |
| `digitalocean`, `hetzner`, `vultr`, `linode` | use host swap; no local zram or swap file. clamd on. cgroup limits per droplet size. |
| `oracle-arm-free` (24 GB) | clamd on. zram zstd 25%. 4G swap on local NVMe. 3B local + reranker. Browser 1G. |
| `fly`, `fly+gpu` | OCI runtime handles. cgroup limits via fly machine spec. |

## 13. Failure modes we explicitly guard against

| Failure | Guard |
|---|---|
| ClamAV reload swamps RAM and kills the agent on rpi4-4gb | clamd disabled; on-demand `clamscan` runs in its own cgroup with `OOMPolicy=stop`. |
| Browser memory leak silently grows over hours | Camoufox cgroup `MemoryMax`; reaped after 60 s idle; restart on every tool call. |
| Local LLM mmap thrashes on SD | mmap disabled when SD is detected; wizard nudges NVMe. |
| Multiple voice sessions overlap | Whisper service is a singleton; second concurrent voice request queues. |
| User opts into too many channels for their RAM | Wizard refuses or warns with calculated headroom. |
| Sustained swap to disk wears the SD card | `vm.swappiness=10` + zram absorbs most pressure; doctor warns user. |
| Kernel OOM kills the agent randomly | systemd-oomd PSI fires first and picks the cgroup with the most reclaim activity (typically the runaway component, not the agent). |
| Update applies and the new image's working set exceeds RAM | A/B watchdog rolls back if `deputyctl doctor` doesn't report green within 5 min. |

## 14. Open items for empirical resolution in M1/M2

1. **PSI thresholds**: 60%/20 s is a conservative starting point. Real workload data may justify 50%/30 s on bigger targets.
2. **Camoufox idle-reap timeout**: 60 s is a guess. Browser tool calls cluster, so 120 s might be better. Measure.
3. **llama.cpp KV-cache size on rpi5-8gb 1B**: 4096 context = comfortable; 8192 starts evicting other things. Pin at 4096 and surface a wizard knob.
4. **Whether to disable mmap unconditionally on rpi4 8GB despite NVMe-via-USB3**: USB3 SSD is fast enough but the cgroup math is tighter.
5. **Should the wizard let users *force* clamd on rpi4-4gb?** Probably yes with a stern warning, since some users genuinely need on-write scanning more than they need 900 MB free.

## 15. The point

A semi-technical user with a Pi 4 should boot the image, configure their agent, and forget about memory. They never see "out of memory" errors because the system was designed to expect overshoot and absorb it. They see, at worst, a brief latency spike when summoning a tool that was paged out — and the doctor command and the PWA dashboard show them why if they ever look.

If the user wants to know "is this going to break later", `deputyctl doctor` answers truthfully. If they want to know "should I upgrade my RAM or storage", the wizard nudges, doesn't lecture. The whole point is to make the appliance feel solid even on hardware that would never run this stack on its own.
