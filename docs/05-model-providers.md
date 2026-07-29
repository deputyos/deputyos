# 05 — Model providers

The first-boot wizard collects exactly one mandatory piece of credentials: a model provider key. We support fifteen providers natively plus any OpenAI-compatible custom endpoint. The wizard validates the key with a one-token round-trip before persisting anything, so wrong keys never produce a half-configured device.

## Wizard contract

- **Input:** provider, model (default if not changed), API key (and base URL for self-hosted), optional region (Bedrock).
- **Validation:** a synthetic 1-token chat completion against the chosen model. Failure → user retries; nothing written.
- **Storage:** `/etc/deputyos/secrets.env` mode `0600`, root-owned, mounted into the agent's systemd unit via `EnvironmentFile=`. Wizard never writes to logs.
- **Rotation:** `deputyctl model set` re-runs the relevant prompts and replaces the existing entry atomically.
- **Test:** `deputyctl model test` repeats the validation round-trip on demand; non-zero exit if the configured key is no longer valid.

There is **no OAuth** in the wizard. OAuth flows on a headless device are hostile to provision; every supported provider uses an API key + base URL. See [ADR-0006](adr/0006-no-oauth-in-wizard.md).

## Supported providers

| Provider | Auth | Default model | Notes |
|---|---|---|---|
| **OpenRouter** (recommended) | API key | `anthropic/claude-sonnet-4.6` | One key, hundreds of models. Easiest first-time path. |
| Anthropic | API key | `claude-sonnet-4-6` | Direct API. Lowest latency to Claude models. |
| OpenAI | API key | `gpt-4o` | Direct API. |
| Google AI Studio | API key | `gemini-2.0-flash` | Free tier exists; the wizard surfaces this. |
| AWS Bedrock | AWS key + secret + region | `anthropic.claude-sonnet-4-6` | The only provider that needs more than one field; wizard handles it. |
| Nous Portal | API key | `Hermes-3-Llama-3.1-405B` | Native to Hermes; works with OpenClaw too. |
| NVIDIA NIM | API key | `nemotron-4-340b-instruct` | OpenAI-compatible endpoint. |
| Ollama Cloud | API key | `llama-3.3-70b` | Hosted Ollama; no local resources needed. |
| Local Ollama | URL only (no key) | `llama3.2:3b` | Wizard offers this only on offline-build images that bake in a model. |
| z.ai / GLM | API key | `glm-4.6` | OpenAI-compatible. |
| Kimi / Moonshot | API key | `moonshot-v1-128k` | Long-context strength. |
| MiniMax (global + CN) | API key | `abab6.5s-chat` | CN endpoint selectable. |
| Xiaomi MiMo | API key | `mimo-7b-rl` | RL-trained reasoning model. |
| Hugging Face Inference | API key | configurable | Specifying a `model` field is required. |
| Custom OpenAI-compatible | URL + optional key | configurable | For vLLM, llama.cpp server, SGLang, LocalAI, etc. |

## What the wizard writes

For each provider, `secrets.env` gets a small set of variables (the agent reads from a normalised set):

```bash
# Examples (only the active provider's lines exist on a given device)
AGENT_PROVIDER=openrouter
AGENT_BASE_URL=https://openrouter.ai/api/v1
AGENT_API_KEY=sk-or-v1-...
AGENT_DEFAULT_MODEL=anthropic/claude-sonnet-4.6

# Bedrock (multi-field)
AGENT_PROVIDER=bedrock
AGENT_AWS_REGION=us-east-1
AGENT_AWS_ACCESS_KEY_ID=...
AGENT_AWS_SECRET_ACCESS_KEY=...
AGENT_DEFAULT_MODEL=anthropic.claude-sonnet-4-6

# Custom OpenAI-compatible
AGENT_PROVIDER=custom
AGENT_BASE_URL=http://192.168.1.50:8000/v1
AGENT_API_KEY=    # may be empty for self-hosted
AGENT_DEFAULT_MODEL=Qwen2.5-72B-Instruct
```

Each profile's systemd unit reads `EnvironmentFile=/etc/deputyos/secrets.env` and translates the normalised variables to whatever the upstream agent expects (e.g. setting `OPENROUTER_API_KEY` for OpenClaw, `OPENAI_API_KEY` for Hermes when used against OpenRouter).

## Validation calls

Each provider has a tiny request body the wizard issues before persisting. The shape is intentionally minimal — one user message, `max_tokens=1` — so it costs essentially nothing and surfaces auth failures immediately. Pseudo-code:

```rust
let body = json!({
  "model": chosen_model,
  "messages": [{"role": "user", "content": "ping"}],
  "max_tokens": 1
});
let resp = http_post(format!("{base}/chat/completions"), api_key, body)?;
if resp.status() != 200 { return Err(...) }
```

Bedrock and Hugging Face have non-OpenAI-shape payloads but the same idea.

## Why API-key-only

We considered OAuth device-code flows for providers that support them (Google, Anthropic). The reasons we don't use them:

1. **Headless-device hostile** — the device has no browser. A device-code flow needs a phone, additional polling, and we'd have to handle revocation gracefully — extra surface for first-time users to get stuck on.
2. **Rotation surprise** — refresh tokens expire silently; users discover they're broken when their bot stops replying. API keys fail loudly on the next call.
3. **Universal abstraction** — every provider has API keys; only some have OAuth. Standardising on the lowest common denominator means the same secrets storage and the same validation pattern works for all fifteen.

The Cloudflare path *is* OAuth, but only for the storage-bucket provisioning flow (see [06-storage-and-backup.md](06-storage-and-backup.md)), and it's optional.

## Cost guardrails (M5)

Once cost telemetry from the chosen provider is plumbed in, the wizard collects:

- per-day spend cap (default off; recommended `$5`)
- per-month spend cap (default off; recommended `$50`)
- behavior on cap hit: pause / throttle / notify-only

Cap state lives in `~/.<profile>/.cost-state` and is checked on every outbound message. Trip → agent posts a "spending paused, raise cap with `deputyctl spend raise-cap`" notice on the configured notification channel.

Some providers don't expose per-call cost in headers; for those we estimate from a cached rate table updated when the manifest publishes.
