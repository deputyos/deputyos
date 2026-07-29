# Air-gapped image builds

```bash
make build TARGET=<hw> PROFILE=<id> TIER=<tier> AIRGAP=1
```

The `AIRGAP=1` flag composes with the existing `TIER` and `TARGET` knobs.
Every supported (target × tier) tuple gets an airgap variant.

## Pre-bake host requirements

The Packer host needs the LFM2 GGUFs cached locally. `scripts/build.sh`
fetches and verifies SHAs once into `build/staging/llm/` before invoking
Ansible.

| Tier     | GGUF                                                  | SHA256 (pinned in `roles/deputyos/vars/llm-airgap.yml`) |
|----------|-------------------------------------------------------|--------------------------------------------------------|
| lean     | `LFM2-350M-Q4_K_M.gguf`                               | (pinned)                                               |
| standard | `LFM2-1.2B-Q4_K_M.gguf`                               | (pinned)                                               |
| rich     | `LFM2-2.6B-Q4_K_M.gguf` + `Qwen2.5-Coder-1.5B-Instruct-Q4_K_M.gguf` | (pinned, both)                            |

`vars/llm-airgap.yml` pins each model to the SHA-256 recorded by its
authoritative Hugging Face LFS object. The role refuses to bake an image whose
downloaded model does not match.

## What `AIRGAP=1` adds vs. a normal build

| Layer    | Normal build                              | `AIRGAP=1`                                                                            |
|----------|-------------------------------------------|---------------------------------------------------------------------------------------|
| apt      | `https://deb.debian.org/`                 | `file:///opt/deputyos/airgap/apt-mirror/`                                              |
| egress   | `ufw default deny`, allow-list per channel| nftables policy `mode=airgap`; only RFC1918 + mDNS + local DNS escape                 |
| LLM      | optional Ollama / cloud API               | LFM2 GGUF baked + `deputyos-llamacpp@<id>.service` enabled                             |
| `deputyctl model list` | catalog from network        | catalog from `/opt/deputyos/airgap/models/catalog.json`; no network calls allowed      |
| `/etc/deputyos/limits.json` | `airgap_supported: false` | `airgap_supported: true`                                                              |

## Verifying

```bash
qemu-system-x86_64 \
  -nodefaults -nographic \
  -drive file=build/qemu-x86_64-openclaw.qcow2,if=virtio \
  -m 4G -smp 2 \
  -netdev user,id=u0 -device virtio-net,netdev=u0 \
  -net none                                  # <-- the smoking gun
```

Wizard at `http://deputyos.local:8088/` should still come up; built-in
chat at `/chat` should still answer (slowly on lean; happily on rich).
