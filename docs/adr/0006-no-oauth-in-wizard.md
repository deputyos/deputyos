# ADR 0006 — No OAuth in the wizard (with one exception)

**Status:** Accepted (M0)

## Context

Some model providers (Anthropic, Google, AWS) support OAuth or device-code flows. The wizard could theoretically use these to avoid asking the user for an API key.

## Decision

**The wizard collects API keys only.** No OAuth in the model-provider flow. The single exception is the Cloudflare bucket-provisioning flow (see [06-storage-and-backup.md](../06-storage-and-backup.md)), which uses Cloudflare's device-code OAuth to mint an R2 bucket and an R2 access key for the user — and that's an *optional* convenience path; users can paste R2 or B2 credentials directly instead.

## Why

1. **Headless device, hostile flow.** The device usually has no browser. A device-code flow needs the user to bounce to a phone, copy a code, wait for polling, deal with intermittent network, and then handle the moment when refresh tokens silently expire weeks later. Each step is one more place a non-technical user gets stuck.
2. **Universal abstraction.** Every supported provider has API keys. Only some have OAuth. Standardising on the lowest common denominator means one storage scheme and one validation pattern for fifteen providers.
3. **Loud failures.** API keys fail loudly on the next call ("401 invalid API key"). Refresh-token expiry fails silently — the bot just stops responding, often hours after the actual failure. Loud is better for our user base.
4. **Audit clarity.** A user can revoke an API key from the provider's dashboard. OAuth grants are harder to enumerate and audit, especially when refresh tokens have been minted.

## The Cloudflare exception, justified

The Cloudflare bucket-provisioning OAuth flow is opt-in and replaces a worse alternative ("ask the user to find their R2 credentials in the Cloudflare dashboard"). Non-technical users routinely get stuck on R2 credential paperwork; OAuth here genuinely improves UX. The path remains optional — pasting credentials still works.

## Alternatives considered

- **Full OAuth for providers that support it.** Rejected for the reasons above. Worth re-evaluating in M7+ if we're confident we can handle expiry gracefully and we have a compelling UX for the device-code flow.
- **Browser-driven OAuth via the local web wizard.** Considered. Same headless-device problem when the user is provisioning by SSH or via a phone scan with no laptop nearby. The flow is also harder to make robust against home-network DNS quirks.

## Consequences

- The wizard's secrets-collection logic has one shape per provider, and they all collapse to "validate + write `secrets.env`". This keeps the wizard simple and auditable.
- Users who'd prefer OAuth (small minority by survey) can configure their provider key with a cap-limited, scoped-down API key from the provider's dashboard. The wizard's prompts encourage this (e.g. "use a key scoped to chat completions only").
