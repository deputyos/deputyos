# The Profile Class

A *profile* in deputyOS is a specific kind of thing — not any
agent-shaped piece of software. This page states the rule, gives the
fitting and non-fitting examples, and explains why the rule exists.

## The rule

A profile in deputyOS **must** be a personal AI assistant of the
OpenClaw / Hermes shape:

- **Multi-channel gateway.** The assistant accepts inbound messages from
  multiple text-message-driven channels — Telegram, Slack, Discord,
  Matrix, WhatsApp, Signal, IRC, email, web chat, and so on. The agent's
  primary surface is a conversation, not an editor or a dashboard.
- **Persistent memory across conversations.** The agent retains state
  beyond a single chat turn. SQLite, FTS5, embedded vector stores, plain
  files in a data directory — the storage choice is up to the profile;
  the *property* is required.
- **Skill / tool system the agent can invoke.** The agent calls tools or
  loads skills to take actions, search the web, run code, query
  documents, etc. A pure passthrough to an LLM API is not a profile.

A profile **must not** be:

- An IDE coding agent — Aider, Continue.dev, Cursor and the like belong
  as IDE plugins. The shape is wrong: the surface is the editor, not a
  conversation; persistence is the codebase, not a memory store; tools
  are language-server actions, not channel handlers.
- An agent framework — AutoGen, LangGraph, LangChain, CrewAI and the
  like are libraries for building agents, not appliances. They have no
  persistent identity, no first-boot UX, no channels of their own.
- An ecosystem integration shim — Home Assistant, OpenHAB, MISP, n8n,
  and the like are platforms in their own right. They should *consume*
  deputyOS (e.g. a Home Assistant integration that talks to an deputyOS
  device on the LAN), not be a profile inside it.

The rule is restated in [`CONTRIBUTING.md`](https://github.com/deputyos/deputyos/blob/main/CONTRIBUTING.md#profile-class)
in the repo. PRs that propose profiles outside this class get a polite
redirection rather than a merge.

## Flagship profiles

OpenClaw and Hermes Agent are the two profiles deputyOS is built around.
They are first-class supported, smoke-tested on every release, featured
in the picker, and the default options offered in the wizard.

### OpenClaw

[OpenClaw](https://github.com/openclaw/openclaw) is a Node.js personal
assistant with broad messaging-channel support — Telegram, Slack,
Discord, WhatsApp, Signal, iMessage / BlueBubbles, IRC, Matrix, Feishu,
Line, Mattermost, Nextcloud Talk, Nostr, Synology Chat, Tlon, Twitch,
Zalo, WeChat, QQ, Google Chat, Microsoft Teams, and a built-in web chat.
It keeps session state in SQLite under `~/.openclaw/`. It exposes a
skill API the agent uses for actions.

Manifest: [`profiles/openclaw.toml`](https://github.com/deputyos/deputyos/blob/main/profiles/openclaw.toml).
Rationale (from the manifest): multi-channel gateway, persistent memory,
skill system. Three for three.

### Hermes

[Hermes Agent](https://github.com/NousResearch/hermes-agent) is a
Python self-improving agent. It serves Telegram, Discord, Slack,
WhatsApp, Signal, DingTalk, Twilio SMS, Mattermost, Matrix, generic
webhook, IMAP/SMTP email, Home Assistant, Feishu, WeCom, and iMessage.
Memory is an FTS5 SQLite session store. The skill system is
self-modifying — Hermes can write new skills at runtime under an
unprivileged-userns sandbox, which is why
[`profiles/hermes.toml`](https://github.com/deputyos/deputyos/blob/main/profiles/hermes.toml)
declares `kernel.unprivileged_userns_clone = "1"` as a required sysctl.

Three for three.

## Community profile (validates the plugin model)

### Khoj

[Khoj](https://github.com/khoj-ai/khoj) is a Python assistant with
agent personas, document Q&A, online search, code, and image tools.
Channels: web, Telegram, WhatsApp (via Twilio), Obsidian, Emacs,
desktop client. Memory is SQLite + an embedded vector store under
`~/.khoj/`. The skill system is agent-personas-with-tools.

Manifest: [`profiles/khoj.toml`](https://github.com/deputyos/deputyos/blob/main/profiles/khoj.toml).
Three for three.

Khoj is **not** a flagship — it ships to prove the plugin model works.
It landed as a manifest plus six bake artefacts and zero Rust changes
(see [Plugin model](plugin-model.md) for the M7 acceptance test).
Future community profiles follow the same pattern: a manifest, a bake
recipe, an AppArmor profile, a systemd unit, a stub fallback, and one
line in `tasks/main.yml`. Community profiles get smoke-tested when
they land but are not gated on every release the way flagships are.

## Examples that don't fit (and why)

### Aider, Continue.dev, Cursor — IDE coding agents

These tools belong inside an editor. The natural interaction surface is
the codebase, not a conversation. Their state is the diff they're
producing; they don't want a multi-channel gateway because the editor
*is* the channel. Putting them in deputyOS would require shoehorning them
into a shape they were not designed for — and would still not give the
user what they want, which is "an AI in my IDE."

### AutoGen, LangGraph, CrewAI, LangChain — agent frameworks

These are libraries you build with, not products you install. They have
no first-boot UX of their own, no channels of their own, no persistent
identity that survives a process restart. Profiles built *with* these
frameworks are fine — Hermes is in fact a library-built agent — but the
framework itself is not a profile.

### Home Assistant, OpenHAB, n8n — ecosystem platforms

These are platforms in their own right with rich UIs, plugin ecosystems,
and operational expectations. The right relationship is "Home Assistant
talks to an deputyOS-hosted assistant over the network," not "Home
Assistant runs as a profile on deputyOS." If we made Home Assistant a
profile we would inherit its scope creep — automations, dashboards,
add-ons, HACS — none of which is a personal AI assistant.

### Pure LLM clients — Ollama Web UI, OpenWebUI, LibreChat

These are chat clients that talk to an LLM endpoint. They don't have
multi-channel gateways (browser only), they don't have a persistent
memory beyond chat history, and they don't have a tool system the agent
calls — the user calls the tools by typing. They are perfectly good
software in their category; that category isn't "personal AI assistant
appliance."

### Coding sandboxes, RPA tools, scrapers

If the primary user interaction is "click a button, watch the agent do
a thing in a browser," it's a different category. deputyOS profiles are
conversational — the user talks, the agent answers and acts.

## Why the rule

Three reasons:

1. **Coherence of the appliance shape.** deputyOS is a single-purpose
   appliance: "a personal AI assistant on a Pi / mini-PC / cloud VM."
   That sentence sets every operational expectation — the wizard, the
   PWA, the cost guardrails, the channel allowlist, the message relay,
   the limits surface, the AppArmor profile shape. Stretching the
   definition stretches all of those, and the result is a worse
   experience for everything.
2. **Reviewability.** The profile-class rule lets a reviewer say
   "yes / no" to a profile PR in minutes. Without it every proposal
   becomes an architecture argument. With it, the question is concrete
   and answerable: does this thing have a multi-channel gateway, does
   it persist memory, does it have a skill system?
3. **Trust at the picker.** A user looking at deputyos.com sees three
   profiles today and might see ten next year. Every one of them is
   the same kind of thing. The user can choose by upstream and by
   feature set, not by "wait, is *this* one even an assistant?"

The rule is intentionally narrow. It is not "any agent." It is "the
OpenClaw / Hermes shape." If your project does not fit and you
think we got it wrong, open a discussion issue first — the
[CONTRIBUTING guide](../contributing/overview.md) has the path.

## What you can do with a fitting profile

Once a profile fits, the [plugin model](plugin-model.md) gives it:

- A baked place under `/opt/deputyos/profiles/<id>/` (no install at boot).
- A first-boot wizard step driven by the manifest's `[wizard].prompts`.
- An AppArmor profile baked at `/etc/apparmor.d/deputyos.<id>`.
- A systemd unit started by [`deputyctl up`](../reference/cli/deputyctl.md).
- Channel toggles surfaced through the [PWA](../reference/apis/pwa-http.md).
- Hooks routed through the [message relay](../reference/apis/message-relay.md).
- Cost ledger entries tied to model calls (see
  [Operations → Cost guardrails](../operations/cost-guardrails.md)).
- Backup/restore of the data directory via
  [`deputyctl backup`](../reference/cli/deputyctl.md).
- Update / rollback / factory-reset semantics inherited automatically.

A profile gets all of this without writing any deputyOS-specific code —
just a manifest, a bake recipe, an AppArmor profile, a systemd unit
template, and an offline-bake fallback stub.

The practical recipe lives in [How-to → Add a profile](../how-to/add-a-profile.md).

## Where to go next

- [Concepts → Plugin model](plugin-model.md) — the contract that makes a
  profile a drop-in.
- [How-to → Add a profile](../how-to/add-a-profile.md) — the worked
  recipe, six files, no Rust changes.
- [Reference → Schemas → profile.toml](../reference/schemas/profile-toml.md)
  — every field of the manifest.
- [Contributing → Overview](../contributing/overview.md) — how a profile
  PR moves through review.
