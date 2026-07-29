# Provider catalogue (`providers.json`, `cost-defaults.json`, `voice.toml`)

`/etc/deputyos/providers.json` is the **bake-time catalogue of model
providers** the wizard offers. Each entry is everything the runtime
needs to test a key, render a help string, look up an env var, and
present pricing.

This page also covers the two related runtime config files:
`/etc/deputyos/cost-defaults.json` (per-provider USD rate sheet used by
the cost ledger when the per-message provider response omits cost) and
`/etc/deputyos/voice.toml` (voice-relay runtime config).

The Rust struct is in
[`deputyctl/src/model.rs`](https://github.com/deputyos/deputyos/blob/main/deputyctl/src/model.rs);
the catalogue source is `deputyctl/etc/providers.json`.

[TOC]

## `providers.json` — schema

```rust
struct ProvidersFile { providers: Vec<Provider> }

struct Provider {
    id: String,                   // stable identifier, kebab-case
    display_name: String,         // human-readable
    kind: String,                 // dispatch tag for the test path
    endpoint_default: String,     // canonical base URL (may be empty)
    key_env_var: String,          // env var name in /etc/deputyos/secrets.env
    key_format: String,           // hint string shown in the wizard
    default_model: String,        // optional, default ""
    supported_models_hint: String,// optional, default ""
}
```

| Field | Type | Required | Description |
|---|---|---|---|
| `id` | string | yes | Stable provider id, kebab-case (e.g. `openrouter`, `anthropic`, `google-ai-studio`). Used by `deputyctl model set --provider <id>` and the wizard `<select>`. |
| `display_name` | string | yes | Human-readable name shown in the wizard and PWA. |
| `kind` | string | yes | Dispatch tag for the test path. One of: `openai-compatible`, `openai`, `anthropic`, `google`, `bedrock`, `huggingface`. |
| `endpoint_default` | string | yes | Canonical base URL. May be empty for self-hosted (Custom OpenAI-compatible). |
| `key_env_var` | string | yes | Name of the `KEY=VALUE` line written into `/etc/deputyos/secrets.env`. |
| `key_format` | string | yes | Hint shown next to the input box (e.g. `sk-or-v1-...`, `AIza...`, `(URL only; no key required)`). |
| `default_model` | string | optional | Suggested default model id. May be empty for providers requiring per-call selection. |
| `supported_models_hint` | string | optional | Free-form hint surfaced in the wizard. |

## Resolution order

`deputyctl/src/paths.rs::providers_file()`:

1. `$DEPUTYOS_PROVIDERS_FILE` env var if set.
2. `/etc/deputyos/providers.json` if it exists.
3. `deputyctl/etc/providers.json` (dev fallback).

## `kind` enum values

| Kind | Test path | Notes |
|---|---|---|
| `openai-compatible` | GET `<endpoint>/models` with `Authorization: Bearer <key>`. | The bulk of providers — anything that speaks the OpenAI Chat Completions API on `/v1/...`. |
| `openai` | Same as above, against api.openai.com. | Distinguished only for cost-defaults bookkeeping. |
| `anthropic` | GET `<endpoint>/v1/models` with `x-api-key: <key>` and `anthropic-version` header. | |
| `google` | GET `<endpoint>/models?key=<key>`. | Google AI Studio and equivalents. |
| `bedrock` | AWS SigV4 hand-shake against `<endpoint>`. Requires `AWS_ACCESS_KEY_ID`, `AWS_SECRET_ACCESS_KEY`, `AWS_REGION`. | The wizard detects all three before validating. |
| `huggingface` | GET `<endpoint>/models` with bearer token. | The wizard requires a model id. |

The wizard's "Skip validation" checkbox bypasses the round-trip and
trusts the user. `deputyctl model test` runs the same path post-apply.

## All 15 transcribed providers

Rendered from `deputyctl/etc/providers.json` — the source of truth. Some
columns trimmed for fit; full detail lives in the JSON file.

| id | display_name | kind | endpoint_default | key_env_var | key_format hint | default_model |
|---|---|---|---|---|---|---|
| `openrouter` | OpenRouter | openai-compatible | `https://openrouter.ai/api/v1` | `OPENROUTER_API_KEY` | `sk-or-v1-...` | `anthropic/claude-sonnet-4.6` |
| `anthropic` | Anthropic | anthropic | `https://api.anthropic.com` | `ANTHROPIC_API_KEY` | `sk-ant-...` | `claude-sonnet-4-6` |
| `openai` | OpenAI | openai | `https://api.openai.com/v1` | `OPENAI_API_KEY` | `sk-...` | `gpt-4o` |
| `google-ai-studio` | Google AI Studio | google | `https://generativelanguage.googleapis.com/v1beta` | `GOOGLE_AI_STUDIO_API_KEY` | `AIza...` | `gemini-2.0-flash` |
| `bedrock` | AWS Bedrock | bedrock | `https://bedrock-runtime.us-east-1.amazonaws.com` | `AWS_ACCESS_KEY_ID` | `AKIA...` (+ secret + region) | `anthropic.claude-sonnet-4-6` |
| `nous-portal` | Nous Portal | openai-compatible | `https://inference-api.nousresearch.com/v1` | `NOUS_PORTAL_API_KEY` | `sk-nous-...` | `Hermes-3-Llama-3.1-405B` |
| `nvidia-nim` | NVIDIA NIM | openai-compatible | `https://integrate.api.nvidia.com/v1` | `NVIDIA_NIM_API_KEY` | `nvapi-...` | `nemotron-4-340b-instruct` |
| `ollama-cloud` | Ollama Cloud | openai-compatible | `https://api.ollama.com/v1` | `OLLAMA_CLOUD_API_KEY` | `ollama-...` | `llama-3.3-70b` |
| `local-ollama` | Local Ollama | openai-compatible | `http://127.0.0.1:11434/v1` | `LOCAL_OLLAMA_BASE_URL` | URL only; no key required | `llama3.2:3b` |
| `zai-glm` | z.ai / GLM | openai-compatible | `https://api.z.ai/api/paas/v4` | `ZAI_API_KEY` | (z.ai key) | `glm-4.6` |
| `kimi-moonshot` | Kimi / Moonshot | openai-compatible | `https://api.moonshot.cn/v1` | `MOONSHOT_API_KEY` | `sk-...` | `moonshot-v1-128k` |
| `minimax` | MiniMax | openai-compatible | `https://api.minimaxi.com/v1` | `MINIMAX_API_KEY` | (MiniMax key) | `abab6.5s-chat` |
| `xiaomi-mimo` | Xiaomi MiMo | openai-compatible | `https://api.mimo.xiaomi.com/v1` | `XIAOMI_MIMO_API_KEY` | (Xiaomi key) | `mimo-7b-rl` |
| `huggingface` | Hugging Face Inference | huggingface | `https://api-inference.huggingface.co` | `HUGGINGFACE_API_KEY` | `hf_...` | (model required) |
| `custom` | Custom OpenAI-compatible | openai-compatible | (empty) | `CUSTOM_OPENAI_API_KEY` | may be empty | (user-set) |

`local-ollama` is offered only on offline-build images that bake in a
model — the wizard hides it on online-build images so users don't pick
a provider that won't work.

## The `key_env_var` contract

When the wizard finishes, `deputyctl model set` writes
`/etc/deputyos/secrets.env` (mode `0600`, owned by `root:agent` so the
gateway services that `EnvironmentFile=-/etc/deputyos/secrets.env` can
read it). The line is exactly `<key_env_var>=<value>` — no quoting,
no comments. `deputyctl model list` parses it back via
`model::load_secrets_from`, which tolerates lines without `=`,
`#`-comments, and one optional layer of surrounding `"` or `'`.

See [Security / Secrets storage](../../security/secrets-storage.md)
for the file mode, ownership, and factory-reset behaviour.

## `cost-defaults.json` — per-provider rate sheet

`/etc/deputyos/cost-defaults.json` is the bootstrap rate sheet the cost
ledger uses when a provider's per-message response omits `cost_usd`.
It is **not** authoritative — the per-message ledger writeback is —
and is updated during routine release prep against each provider's
public pricing page.

### Schema

```json
{
  "providers": {
    "<provider-id>": {
      "default_model": "<id>",
      "input_per_1m_usd":  <number>,
      "output_per_1m_usd": <number>
    }
  }
}
```

### Full file

```json
{
  "_comment": "Default per-1M-token rates per provider (USD). Sourced from each provider's public pricing as of 2026-04-27. NOT authoritative — used only as a sane bootstrap when the agent profile's ledger entry omits cost_usd. Update during routine release prep; the per-message ledger writeback is the source of truth.",
  "providers": {
    "openrouter": {
      "default_model": "anthropic/claude-sonnet-4-6",
      "input_per_1m_usd": 3.00,
      "output_per_1m_usd": 15.00
    },
    "anthropic": {
      "default_model": "claude-sonnet-4-6",
      "input_per_1m_usd": 3.00,
      "output_per_1m_usd": 15.00
    },
    "openai": {
      "default_model": "gpt-4o",
      "input_per_1m_usd": 2.50,
      "output_per_1m_usd": 10.00
    },
    "google-ai-studio": {
      "default_model": "gemini-2.0-flash",
      "input_per_1m_usd": 0.10,
      "output_per_1m_usd": 0.40
    },
    "mistral": {
      "default_model": "mistral-large-latest",
      "input_per_1m_usd": 2.00,
      "output_per_1m_usd": 6.00
    }
  }
}
```

Providers absent from `cost-defaults.json` default to zero cost; the
ledger marks those messages as `cost_unknown=true` so the daily
report doesn't silently understate spend.

See [Operations / Cost guardrails](../../operations/cost-guardrails.md)
for the ledger and budget-threshold semantics.

## `voice.toml` — voice-relay runtime config

`/etc/deputyos/voice.toml` is the runtime source of truth for the
deputyOS voice relay (whisper.cpp + Piper bridge). It is rendered from
`roles/deputyos/templates/voice.toml.j2` at bake time and lives under
`/etc/deputyos/voice.toml`. Voice config does **not** live in the
profile manifest — manifests are M0-frozen and voice arrived at M6.

The systemd unit
[`deputyos-voice-relay.service`](../system/systemd-units.md#deputyos-voice-relayservice)
refuses to start until both `/etc/deputyos/voice.toml` exists and
`enabled = true`.

### Schema

| Section | Field | Type | Description |
|---|---|---|---|
| `[voice]` | `enabled` | bool | Master switch. Wizard / PWA voice card flips this to true after consent + mic check. |
| `[voice]` | `wake_word` | string | Case-insensitive substring match against the first token of each whisper transcription. |
| `[voice]` | `stt_model` | string | One of `whisper-tiny.en`, `whisper-base.en`, `whisper-small.en`. |
| `[voice]` | `tts_voice` | string | Piper voice id (e.g. `en_US-amy-medium`). |
| `[voice]` | `audio_device` | string | ALSA device. `default` routes through the user's default soundcard; `plughw:1,0` etc. for explicit cards. |
| `[stt]` | `model_path` | string | Absolute path to the whisper model `.bin`. |
| `[stt]` | `binary` | string | Absolute path to `whisper-cli`. |
| `[tts]` | `model_path` | string | Absolute path to the Piper `.onnx`. |
| `[tts]` | `binary` | string | Absolute path to `piper`. |
| `[relay]` | `socket_path` | string | Where to talk to the message-relay socket. Default `/run/deputyos/relay.sock`. |
| `[relay]` | `hook_kind` | string | The HookKind to fire — convention is `pre-message`. |
| `[relay]` | `source_tag` | string | Payload `source` field — convention is `voice`. |

### Sizing per device class

Per `roles/deputyos/templates/voice.toml.j2`:

- **rpi5 8GB**: `whisper-tiny.en` (default).
- **rpi5 16GB**: `whisper-base.en`.
- **x86_64-mini-pc**: `whisper-small.en`.

Pi 4, qemu-aarch64, and all cloud targets disable voice entirely — see
the corresponding [limits.json](limits-json.md) entries.

### Example rendered file

```toml
[voice]
enabled      = true
wake_word    = "agent"
stt_model    = "whisper-tiny.en"
tts_voice    = "en_US-amy-medium"
audio_device = "default"

[stt]
model_path = "/opt/deputyos/voice/whisper-tiny.en.bin"
binary     = "/opt/deputyos/voice/whisper-cli"

[tts]
model_path = "/opt/deputyos/voice/piper/en_US-amy-medium.onnx"
binary     = "/opt/deputyos/voice/piper"

[relay]
socket_path = "/run/deputyos/relay.sock"
hook_kind   = "pre-message"
source_tag  = "voice"
```

## See also

- [How-to / Add a model provider](../../how-to/add-a-model-provider.md) —
  the recipe for extending `providers.json`.
- [How-to / Rotate keys](../../how-to/rotate-keys.md) — moving an
  existing provider's key to a new value.
- [How-to / Enable voice](../../how-to/enable-voice.md) — flipping
  `voice.toml`'s master switch.
- [Reference / System / systemd units](../system/systemd-units.md) —
  `deputyos-voice-relay.service` reads `voice.toml`.
- [Reference / System / AppArmor profiles](../system/apparmor-profiles.md) —
  `deputyos.voice-relay` confines the bridge.
- [Operations / Cost guardrails](../../operations/cost-guardrails.md) —
  how `cost-defaults.json` interacts with the ledger.
- [Reference / APIs / wizard HTTP](../apis/wizard-http.md) — the
  step-3 provider page that consumes `providers.json`.
