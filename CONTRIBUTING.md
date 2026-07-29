# Contributing to deputyOS

Thanks for considering a contribution. The fastest way to get a change merged is to read this doc once before opening a PR.

## Before you start

- Open an issue first for anything larger than a typo or a clearly-scoped bug fix. Architectural changes need agreement on the approach before code lands.
- Read the relevant ADRs in [docs/adr/](docs/adr/). They're short. They tell you why the obvious-looking alternative isn't what we picked.
- The single load-bearing constraint is **zero first-boot network installs** ([ADR-0002](docs/adr/0002-zero-first-boot-network-installs.md)). Any change that violates it will be rejected.

## Local development (the contributor loop)

You don't need a Pi. You need a laptop:

```sh
git clone https://github.com/deputyos/deputyos.git
cd deputyos
make doctor                              # checks tools; tells you what to install
make try TARGET=qemu-aarch64             # ~12 min on a fresh checkout
# wizard at http://localhost:8088
```

The same Ansible role CI uses runs on your laptop. macOS (Apple Silicon + Intel), Windows (WSL2), Linux (x86_64 + arm64) are all first-class build hosts. Full details and per-host specifics in [docs/15-local-build.md](docs/15-local-build.md).

Iteration loop: edit → `make build TARGET=qemu-aarch64` → `make try` → `make smoke` → PR.

## What we accept

- New profile manifests (subject to the profile-class rule below).
- New hardware variants (PR adds `roles/deputyos/tasks/variant-<hw>.yml` + a Packer template + a CI matrix row).
- Bug fixes to `deputyctl`, the wizard, or the build pipeline.
- Doc improvements — typos, clarifications, accuracy fixes.
- New ADRs documenting architectural decisions we missed.

## What we don't accept

### Profile class

A profile **must** be a personal AI assistant of the OpenClaw / Hermes shape:

- multi-channel gateway (Telegram, Slack, Discord, etc. — text-message-driven)
- persistent memory across conversations
- skill / tool system the agent can invoke

A profile **must not** be:

- an IDE coding agent (Aider, Continue.dev) — those belong as IDE plugins
- an agent framework (AutoGen, LangGraph, LangChain) — those are libraries, not appliances
- an integration shim for another ecosystem (Home Assistant) — those should consume deputyOS, not become a profile inside it

PRs adding profiles outside this class will get a polite redirection, not a merge. If you think your project belongs and we got it wrong, open a discussion issue first.

### Other rejections

- Changes that re-introduce first-boot network installs.
- New runtime dependencies that don't have ARM64 + x86_64 prebuilt binaries.
- Features that expose the agent to the public internet by default. (Opt-in is fine.)
- Code that bypasses signature verification on the update path.

## Profile contributions — the recipe

To add a new profile:

1. **Bake recipe** — `roles/deputyos/tasks/profile-<id>.yml` lays the agent down at `/opt/deputyos/profiles/<id>/` *at build time*. Pre-resolve all native modules and Python wheels into the offline cache.
2. **systemd unit template** — `templates/<id>.service.j2`.
3. **AppArmor profile** — `apparmor/<id>` with the right `r/w/ix` rules for the agent's data dir, install root, and any sockets.
4. **Manifest** — `profiles/<id>.toml` matching the schema in [docs/02-profiles.md](docs/02-profiles.md).
5. **CI matrix entry** — add a row to `.github/workflows/build.yml`.
6. **Docs** — link from the README and add provider-specific notes to [docs/05-model-providers.md](docs/05-model-providers.md) if relevant.

No Rust changes should be needed. If you find yourself wanting to change `deputyctl`, raise it in your PR description — we may need to extend the manifest schema rather than special-case the profile in code.

## Hardware target contributions — the recipe

1. `roles/deputyos/tasks/variant-<hw>.yml` (or `variant-<cloud>.yml`) gated by `when: hw == "..."`.
2. `packer/<hw>.pkr.hcl` (or a cloud-init recipe under `cloud-init/<hw>.yaml`).
3. CI matrix row in `.github/workflows/build.yml`.
4. Smoke-test fixture under `test/qemu/<hw>.cloudinit.yaml`.
5. Per-target install-path note in `docs/01-getting-started.md`.

## Code style

- Rust: `rustfmt` defaults, `clippy --all-targets --all-features` clean before merge.
- Ansible: `ansible-lint` clean, YAML 2-space indent, no shell where a module exists.
- Markdown: no trailing whitespace, hard-wrap at 100 cols inside paragraphs of prose.
- Commit messages: imperative mood. PR titles short and specific.

## Tests

- Rust: every public function in `deputyctl` has a unit test or a justification comment.
- Image: changes that affect what's in the image must update the QEMU smoke-test fixture if the new behaviour is observable.
- Provider library: every new model provider needs a stub-server integration test that exercises the validation round-trip.

## Security

Vulnerability reports go to `security@deputyos.com` (when M5 lands) or directly to the maintainers via private channel. Please don't open public issues for vulnerabilities. We commit to a 90-day disclosure SLA.

## Code of conduct

Be kind, especially to people newer than you. We're building this for semi-technical users and that includes you, sometimes.
