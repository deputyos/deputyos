# Enable voice

## What this guide does

Turn on the **voice channel** on an deputyOS device that supports it.
The voice stack is whisper.cpp (speech-to-text) + Piper (text-to-speech)
+ a thin shell-script bridge that connects ALSA audio I/O to the
existing message-relay socket the upstream agent already speaks.

Voice is **opt-in and per-target gated**. By default
`deputyos_voice_enabled` is `false`. Even when set to `true`, the role
refuses to bake voice assets onto hardware that can't run them
(rpi4 / wsl2 / cloud / macos-qemu — see the matrix below).

## Prerequisites

- An deputyOS device on a target that supports voice. Today: **rpi5
  (8GB+)**, **x86_64-mini-pc**, **arm64-generic** (with adequate RAM).
- A USB or built-in microphone the kernel sees as an ALSA device.
- A speaker (or headphones) on the same ALSA card.
- For TTS: at least 8GB RAM headroom (Piper's voice models are
  ~60-130MB, decoded into roughly 4× that working set).
- The wizard finished and the active profile running.

## Per-target voice support

| Target | Wake word | TTS | Notes |
| --- | --- | --- | --- |
| rpi5 (8GB+) | yes | yes | recommended; `whisper-tiny.en` |
| rpi5 (4GB) | no | no | RAM headroom too tight |
| rpi4 | no | no | Cortex-A72 too slow, passive cooling thermal-throttles |
| arm64-generic | yes (8GB+) | yes (8GB+) | depends on board |
| x86_64-mini-pc | yes | yes | recommended; `whisper-base.en` or `-small.en` |
| qemu-aarch64 / qemu-x86_64 | no | no | emulation makes wake-word unworkably slow |
| wsl2 | no | no | Windows audio routing not wired |
| macos-qemu | no | no | host audio not pumped through; UTM CoreAudio is its own project |
| digitalocean / oracle / hetzner / vultr / linode | no | no | cloud — no audio device |
| fly-machines | no | no | container — no audio device |

The role's gate is in `roles/deputyos/tasks/voice-baseline.yml`:

```yaml
- name: Decide whether to bake voice assets
  ansible.builtin.set_fact:
    deputyos_voice_should_bake: >-
      {{ (deputyos_voice_enabled | bool)
         and (deputyos_hw not in deputyos_no_voice_hw) }}
```

`deputyos_no_voice_hw` is `[rpi4, wsl2, macos-qemu, digitalocean,
oracle-arm-free, hetzner-cloud, vultr, linode, fly-machines]`.

## The recipe

### 1. Bake an image with voice enabled

Voice is a **bake-time** decision (the binaries and model weights ship
in the image). Two paths:

- **Recommended**: Set the role fact via Packer extra-vars at bake.

    ```sh
    make build TARGET=rpi5 PROFILE=openclaw \
      DEPUTYOS_PACKER_EXTRA_VARS='-var "deputyos_voice_enabled=true"'
    ```

- Or set via environment when invoking `scripts/build.sh` directly.

The role downloads (during the staging step in `scripts/build.sh`):

- `ggml-<model>.bin` from Hugging Face (`whisper.cpp` mirror).
- `piper_linux_<arch>.tar.gz` from Piper's GitHub releases.
- `<voice>.onnx` + `<voice>.onnx.json` from Piper's voice repo.

Set `DEPUTYOS_VOICE_OFFLINE=1` to skip the downloads (air-gapped CI).
The role tolerates missing assets and refuses to start the unit until
real binaries land.

### 2. Pick STT and TTS models

Whisper STT model — driven by `DEPUTYOS_WHISPER_MODEL`:

| Model | Size | RAM target | Recommended for |
| --- | --- | --- | --- |
| `tiny.en` | 39 MB | 8GB | Pi 5 8GB |
| `base.en` | 142 MB | 16GB | Pi 5 16GB / x86_64-mini-pc |
| `small.en` | 466 MB | 16GB+ | x86_64-mini-pc |

Piper TTS voice — driven by `DEPUTYOS_PIPER_VOICE`:

- `en_US-amy-medium` (default; ~70MB).
- `en_US-libritts_r-medium` (better quality; ~130MB).
- See [Piper voices](https://github.com/rhasspy/piper) for the full
  catalogue.

### 3. After flashing, edit `/etc/deputyos/voice.toml` (optional)

The role drops a default `voice.toml` at bake. Tunables:

```toml
[stt]
model_path = "/opt/deputyos/voice/whisper-tiny.en.bin"
wake_word  = "agent"          # substring match against whisper-cli's first token
sample_rate = 16000

[tts]
model_path = "/opt/deputyos/voice/en_US-amy-medium.onnx"
config_path = "/opt/deputyos/voice/en_US-amy-medium.onnx.json"

[audio]
input_device  = "default"      # ALSA device name; `arecord -L` to list
output_device = "default"      # ALSA device name; `aplay -L` to list
```

Wake-word detection is a **substring match** against whisper-cli's
first emitted token, intentionally simple — the goal is "user says
'agent, what's the weather'" not Alexa-grade always-on KWS.

### 4. Start the voice relay

Voice is a separate systemd unit, gated on the asset presence:

```sh
sudo systemctl enable --now deputyos-voice-relay.service
```

The unit lives at `/etc/systemd/system/deputyos-voice-relay.service`,
hardened similarly to the gateway unit (see
[Reference → System → systemd units](../reference/system/systemd-units.md)),
runs as `agent`, and confines via the `deputyos.voice-relay` AppArmor
profile.

## Verification

```sh
# 1. Unit is active
sudo systemctl status deputyos-voice-relay.service

# 2. Logs are healthy
sudo journalctl -u deputyos-voice-relay -f

# 3. Talk to it: whisper-cli prints recognized text to the journal
arecord -d 5 -f S16_LE -r 16000 test.wav
sudo /opt/deputyos/voice/whisper-cli -m /opt/deputyos/voice/whisper-tiny.en.bin \
  -f test.wav --output-json | jq

# 4. TTS round-trip: pipe text into Piper, play through speakers
echo "hello from deputyos" | sudo /opt/deputyos/voice/piper \
  --model /opt/deputyos/voice/en_US-amy-medium.onnx \
  --output_raw | aplay -r 22050 -f S16_LE
```

## Worked example: enabling voice on rpi5 8GB

```sh
# 1. Bake with voice.
make build TARGET=rpi5 PROFILE=openclaw \
  DEPUTYOS_WHISPER_MODEL=tiny.en \
  DEPUTYOS_PIPER_VOICE=en_US-amy-medium \
  DEPUTYOS_PACKER_EXTRA_VARS='-var "deputyos_voice_enabled=true"'

# 2. Flash, boot, finish wizard.
# 3. SSH in.
ssh agent@deputyos.local

# 4. Verify
sudo systemctl status deputyos-voice-relay
arecord -L | head    # confirm mic is visible
aplay -L | head      # confirm speaker is visible

# 5. Say the wake word + a question. Tail the journal.
sudo journalctl -u deputyos-voice-relay -f
```

## Troubleshooting

!!! warning "Unit is enabled but refuses to start"
    Most common cause: a missing voice asset. The unit's
    `ConditionPathExists=` gates check for whisper-cli, the model bin,
    and the Piper binary. Run `ls /opt/deputyos/voice/` and compare
    with `/opt/deputyos/voice/MANIFEST` (written at bake time). Re-bake
    with network connectivity if assets are missing.

!!! warning "ALSA can't find the mic"
    `arecord -L` lists the kernel's view. If your USB mic shows up but
    `default` doesn't route to it, edit `/etc/asound.conf` (or the
    user-level `~/.asoundrc`) to set the default. The voice config's
    `input_device = "default"` follows whatever ALSA's default is.

!!! warning "Wake word fires constantly on background noise"
    The substring-match wake-word is intentionally permissive. If your
    environment has a lot of speech (TV, music with vocals), pick a
    less-likely wake word (a made-up name) and update
    `voice.toml`. The whole stack restarts on edit:
    `systemctl restart deputyos-voice-relay`.

!!! danger "Voice is on but the agent doesn't respond to speech"
    Almost always the relay socket is stale. The voice-relay shell
    script speaks to `/run/deputyos/relay.sock`; if the gateway unit
    is down, that socket doesn't exist. Restart the gateway, then the
    voice relay, in that order.

!!! tip "Add a per-target audio device default"
    rpi5 ships with both HDMI audio and (optionally) a USB DAC. Pin
    the right one in `voice.toml` rather than relying on `default`,
    which can change after every reboot.

## Related

- [Reference → System → systemd units](../reference/system/systemd-units.md) (the `deputyos-voice-relay` unit)
- [Reference → System → AppArmor profiles](../reference/system/apparmor-profiles.md) (the voice-relay profile)
- [Reference → APIs → message relay](../reference/apis/message-relay.md)
- [Reference → Schemas → limits.json](../reference/schemas/limits-json.md) (the per-target voice gates)
- [Distribution → Hardware matrix](../distribution/hardware-matrix.md)
