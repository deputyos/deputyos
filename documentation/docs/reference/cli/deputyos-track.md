# deputyos-track

Release-tracker bot.

`deputyos-track` polls the upstream GitHub repository for each profile in
`profiles/*.toml`, compares the latest tag to `[profile].pinned_version`,
and proposes a diff that bumps the version. It is **not** baked into the
appliance image — it ships only in the development repo and runs in two
places:

- **CI**: the `.github/workflows/release-tracker.yml` workflow runs the
  binary on a cron schedule. It opens PRs against the deputyOS repo when an
  upstream profile cuts a new release.
- **Contributor laptops**: `make track` runs the same binary locally so a
  contributor can preview proposed bumps before CI catches them.

The binary is intentionally decoupled from `deputyctl` — it only parses the
`[profile]` section of each manifest, and its TOML patcher does a targeted
line edit so comments and alignment in the profile files are preserved
verbatim.

## Synopsis

```
deputyos-track [--profiles-dir <path>] [--offline] <command>
```

## Global options

| Flag | Default | Purpose |
|---|---|---|
| `--profiles-dir <path>` | `profiles` | Where to look for `*.toml` profile manifests. Relative paths resolve against the current working directory. |
| `--offline` | off | Skip every network call. `discover()` returns an empty bump list, so every subcommand becomes a clean no-op. Used by `make ci` to exercise the tool without external dependence. |
| `-h, --help` | — | Print help. |
| `-V, --version` | — | Print version. |

## Logging

`tracing-subscriber` writes to **stderr** with a compact format. Default
filter is `deputyos_track=info,info`. Override via `RUST_LOG`:

```
RUST_LOG=deputyos_track=debug,info deputyos-track check
```

## Environment variables

| Variable | Purpose |
|---|---|
| `GITHUB_TOKEN` | Sent as `Authorization: token <…>` to api.github.com. Optional but **strongly recommended** — unauthenticated requests are rate-limited to 60/hour per IP, which is exhausted by a single repo with many forks polling. CI passes the workflow's `${{ secrets.GITHUB_TOKEN }}`. |

## Network dependency

Each non-`--offline` subcommand makes one HTTPS request per profile to
`api.github.com`. The HTTP client is `ureq` (sync, blocking) with a 20s
timeout. Network errors / rate-limits / 5xx are **logged as `warn` and
skipped per-profile** — the tool keeps working for the others.

## Version comparison

Implemented in [`deputyos-track/src/version.rs`](https://github.com/dipankarsr/deputyOS/blob/main/deputyos-track/src/version.rs). The `pinned_version` format follows two upstream conventions:

- **Date-style**: `YYYY.M.D` (OpenClaw — e.g. `2026.4.25`).
- **Semver-ish**: `MAJOR.MINOR.PATCH` (Hermes — e.g. `0.11.0`).

Both are parsed into a `(u64, u64, u64)` triple plus an optional
pre-release suffix string. Rules:

- A leading `v` is stripped (`v2026.4.27` → `2026.4.27`).
- Missing minor/patch components default to 0 (`1.0` → `(1, 0, 0)`).
- Anything after the first `-` is preserved as the suffix.
- Comparison is lexicographic on the triple.
- A present suffix sorts **before** the same triple with no suffix
  (`2026.4.25-beta1 < 2026.4.25`).

Tags from upstream are converted to the `pinned_version` format by
stripping a leading `v` for storage consistency.

## Channel handling

`[profile].release_channel` is one of `stable` or `beta`:

- **`stable`**: hits `GET /repos/{owner}/{repo}/releases/latest`.
- **`beta`**: hits `GET /repos/{owner}/{repo}/releases?per_page=10` and
  picks the first non-draft release (which is the most recent including
  pre-releases).

Any other channel string is a hard error per profile.

## Cron schedule

The CI workflow at `.github/workflows/release-tracker.yml` runs every 30
minutes:

```yaml
on:
  schedule:
    - cron: '*/30 * * * *'
```

It executes:

```
deputyos-track --profiles-dir profiles propose --out-dir .github/proposed
```

then a downstream step inspects `.github/proposed/*.json` to decide
whether to open a PR.

## Exit codes

`deputyos-track` returns `0` on every routinely-handled outcome (no
bumps, all bumps applied, dry-run printed). It only fails non-zero when
something is wrong with the *input* (malformed profile TOML, bad CLI
arguments) or when a write/git/gh subprocess fails.

| Code | Meaning |
|---|---|
| `0` | Success, including "no bumps" and `--dry-run`. |
| Other | anyhow error — printed with `eprintln!` and propagated. |

---

## Commands

### `check`

Print profiles that have an upstream release newer than pinned.

#### Synopsis

```
deputyos-track check
```

#### Behavior

1. Enumerate `<profiles-dir>/*.toml`.
2. For each: parse `[profile]`, query the upstream channel's latest tag,
   compare versions.
3. Print one line per profile that has a newer upstream:
   ```
   <id>: <pinned> -> <new>  (<release-url>)
   ```
4. If no profile has an update, print `all profiles up to date`.

`--offline` short-circuits to "all profiles up to date" without any HTTP.

#### Examples

```
$ deputyos-track check
openclaw: 2026.4.20 -> 2026.4.27  (https://github.com/openclaw/openclaw/releases/tag/v2026.4.27)
hermes: 0.11.0 -> 0.12.0  (https://github.com/anthropics/hermes/releases/tag/v0.12.0)
```

```
$ deputyos-track --offline check
all profiles up to date
```

#### Exit codes

- `0` — always (network errors per-profile are logged but don't fail the run).

#### Related

- [profile-toml schema](../schemas/profile-toml.md)

---

### `propose`

Emit patch files describing the bump.

#### Synopsis

```
deputyos-track propose [--out-dir <path>] [--stdout]
```

#### Options

| Flag | Default | Purpose |
|---|---|---|
| `--out-dir <path>` | `.` | Output dir for `propose-<id>-<v>.patch` and sidecar `.json` files. |
| `--stdout` | off | Print the patched TOML to stdout instead of writing files. |

#### Behavior

1. Run discover (same as `check`).
2. For each bump, **read** `profiles/<id>.toml`, run `bump_pinned_version`
   to produce a targeted line-edit, and either:
   - Print the patched TOML to stdout (with a `# <id>: <old> -> <new>` and
     `---` header line); or
   - Write `<out-dir>/propose-<id>-<v>.patch` (the patched TOML body) +
     `<out-dir>/propose-<id>-<v>.json` (metadata sidecar).
3. The TOML patcher is a deliberately conservative line edit — comments,
   blank lines, and alignment in the rest of the file are preserved
   verbatim.

#### Sidecar JSON shape

```json
{
  "profile_id": "openclaw",
  "old_version": "2026.4.20",
  "new_version": "2026.4.27",
  "upstream_repo": "openclaw/openclaw",
  "release_url": "https://github.com/openclaw/openclaw/releases/tag/v2026.4.27",
  "release_name": "OpenClaw 2026.4.27",
  "release_body": "...full release body...",
  "released_at": "2026-04-27T12:00:00Z",
  "profile_path": "profiles/openclaw.toml"
}
```

The CI workflow consumes the sidecars to populate the PR title and body
without re-fetching from GitHub.

#### Examples

```
$ deputyos-track propose --out-dir /tmp/bumps
wrote /tmp/bumps/propose-openclaw-2026.4.27.patch (2026.4.20 -> 2026.4.27)
```

```
$ deputyos-track propose --stdout
# openclaw: 2026.4.20 -> 2026.4.27
---
[profile]
id = "openclaw"
display_name = "OpenClaw"
upstream_repo = "openclaw/openclaw"
release_channel = "stable"
pinned_version = "2026.4.27"
...
```

#### Files written

- `<out-dir>/propose-<id>-<v>.patch`
- `<out-dir>/propose-<id>-<v>.json`

(none if `--stdout`).

---

### `apply`

Apply the bump in place to `profiles/<id>.toml`.

#### Synopsis

```
deputyos-track apply [--yes]
```

#### Options

| Flag | Purpose |
|---|---|
| `--yes` | Required to actually write. Without it, prints a dry-run line per bump. |

#### Behavior

1. Run discover.
2. For each bump:
   - Without `--yes`: print `[dry-run] would bump <id>: <old> -> <new>`.
   - With `--yes`: write the patched TOML over `profiles/<id>.toml`. The
     write is **not atomic** (no tmp+rename) — `deputyos-track` is a
     development-side tool, not a runtime path.

#### Examples

```
$ deputyos-track apply
[dry-run] would bump openclaw: 2026.4.20 -> 2026.4.27
[dry-run] would bump hermes: 0.11.0 -> 0.12.0
```

```
$ deputyos-track apply --yes
applied: profiles/openclaw.toml (2026.4.20 -> 2026.4.27)
applied: profiles/hermes.toml (0.11.0 -> 0.12.0)
```

#### Files written

- `<profiles-dir>/<id>.toml` (one per bump, only with `--yes`).

---

### `open-pr`

Apply + commit + push + open a PR via `gh`.

#### Synopsis

```
deputyos-track open-pr [--yes]
```

#### Options

| Flag | Purpose |
|---|---|
| `--yes` | Required to actually run. Without it, prints a dry-run line per bump. |

#### Behavior

1. Run discover.
2. If the `gh` CLI is not on PATH, print a friendly skip message and
   exit 0:
   ```
   gh CLI not on PATH; skipping open-pr (use propose + apply locally instead)
   ```
3. For each bump (with `--yes`):
   1. `git checkout -B track/<id>-<new>`
   2. Write the patched TOML.
   3. `git add <profile-path>`
   4. `git commit -m "track: bump <id> pinned_version <old> -> <new>"`
   5. `git push -u origin track/<id>-<new>`
   6. `gh pr create --title "<same>" --body "<auto-body>" --label tracker --label auto-pr`

The PR body is built from the upstream release name, URL, published-at
timestamp, and release notes (truncated at 4000 chars with a
`[...truncated]` marker).

#### Examples

```
$ deputyos-track open-pr --yes
[git, gh output...]
✓ PR opened: https://github.com/dipankarsr/deputyOS/pull/123
```

#### External dependencies

- `git` on PATH.
- `gh` on PATH and authenticated. The CI workflow uses
  `secrets.GITHUB_TOKEN` via `gh auth login --with-token`.

#### Files written

- `<profiles-dir>/<id>.toml` (per bump).
- New branch on the remote.

---

## See also

- [profile-toml schema](../schemas/profile-toml.md) — the manifest fields the tracker reads.
- [build/make-targets](../../build/make-targets.md) — the `make track` target.
- [contributing/overview](../../contributing/overview.md) — how to land an upstream bump locally vs via the bot.
