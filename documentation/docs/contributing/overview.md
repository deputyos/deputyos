# Contributing — Overview

This page is the contributor's tour. It folds the repo's
[`CONTRIBUTING.md`](https://github.com/deputyos/deputyos/blob/main/CONTRIBUTING.md)
into a fuller guide for someone arriving from outside the project. If
you have not opened a PR against deputyOS before, read this end-to-end
once before you start writing code.

## Before you start

- **Read the architecture.** [Concepts → Architecture](../concepts/architecture.md)
  is the shortest path to a working mental model. The rest of this site
  is much easier to navigate once that page has been read.
- **Read the profile-class rule.** Whether your change is a profile or
  not, [Concepts → Profile class](../concepts/profile-class.md) explains
  the kind of system this is — and isn't. A surprising number of
  proposals get a polite redirection because they are asking deputyOS to
  be a category of thing it is not.
- **Look at recent issues and PRs.** If your topic is already in
  motion, save yourself the duplicate effort. If a similar PR was
  recently closed without merge, the closing comment usually has the
  reason and what would unblock the same idea.
- **Open an issue first for non-trivial changes.** Architectural
  changes, new dependencies, new hardware targets, new milestones, and
  new ADRs all benefit from agreement on the approach before code
  lands. A typo fix or a clearly scoped bug fix can go straight to PR.

!!! warning "The load-bearing invariant"
    The single non-negotiable design rule is **zero first-boot network
    installs**. Any change that introduces an `apt`, `npm`, `pip`,
    `cargo`, or `git clone` step on the booted device — even an
    optional one, even gated behind a flag — will be rejected. See
    [Concepts → Architecture](../concepts/architecture.md) for the
    rationale and [`docs/adr/0002-zero-first-boot-network-installs.md`](https://github.com/deputyos/deputyos/blob/main/docs/adr/0002-zero-first-boot-network-installs.md)
    in the repo for the design history.

## The development loop

You don't need a Pi. You need a laptop:

```sh
git clone https://github.com/deputyos/deputyos.git
cd deputyos
make doctor                              # checks tools; tells you what to install
cargo test --all                         # 221 tests; the baseline
make try TARGET=qemu-aarch64             # ~12 min on a fresh checkout
# wizard at http://localhost:8088
```

The same Ansible role CI uses runs on your laptop — macOS (Apple
Silicon and Intel), Windows (WSL2), and Linux (x86_64 and arm64) are
all first-class build hosts. There is no privileged build environment.
Per-host specifics are in
[Build → Image bake internals](../build/image-bake-internals.md), and
[Build → Make targets](../build/make-targets.md) is the inventory of
every Make target.

The iteration loop:

1. Edit (Rust, Ansible, Markdown — whatever your change touches).
2. `cargo test --all` if Rust changed.
3. `make build TARGET=qemu-aarch64` if the image needs to change.
4. `make try TARGET=qemu-aarch64` to boot the new image and exercise it.
5. `make smoke` to run the QEMU smoke harness.
6. `make ci SCAFFOLD_PHASE=1` to re-run the same checks CI runs.
7. Commit, push, open a PR.

## Workspace structure

deputyOS is a five-crate Cargo workspace plus a shared Ansible role plus
per-target Packer / cloud-init artefacts. The crates are:

| Crate | Purpose | Reference |
|---|---|---|
| `deputyctl` | The operator CLI. Reads profile manifests, drives systemd, validates keys, performs updates, runs backups, prints capability limits. The single binary that owns the management surface. | [Reference → CLI → deputyctl](../reference/cli/deputyctl.md) |
| `deputywizard` | First-boot HTTP server on `:8088`. Writes `/etc/deputyos/secrets.env`. Profile-aware, profile-agnostic. | [Reference → CLI → deputywizard](../reference/cli/deputywizard.md), [Reference → APIs → wizard HTTP](../reference/apis/wizard-http.md) |
| `deputypwa` | Always-on web dashboard and built-in private chat client. Replaces the wizard on `:8088` after first-boot. | [Reference → CLI → deputypwa](../reference/cli/deputypwa.md), [Reference → APIs → PWA HTTP](../reference/apis/pwa-http.md) |
| `deputyos-track` | Release-tracker bot. Polls upstreams every 30 minutes, opens PRs that bump pinned profile versions. | [Reference → CLI → deputyos-track](../reference/cli/deputyos-track.md) |
| `deputyos-desktop` | Desktop launcher (~5 MB). Mandates the platform-native hypervisor — WSL2 on Windows, UTM on macOS, qemu+KVM on Linux. Downloads the latest signed image and boots it. | [Reference → CLI → deputyos-desktop](../reference/cli/deputyos-desktop.md), [Distribution → Desktop launcher internals](../distribution/desktop-launcher-internals.md) |

Outside the Rust workspace:

- `roles/deputyos/` — the shared Ansible role. The single source of
  truth for what goes into an image. Variant gates per hardware
  target. See [Build → Image bake internals](../build/image-bake-internals.md).
- `packer/` — Packer templates, one per buildable hardware target.
- `cloud-init/` — `cloud-init` recipes for cloud targets that don't
  get a baked image (Hetzner, Vultr, Linode).
- `wsl/`, `macos/` — host-specific tarball / qcow2 packagers.
- `templates/` — community templates (Proxmox, Unraid, TrueNAS).
- `fly/` — Fly.io OCI artefact.
- `test/` — QEMU smoke harness fixtures.
- `profiles/` — the three profile manifests.
- `dist/` — release-pipeline outputs, gitignored.

## Adding things

The recipes for the most common contributions are in How-to:

- [How-to → Add a profile](../how-to/add-a-profile.md) — six files,
  zero Rust changes. The reference is the Khoj PR.
- [How-to → Add a hardware target](../how-to/add-a-hardware-target.md)
  — five files: variant task, Packer template, smoke fixture,
  `limits.json` row, `main.yml` import.
- [How-to → Add a hook](../how-to/add-a-hook.md) — drop a script in
  `/etc/deputyos/hooks.d/<kind>/`; payload schemas are in
  [Reference → Schemas → hook payloads](../reference/schemas/hook-payloads.md).
- [How-to → Add a model provider](../how-to/add-a-model-provider.md)
  — extend `deputyctl/etc/providers.json`.
- [How-to → Add a channel](../how-to/add-a-channel.md) — channel
  registry, ufw rule, AppArmor mediation.

What we accept (the short list): new profiles in the
[profile class](../concepts/profile-class.md), new hardware variants,
bug fixes to the workspace crates or the build pipeline, doc fixes,
new ADRs documenting architectural decisions we missed.

What we don't accept: profiles outside the class, changes that
re-introduce first-boot network installs, runtime dependencies without
arm64 + x86_64 prebuilt binaries, features that expose the agent to the
public internet by default (opt-in is fine), and code that bypasses
signature verification on the update path.

## Code style

| Language | Tool | Settings |
|---|---|---|
| Rust | `rustfmt` | Defaults. Run with `cargo fmt --all`. |
| Rust | `clippy` | `cargo clippy --all-targets --all-features -- -D warnings`. Warnings are errors in CI. |
| Ansible | `ansible-lint` | At the production profile. Run with `make lint`. |
| YAML | `yamllint` | 2-space indent. Configured via `.yamllint`. |
| Shell | `shellcheck` | All scripts in `roles/deputyos/files/` and `scripts/`. |
| Markdown | (no auto-formatter) | No trailing whitespace; hard-wrap at 100 cols inside paragraphs of prose; use mkdocs-material admonitions where useful. |
| Commit messages | (no enforced format) | Imperative mood. Short, specific PR titles. Conventional-commit prefixes are welcome but not required. |

The single rule: no commits land without `make ci` green. CI is the
contract; if CI is wrong, fix CI in the same PR.

## Testing

| Surface | What to run | Where to add tests |
|---|---|---|
| Rust units / integration | `cargo test --all` (221-test baseline) | Adjacent `mod tests` blocks; `tests/` for cross-crate integration; `deputyctl/tests/` for CLI shape tests. |
| Rust public API | `cargo doc --all` builds clean | Doc-comment examples are run; keep them minimal and meaningful. |
| Ansible role | `ansible-lint` and `make lint` | If you add a task, add an `assert` or a `command` check that proves the post-condition. |
| QEMU smoke | `make smoke` | `test/qemu/<hw>.cloudinit.yaml` per target; `test/smoke/<scenario>.sh` for behaviour. |
| Provider library | `cargo test -p deputyctl model::test_provider_key` | Stub-server integration test for every new provider. |
| Schema validation | The schema files live in `deputyctl/etc/`. | Add a fixture under `deputyctl/tests/fixtures/` that exercises the new field. |

A profile PR must include a smoke fixture that boots an image with the
profile selected, runs the wizard, validates that the gateway comes up
healthy, and tears down. A hardware-target PR must include both a
smoke fixture and a `limits.json` entry; the latter feeds the picker
page and the `deputyctl limits` output.

The 221-test baseline is the floor — adding a feature without adding
tests is a signal something is wrong, not a sign the feature is small.

## ADRs

Architecture Decision Records live at `docs/adr/` in the repo. There
are eight today (numbered `0001` through `0008`). The numbering is
sequential; the next ADR is `0009`.

Write an ADR when:

- You are making an *architectural* change — something that ripples
  across more than one crate, or that future contributors will want to
  understand the *why* of, not just the *what*.
- You are making a security decision — adding a default-on control,
  changing a trust boundary, picking a crypto primitive, opting out of
  a hardening feature for a reason.
- You are *changing direction* — replacing a previously-decided
  approach with a new one. The new ADR supersedes the old one and
  should reference it explicitly.

You do not need an ADR for: a routine bug fix, a refactor that
preserves behaviour, a new test, a doc improvement, a profile addition
inside the existing class, or a hardware-target addition that uses the
existing variant-gate pattern.

The format is short — a problem statement, the alternatives
considered, the decision, and the consequences. Pattern-match on the
existing eight ADRs.

## The PR flow

1. **Open an issue first** for anything non-trivial. State the goal,
   the alternatives you considered, and the rough shape of the
   change. Get a thumbs-up from a maintainer before you spend a
   weekend on it.
2. **Branch.** No enforced naming; `feature/<short-slug>` or
   `fix/<issue-number>-<slug>` is conventional.
3. **Implement.** Keep PRs focused. A 2,000-line PR that touches
   `deputyctl`, the wizard, and the Ansible role is harder to review
   than three 700-line PRs each focused on one surface.
4. **Test locally.** `make ci SCAFFOLD_PHASE=1` is the same set of
   checks CI runs. If it's red on your laptop, it's red in CI.
5. **Open the PR.** Reference the issue. State what you did and what
   you didn't do. If the change touches a documented surface, update
   this site (the page you're touching most likely lives under
   [Reference](../reference/cli/deputyctl.md) or
   [Concepts](../concepts/architecture.md)).
6. **Review.** Reviewers will land on the PR within a few days. Expect
   feedback; expect to iterate.
7. **Merge.** Squash-merge is the default. The PR title becomes the
   commit message; keep it specific.

!!! tip "Don't forget the docs site"
    If your PR adds a CLI flag, a TOML field, an HTTP route, or a
    systemd unit, the matching page in this site needs to be updated
    in the same PR. `mkdocs build --strict` is part of `make ci`, so a
    broken cross-link will fail your build.

## Cross-references

- [Concepts → Architecture](../concepts/architecture.md) — the system
  view.
- [Concepts → Profile class](../concepts/profile-class.md) — what
  qualifies as a profile.
- [Concepts → Plugin model](../concepts/plugin-model.md) — the
  contract that makes a profile a drop-in.
- [Build → Make targets](../build/make-targets.md) — every Make
  target, what it does, what it produces.
- [Build → Image bake internals](../build/image-bake-internals.md) —
  pi-gen, Packer, the Ansible role, variant gates.
- [Reference → CLI → deputyctl](../reference/cli/deputyctl.md) — the
  authoritative command surface.
- [Security → Reporting vulnerabilities](../security/reporting-vulnerabilities.md)
  — the private-disclosure path. **Vulnerability reports do not go
  through public issues.**

## Code of conduct

Be kind, especially to people newer than you. deputyOS is built for
semi-technical users — and that includes contributors, sometimes.
Patient review and clear writing are first-class contributions on
their own.
