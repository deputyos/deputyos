# deputyOS Technical Documentation

deputyOS is a batteries-included appliance image for personal AI assistants. You
flash one image, boot it on a Raspberry Pi, an x86 mini-PC, a desktop
hypervisor, or a cloud VM, and within a few minutes you have a hardened,
self-managing host running one of the two flagship assistants —
[OpenClaw](https://github.com/openclaw/openclaw) or
[Hermes Agent](https://github.com/NousResearch/hermes-agent) — or a
community profile such as [Khoj](https://github.com/khoj-ai/khoj). The image
carries every runtime, native module, signature database, and configuration
template it needs. Nothing is fetched from the network on first boot except
the model provider you chose to use.

!!! info "Flagship vs community profiles"
    **OpenClaw and Hermes Agent** are the two flagship profiles deputyOS is
    built around — first-class supported, smoke-tested on every release,
    featured in the picker. **Khoj** is a community profile shipped to prove
    that the plugin model works (it added with zero Rust changes — see
    [Concepts → Plugin model](concepts/plugin-model.md)). Future community
    profiles follow Khoj's pattern.

This site is the technical reference for that system.

## Who this site is for

- **Profile authors** adding a new assistant to the appliance — start with
  [Concepts → Profile class](concepts/profile-class.md), then
  [How-to → Add a profile](how-to/add-a-profile.md).
- **Hardware integrators** porting deputyOS to a new SBC, cloud, or hypervisor
  — [How-to → Add a hardware target](how-to/add-a-hardware-target.md) and
  [Build → Image bake internals](build/image-bake-internals.md).
- **Security reviewers** auditing the default posture and update trust chain
  — [Concepts → Threat model overview](concepts/threat-model-overview.md) and
  [Security → Default-on controls](security/default-on-controls.md).
- **Sysadmins and SREs** running deputyOS in fleets — [Operations](operations/monitoring-and-logs.md)
  for the runbook, [Reference → CLI → deputyctl](reference/cli/deputyctl.md) for
  the operator surface.
- **Contributors** working on the Rust crates, the Ansible role, or the
  release pipeline — [Contributing → Overview](contributing/overview.md).
- **Integrators** wiring deputyOS into another system — [Reference → APIs](reference/apis/message-relay.md)
  for the wire protocols.

If you are a *user* (you bought a Pi and want to chat with an assistant), the
[README](https://github.com/deputyos/deputyos#readme) and the
[deputyos.com](https://www.deputyos.com) picker are the right starting points. This
site is one layer below that, for the people who extend, audit, or operate
the system.

## How this site is organised

The navigation follows the [Diátaxis](https://diataxis.fr/) split, with three
operational sections grafted on:

- **[Concepts](concepts/architecture.md)** — what deputyOS *is*. The appliance
  model, the profile-class rule, the plugin model, the threat-model overview.
  Read these to understand *why* the system looks the way it does.
- **[Reference](reference/cli/deputyctl.md)** — the authoritative inventory.
  Every CLI subcommand, every TOML field, every JSON schema, every systemd
  unit, every HTTP route, every Unix socket. Look here when you need an
  exact answer.
- **[How-to](how-to/add-a-profile.md)** — task-oriented recipes. Add a
  profile, add a hardware target, rotate a key, set up a tunnel.
- **[Operations](operations/monitoring-and-logs.md)** — running the appliance
  in production. Monitoring, cost guardrails, update and rollback.
- **[Security](security/default-on-controls.md)** — the controls turned on by
  default, the secrets-storage contract, the update trust chain, and how to
  report a vulnerability.
- **[Build](build/make-targets.md)** — building images locally. Make targets
  and the bake-pipeline internals.
- **[Distribution](distribution/hardware-matrix.md)** — what we publish for
  what hardware, and how the desktop launcher boots an image on a laptop.
- **[Contributing](contributing/overview.md)** — the development loop, the
  PR flow, the ADR process.
- **[Glossary](glossary.md)** — every term used across the site, defined.

## If you want to…

| Goal | Go to |
|---|---|
| Understand the architecture and lifecycle | [Concepts → Architecture](concepts/architecture.md) |
| Look up a CLI flag or subcommand | [Reference → CLI → deputyctl](reference/cli/deputyctl.md) |
| Add a third (or fourth) profile | [How-to → Add a profile](how-to/add-a-profile.md) |
| Review the security posture | [Security → Default-on controls](security/default-on-controls.md) |
| Build an image locally on your laptop | [Build → Make targets](build/make-targets.md) |
| Run a fleet and watch its logs | [Operations → Monitoring and logs](operations/monitoring-and-logs.md) |
| Verify a published image's signature | [Security → Update trust chain](security/update-trust-chain.md) |
| Read the wire format of the message relay | [Reference → APIs → message relay](reference/apis/message-relay.md) |
| Look up which devices we support | [Distribution → Hardware matrix](distribution/hardware-matrix.md) |
| Find out what a term means | [Glossary](glossary.md) |

## Status

deputyOS today consists of:

- **Five Rust workspace crates**: [`deputyctl`](reference/cli/deputyctl.md) (the
  operator CLI), [`deputywizard`](reference/cli/deputywizard.md) (the first-boot
  setup server), [`deputypwa`](reference/cli/deputypwa.md) (the always-on web
  dashboard), [`deputyos-track`](reference/cli/deputyos-track.md) (the
  release-tracker bot), and [`deputyos-desktop`](reference/cli/deputyos-desktop.md)
  (the desktop launcher).
- **Two flagship profiles plus one community example** — OpenClaw (Node)
  and Hermes Agent (Python) are the supported deployment targets. Khoj
  (Python) is a community profile that validated the plugin surface
  (added with zero Rust changes). See
  [Concepts → Plugin model](concepts/plugin-model.md).
- **14 first-class hardware targets plus 3 community templates** —
  Raspberry Pi 4 / 5, generic arm64 SBCs, x86_64 mini-PCs, WSL2, macOS qemu,
  DigitalOcean, Hetzner, Vultr, Linode, Oracle ARM Free, Fly Machines (CPU
  and GPU), plus Proxmox/Unraid/TrueNAS templates and cloud-init recipes.
  See [Distribution → Hardware matrix](distribution/hardware-matrix.md).
- **221 cargo tests** as the Rust baseline, plus the Ansible role linted at
  the production profile, ansible-lint / yamllint / shellcheck on the
  scripts, and a QEMU smoke harness for every published image. See
  [Contributing → Overview](contributing/overview.md).

!!! note "Single-version site"
    This site documents the current state of the codebase. We do not publish
    versioned docs (no `mike`); the GitHub repo is the historical record.

## Where to get help

- **Bugs and feature requests** — open an issue at
  [github.com/deputyos/deputyos/issues](https://github.com/deputyos/deputyos/issues).
  See [Contributing → Overview](contributing/overview.md) for the issue
  template and triage rhythm.
- **Security disclosures** — see
  [Security → Reporting vulnerabilities](security/reporting-vulnerabilities.md)
  for the private-disclosure path. Please do not open public issues for
  vulnerabilities.
- **Discussion** — GitHub Discussions on the same repo. Profile-class
  questions, hardware-port questions, and architecture proposals all start
  there.

!!! tip "Start with Concepts"
    If this is your first time reading these docs, the four
    [Concepts](concepts/architecture.md) pages are the shortest path to a
    working mental model of deputyOS. Reference and How-to are easier to
    navigate once those are clear.
