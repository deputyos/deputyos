# Device limits (`limits.json`)

`/etc/deputyos/limits.json` is the **per-device capability and
limitation report**, baked at image build time. It tells `deputyctl`,
the wizard, and the PWA what this hardware can actually do, what it
can't, and how to unblock the things it can't.

The schema is small and stable; new capability flags are added by
extending the struct with `#[serde(default)]` so older images parse
forward-compatibly. The Rust struct is in
[`deputyctl/src/limits.rs`](https://github.com/deputyos/deputyos/blob/main/deputyctl/src/limits.rs).
The 14 baked-in samples live in `roles/deputyos/files/limits.<target>.json`
and `deputyctl/etc/limits.qemu-aarch64.json`.

[TOC]

## Resolution order

`deputyctl/src/paths.rs::limits_file()`:

1. `$DEPUTYOS_LIMITS_FILE` env var if set.
2. `/etc/deputyos/limits.json` if it exists.
3. `deputyctl/etc/limits.qemu-aarch64.json` (dev fallback shipped with the source tree).

## Top-level shape

| Field | Type | Required | Description |
|---|---|---|---|
| `target` | string | yes | Hardware target id (matches `TARGET=` matrix; e.g. `rpi4`, `x86_64-mini-pc`, `digitalocean`). |
| `tier` | string | yes | Coarse capability tier. One of `low`, `standard`, `high`. |
| `ram_mb` | u32 | yes | Baseline RAM the image was sized for. Live RAM is probed by `deputyctl doctor` and may exceed this on user-resized cloud shapes. |
| `ram_class` | string | yes | One of `low`, `standard`, `high`. Used by the wizard to grey out RAM-heavy options. |
| `storage_class` | string | yes | One of `sd`, `ssd`, `nvme`, `block`, `qcow2`, `ntfs`. Hints at update / backup throughput expectations. |
| `cloud` | string | optional | Cloud provider id when the target is a cloud one (e.g. `digitalocean`, `oracle`, `fly`, `hetzner`, `linode`, `vultr`, `macos`). Absent on bare-metal targets. |
| `capabilities` | object | optional, defaults to all-false | What the device CAN do. See below. |
| `limitations` | array of objects | optional, defaults to empty | What the device CANNOT do, with a reason and an unblock recipe. |

## `capabilities` object

| Field | Type | Default | Description |
|---|---|---|---|
| `local_llm` | bool | false | Can run a local LLM with workable latency. Threshold ≈ 8GB RAM and ≥ Cortex-A76 / x86 mid-tier. |
| `voice_wake_word` | bool | false | Can run real-time wake-word detection. |
| `voice_tts` | bool | false | Can run on-device TTS. |
| `clamav_daemon` | bool | false | Can run `clamd` persistently. When false, `clamscan` runs on a timer instead, paired with Magika. |
| `channels_heavy` | array of strings | `[]` | Heavy channels the device can run alongside the gateway without OOM pressure. |
| `channels_disabled_by_ram` | array of strings | `[]` | Channels the wizard greys out for this device. |

## `limitations[]` array

Each limitation is a `{id, reason, unblock}` triple — what's blocked,
why, and what to do about it.

| Field | Type | Required | Description |
|---|---|---|---|
| `id` | string | yes | Stable identifier (e.g. `no-local-llm`, `no-voice-tts`, `sd-storage-slow-under-update-load`). |
| `reason` | string | yes | One-sentence rationale grounded in the hardware envelope. |
| `unblock` | string | yes | Concrete suggestion for moving past the limitation (target swap, wizard option, scale-up). |

The wizard, `deputyctl limits`, and the PWA all surface this list
verbatim. The principle: **never surprise the user** — every
limitation is visible at the picker, the wizard, doctor, and on the
PWA dashboard. See the [user-awareness feedback note](../../security/default-on-controls.md).

## Per-target summary (14 baked-in files)

Each cell summarises the file that the bake pipeline copies to
`/etc/deputyos/limits.json` for that target.

| Target | Tier | RAM (MB) | RAM class | Storage | Local LLM | Voice | clamd | Heavy channels |
|---|---|---|---|---|---|---|---|---|
| `qemu-aarch64` | standard | 4096 | standard | ssd | – | – | yes | telegram, slack, discord |
| `rpi4` | low | 4096 | low | sd | – | – | – | telegram, slack |
| `arm64-generic` | standard | 4096 | standard | ssd | – | – | yes | telegram, slack, discord |
| `x86_64-mini-pc` | high | 16384 | high | nvme | yes | yes | yes | telegram, slack, discord, whatsapp-cloud-webhook |
| `wsl2` | standard | 8192 | standard | ntfs | – | – | yes | telegram, slack, discord |
| `macos-qemu` | low | 2048 | low | qcow2 | – | – | – | telegram, slack |
| `digitalocean` | standard | 4096 | standard | block | – | – | yes | telegram, slack, discord |
| `hetzner-cloud` | standard | 4096 | standard | block | – | – | yes | telegram, slack, discord |
| `linode` | standard | 4096 | standard | block | – | – | yes | telegram, slack, discord |
| `vultr` | standard | 2048 | low | block | – | – | – | telegram, slack |
| `oracle-arm-free` | high | 6144 | standard | block | – | – | yes | telegram, slack, discord, matrix |
| `fly-machines` | low | 2048 | low | block | – | – | – | telegram, slack |

Two more targets (`rpi5` and `rpi5-16gb`) ship via Packer variants and
inherit a high-tier limits file generated at bake time; their `local_llm`
flag is `true`. Files are listed at
[Distribution / Hardware matrix](../../distribution/hardware-matrix.md).

## Three full samples

### Pi 4 (`limits.rpi4.json`) — low-tier ARM SBC

```json
{
  "_comment": "Per-target limits for rpi4. Copied to /etc/deputyos/limits.json at bake time. Schema lives in deputyctl/src/limits.rs. See docs/14-limitations.md §'Pi 4 (4 GB)' for the source-of-truth narrative.",
  "target": "rpi4",
  "tier": "low",
  "ram_mb": 4096,
  "ram_class": "low",
  "storage_class": "sd",
  "capabilities": {
    "local_llm": false,
    "voice_wake_word": false,
    "voice_tts": false,
    "clamav_daemon": false,
    "channels_heavy": ["telegram", "slack"],
    "channels_disabled_by_ram": ["whatsapp-cloud-webhook", "discord-voice"]
  },
  "limitations": [
    {
      "id": "no-local-llm",
      "reason": "Pi 4 RAM (4 GB) and CPU (Cortex-A72) below local-LLM threshold",
      "unblock": "rpi5 8GB+ or x86_64-mini-pc; or run model via cloud provider"
    },
    {
      "id": "no-voice-wake-word",
      "reason": "Cortex-A72 too slow for real-time wake-word detection",
      "unblock": "rpi5 8GB+ or x86_64-mini-pc"
    },
    {
      "id": "no-voice-tts",
      "reason": "TTS thermal-throttles on passive-cooled Pi 4",
      "unblock": "rpi5 with active cooler, or x86_64-mini-pc"
    },
    {
      "id": "no-clamav-daemon",
      "reason": "clamd RSS exceeds Pi 4 RAM headroom; on-demand clamscan timer used instead",
      "unblock": "rpi5 8GB+ or x86_64-mini-pc"
    },
    {
      "id": "no-whatsapp-cloud-webhook",
      "reason": "WhatsApp Cloud webhook RSS exceeds low-tier RAM headroom",
      "unblock": "upgrade to standard or high RAM tier"
    },
    {
      "id": "no-discord-voice",
      "reason": "Discord voice gateway RSS + jitter buffer exceed low-tier headroom",
      "unblock": "upgrade to standard or high RAM tier"
    },
    {
      "id": "sd-storage-slow-under-update-load",
      "reason": "SD card I/O thrashes under simultaneous update + backup load",
      "unblock": "USB3 SSD recommended (per docs/14)"
    }
  ]
}
```

### x86_64 mini-PC (`limits.x86_64-mini-pc.json`) — high tier

```json
{
  "target": "x86_64-mini-pc",
  "tier": "high",
  "ram_mb": 16384,
  "ram_class": "high",
  "storage_class": "nvme",
  "capabilities": {
    "local_llm": true,
    "voice_wake_word": true,
    "voice_tts": true,
    "clamav_daemon": true,
    "channels_heavy": ["telegram", "slack", "discord", "whatsapp-cloud-webhook"],
    "channels_disabled_by_ram": []
  },
  "limitations": [
    {
      "id": "no-discrete-gpu-tooling",
      "reason": "NVIDIA / AMD discrete-GPU tooling (CUDA, ROCm) not bundled; mini-PC SKUs don't ship with discrete GPUs",
      "unblock": "build a custom image with the discrete-GPU stack you need (docs/15-local-build.md)"
    },
    {
      "id": "openvino-only-npu",
      "reason": "iGPU acceleration via Intel OpenVINO only — covers Beelink/MeLE/NUC silicon; AMD/NVIDIA NPUs not bundled",
      "unblock": "sideload the runtime for your accelerator (see docs/12-bundled-software.md §Acceleration)"
    },
    {
      "id": "llm-7b-unsupported",
      "reason": "3B local LLM runs at workable speed; 7B is feasible but we ship no quality SLA for it",
      "unblock": "use a cloud provider for 7B+ workloads, or accept best-effort"
    }
  ]
}
```

### Fly Machines (`limits.fly-machines.json`) — container-class cloud target

```json
{
  "target": "fly-machines",
  "tier": "low",
  "ram_mb": 2048,
  "ram_class": "low",
  "storage_class": "block",
  "cloud": "fly",
  "capabilities": {
    "local_llm": false,
    "voice_wake_word": false,
    "voice_tts": false,
    "clamav_daemon": false,
    "channels_heavy": ["telegram", "slack"],
    "channels_disabled_by_ram": ["discord", "whatsapp-cloud-webhook", "matrix"]
  },
  "limitations": [
    {
      "id": "ephemeral-storage",
      "reason": "Fly machines without an attached volume lose state on stop. The data partition is a Fly volume; if the volume is deleted, agent state is lost.",
      "unblock": "always attach a Fly volume mount per fly/fly.toml.example; back up to B2/R2"
    },
    {
      "id": "cold-start-latency",
      "reason": "Free Fly machines auto-stop and cold-start adds several seconds; not ideal for low-latency voice channels",
      "unblock": "set min_machines_running=1 on a paid plan to keep the machine warm"
    },
    {
      "id": "no-audio",
      "reason": "Fly machines have no audio device; voice features disabled",
      "unblock": "deploy on rpi5 8GB+ or x86_64-mini-pc for voice"
    },
    {
      "id": "apparmor-complain",
      "reason": "AppArmor enforce mode requires --privileged or specific capability grants Fly's default machine config does not provide. Profiles ship in complain mode.",
      "unblock": "deploy to a Fly machine with the required Linux capabilities and toggle deputyos_apparmor_mode=enforce"
    },
    {
      "id": "no-clamd",
      "reason": "Container PID 1 is the agent; clamd as a background daemon is unreliable in this configuration. On-demand clamscan replaces it.",
      "unblock": "deploy on a non-container target (rpi5, mini-pc, DO, oracle) for persistent clamd"
    },
    {
      "id": "no-systemd",
      "reason": "Container runs the agent as PID 1; systemd is absent. deputyctl up on this target execs the agent in the foreground rather than poking systemd.",
      "unblock": "n/a — this is the documented Fly Machines behaviour. See fly/README.md."
    },
    {
      "id": "no-local-llm",
      "reason": "RAM tier 'low' (2 GB) far below local-LLM threshold; CPU is shared",
      "unblock": "fly-machines+gpu (paid, M3+) for vLLM, or pick a non-container target"
    }
  ]
}
```

Note the `apparmor-complain` and `no-systemd` limitations on
`fly-machines` — those are honest acknowledgements that the assumptions
deputyOS makes for native targets don't all hold inside a container.

## Consumers

| Caller | What it reads |
|---|---|
| `deputyctl limits` | Whole struct; renders the human-readable spec block. |
| `deputyctl doctor` | `ram_mb`, `tier` for live-vs-baked comparison. |
| Wizard step 4 (channels) | `capabilities.channels_disabled_by_ram` to grey out checkboxes. |
| PWA dashboard | `tier`, `target`, `ram_mb`, top 3 limitations. |
| Voice setup card | `voice_wake_word`, `voice_tts` to gate the toggle. |

See [Reference / CLI / deputyctl](../cli/deputyctl.md) for the
`limits` and `doctor` commands.

## See also

- [Distribution / Hardware matrix](../../distribution/hardware-matrix.md) —
  the full target list and which limits file each gets.
- [Reference / CLI / deputyctl](../cli/deputyctl.md) — `deputyctl limits`
  and `deputyctl doctor` reference.
- [Reference / Schemas / Profile manifest](profile-toml.md) —
  `[channels].supported` is intersected with
  `capabilities.channels_disabled_by_ram` at wizard time.
- [Concepts / Architecture](../../concepts/architecture.md) — how
  limits.json sits in the appliance lifecycle.
- [How-to / Add a hardware target](../../how-to/add-a-hardware-target.md) —
  authoring a new `limits.<target>.json`.
