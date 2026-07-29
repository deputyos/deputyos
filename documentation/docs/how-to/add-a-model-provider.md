# Add a model provider

## What this guide does

Extend deputyOS's **provider catalogue** so the wizard, `deputyctl model
list`, and the upstream agent know about a new LLM API. deputyOS ships
with 15 providers baked in (OpenAI, Anthropic, OpenRouter, Bedrock,
Google AI Studio, NVIDIA NIM, Nous Portal, Ollama Cloud, Local Ollama,
z.ai/GLM, …). Adding a 16th is a one-file edit to JSON, plus a one-test
verification, plus an optional cost-defaults entry.

This is the **only** how-to guide that touches Rust crate territory —
the catalogue lives at `deputyctl/etc/providers.json`, which is baked
into the image and consumed by `deputyctl::model::load_providers`.

## Prerequisites

- A working contributor checkout — `make doctor` green, `cargo test
  --all` clean.
- The provider's published API documentation. You need:
    - Endpoint base URL (e.g. `https://api.openai.com/v1`).
    - Authentication header / scheme — almost always `Authorization:
      Bearer <key>`.
    - Request shape — chat completions, messages, generate, models.
    - Default model name.
    - Key format hint (regex-ish, for the wizard's "this looks wrong"
      check).

## The recipe

### 1. Edit `deputyctl/etc/providers.json`

Append a new entry to the `providers` array:

```json
{
  "id": "<short-slug>",
  "display_name": "<Display Name>",
  "kind": "openai-compatible",
  "endpoint_default": "https://api.example.com/v1",
  "key_env_var": "EXAMPLE_API_KEY",
  "key_format": "sk-example-...",
  "default_model": "<model-id>",
  "supported_models_hint": "<freeform; shown in `deputyctl model list`>"
}
```

### 2. Pick the right `kind`

The `kind` field tells the upstream agent which request shape to use.
The supported values today:

| Kind | Endpoint shape |
| --- | --- |
| `openai-compatible` | `POST /chat/completions` (OpenAI v1 spec) |
| `openai` | as above, plus OpenAI-specific extensions (`o1` family) |
| `anthropic` | `POST /v1/messages` (Claude spec) |
| `google` | `POST /v1beta/models/<id>:generateContent` |
| `bedrock` | AWS SigV4 + Bedrock-Runtime API |

Most new providers will be `openai-compatible` — that's the de facto
shape in 2026, and what OpenRouter, Nous Portal, NVIDIA NIM, Ollama
Cloud, Local Ollama, z.ai, and others all speak.

### 3. Pick a `key_env_var` name

Convention: `<UPPER_SNAKE>_API_KEY`. The variable is what lands in
`/etc/deputyos/secrets.env` after the wizard's provider step. The
upstream agent reads the same env var via systemd's
`EnvironmentFile=-/etc/deputyos/secrets.env`.

For providers that don't use a key (like Local Ollama, where the
"key" is just a base URL), name the var `<NAME>_BASE_URL` and set
`key_format` to `(URL only; no key required)`.

### 4. Add a cost-defaults entry (optional but recommended)

Edit `deputyctl/etc/cost-defaults.json` and add a per-1M-token rate:

```json
"<id>": {
  "default_model": "<model-id>",
  "input_per_1m_usd": <number>,
  "output_per_1m_usd": <number>
}
```

These are bootstrap defaults used when the upstream agent's
per-message ledger writeback omits `cost_usd`. The per-message ledger
is the source of truth; this file is the fallback. Update during
routine release prep — `deputyos-track` does not yet auto-bump it.

### 5. Verify

```sh
# JSON parses
jq . deputyctl/etc/providers.json

# The Rust struct round-trips
cargo test -p deputyctl model::test_provider_key

# The catalogue loads at runtime
cargo run -p deputyctl -- model list --json
```

`cargo test -p deputyctl` exercises `model::test_provider_key`, which
deserializes the catalogue and asserts every entry has well-formed
required fields. A typo in the JSON is caught here.

## Worked example: adding "Together AI"

Together AI is a hypothetical addition (not currently shipped). Its
API is OpenAI-compatible.

### `deputyctl/etc/providers.json` append

```json
{
  "id": "together-ai",
  "display_name": "Together AI",
  "kind": "openai-compatible",
  "endpoint_default": "https://api.together.xyz/v1",
  "key_env_var": "TOGETHER_API_KEY",
  "key_format": "(Together API key)",
  "default_model": "meta-llama/Llama-3.3-70B-Instruct-Turbo",
  "supported_models_hint": "Llama, Mixtral, Qwen, DeepSeek; OpenAI-compatible"
}
```

### `deputyctl/etc/cost-defaults.json` append

```json
"together-ai": {
  "default_model": "meta-llama/Llama-3.3-70B-Instruct-Turbo",
  "input_per_1m_usd": 0.88,
  "output_per_1m_usd": 0.88
}
```

### Verify

```sh
cargo test -p deputyctl
cargo run -p deputyctl -- model list --json | jq '.[] | select(.id=="together-ai")'
```

The wizard now shows Together AI in its provider step. The upstream
agent reads `$TOGETHER_API_KEY` from `secrets.env` and POSTs to
`https://api.together.xyz/v1/chat/completions`.

## Provider catalogue today

The 15 providers shipped in `deputyctl/etc/providers.json` (current
M5 list):

`openrouter`, `anthropic`, `openai`, `google-ai-studio`, `bedrock`,
`nous-portal`, `nvidia-nim`, `ollama-cloud`, `local-ollama`,
`zai-glm`, `mistral`, `groq`, `cerebras`, `deepseek`, `xai-grok`.

See [Reference → Schemas → providers.json](../reference/schemas/providers-json.md)
for the per-entry walk-through.

## Troubleshooting

!!! warning "Wizard accepts any string as 'the key'"
    `key_format` is a hint, not a regex enforcer. The wizard checks
    for length-greater-than-zero only — it cannot verify the key works
    until the user runs `deputyctl model test`. The hint just biases
    the user toward typing the right shape.

!!! warning "Provider works but cost ledger reports $0.00"
    The upstream agent didn't return `usage` tokens, and your
    `cost-defaults.json` entry is missing or zeroed. Cost is computed
    as `(input_tokens * input_per_1m_usd + output_tokens *
    output_per_1m_usd) / 1_000_000`. Either side missing → zero cost
    fallback.

!!! danger "Adding a key_env_var that collides with an existing one"
    `secrets.env` is a flat KEY=VALUE namespace. Two providers with
    the same `key_env_var` overwrite each other on `deputyctl model
    set`. Use distinct names — convention is the provider's display
    name in upper-snake.

## Related

- [Reference → Schemas → providers.json](../reference/schemas/providers-json.md)
- [Reference → CLI → deputyctl](../reference/cli/deputyctl.md) (`model` subcommand tree)
- [How-to → Rotate keys](rotate-keys.md)
- [Operations → Cost guardrails](../operations/cost-guardrails.md)
- [Security → Secrets storage](../security/secrets-storage.md)
