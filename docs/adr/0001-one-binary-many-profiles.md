# ADR 0001 — One binary, many profiles

**Status:** Accepted (M0)

## Context

deputyOS exists to package open-source personal AI assistants (OpenClaw, Hermes, future) into appliance images. Each upstream is its own software stack with its own runtime, install path, configuration model, and channel set. A user installs one of them per device.

We need a management surface — start/stop, switch agents, validate keys, run health checks, apply updates, take backups. The shape of that surface is highly similar across agents (channels, allowlists, model providers, upgrade flow), even though the underlying implementations diverge.

## Decision

Ship a **single Rust binary `deputyctl`** that is profile-driven. A profile is a TOML manifest that declares everything `deputyctl` needs to know about the installed agent: paths, systemd unit, channels, healthcheck, AppArmor profile, kernel sysctl requirements, wizard prompts, upgrade hooks.

Adding a new profile means: (1) the build pipeline learns to bake it in, (2) a manifest file lands in `profiles/`. **No code change to `deputyctl`.** This is the lever that lets us track upstream releases for n agents without n forks of the manager.

## Alternatives considered

- **One binary per profile** (`openclawctl`, `hermesctl`). Rejected: most of the surface is the same; we'd duplicate update/backup/wizard logic and accept drift. Also bad for users running multiple devices on different profiles — they'd have to remember which command to use.
- **A thin shell-script orchestrator** invoking each agent's own CLI. Rejected: shell scripts are fragile and don't do well with the wizard's structured key-validation calls and the update flow's signature verification. We want type safety on the partition swap path.
- **Plugin system in the binary** (dlopen profile DLLs). Rejected: harder to sign, harder to AppArmor-confine, and we don't actually need code-level extension — TOML configuration covers the variation we have.

## Consequences

- The schema in [02-profiles.md](../02-profiles.md) is load-bearing. Schema bumps need a coordinated `deputyctl` version bump.
- New profile = manifest + bake recipe. M7 exists to validate this is true with a third profile.
- `deputyctl` itself should grow slowly. Behaviour that's profile-specific should live in the manifest / profile bake recipe, not in Rust.
