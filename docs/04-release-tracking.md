# 04 — Release tracking

deputyOS exists to package upstream agents — OpenClaw, Hermes, future profiles — into appliance images. New upstream releases must reach users quickly and safely. This doc describes the pipeline that turns an upstream `git tag` into a signed image manifest the device trusts.

## The loop

```
   upstream                    deputyOS repo                     B2 + Cloudflare           device
   ────────                    ────────────                     ─────────────────         ──────
   new release tag  ──poll──▶  tracker GH Action
                                   │
                                   ▼
                               PR: bump pinned_version in profiles/<id>.toml
                                   │
                                   ▼
                               human review / auto-merge after CI
                                   │
                                   ▼
                               matrix CI build (~10–30 targets)
                                   │
                                   ▼
                               QEMU smoke gate (every artefact)
                                   │
                                   ▼
                               upload to project B2 bucket  ───────────▶  CDN
                                   │                                       │
                                   ▼                                       │
                               publish manifest.json + minisign + cosign
                                                                           │
                                                                           ▼
                                                                  deputyctl update --check
                                                                  reads manifest from CDN
```

## The release-tracker GitHub Action

Runs every 30 minutes (`cron`). For each profile:

```yaml
- uses: actions/github-script@v8
  with:
    script: |
      const { data: release } = await github.rest.repos.getLatestRelease({
        owner: 'NousResearch', repo: 'hermes-agent'
      });
      // compare to profiles/hermes.toml pinned_version
      // open PR if newer
```

PR is opened against `main`, titled `bump hermes to <new-version>`, with the upstream release notes pasted in the body. CI runs the smoke build on the PR. If green and the diff is purely a `pinned_version` bump in a single profile manifest, auto-merge is permitted. Anything else (e.g. profile schema changes, manifest restructure) blocks on human review.

## Channels

`release_channel = "stable"` and `"beta"` in the profile manifest control which upstream tags the tracker will pick up:

| Channel | Picks up |
|---|---|
| `stable` | upstream tags without pre-release identifiers (no `-beta`, `-rc`, `-dev`) |
| `beta` | latest tag of any kind, including pre-releases |

A device's active channel comes from its image build, not the manifest at runtime. To switch a device from stable → beta, you re-flash with a beta image (or `deputyctl update --channel beta` after we land that flag in M4+).

## Builds

Once a `pinned_version` bump merges, the matrix build runs across every published target (see [03-image-builds.md](03-image-builds.md)). For each successful artefact, CI:

1. Computes SHA256.
2. Signs with `minisign` (project key, kept offline; CI requests a signature via a side-car job that holds the key).
3. Signs with `cosign` (Sigstore, OIDC-attested via GitHub Actions).
4. Uploads `<artefact>`, `<artefact>.sha256`, `<artefact>.minisig`, `<artefact>.cosign.bundle` to the project B2 bucket under a versioned prefix.

The bucket is fronted by a Cloudflare Worker that rewrites paths to a stable CDN URL scheme:

```
https://cdn.deputyos.com/<channel>/<profile>/<hw>/<version>/<file>
```

Cloudflare's Bandwidth Alliance with Backblaze means egress through this CDN URL is free.

## The manifest

After every successful build, CI writes a single signed manifest:

```json
{
  "schema": 1,
  "deputyos_version": "2026.04.27",
  "released_at": "2026-04-27T13:00:00Z",
  "profiles": {
    "openclaw": {
      "agent_version": "2026.4.25",
      "release_channel": "stable",
      "kernel": "6.6.52-rpi-2712",
      "clamav_db_date": "2026-04-26",
      "magika_model": "1.4.0",
      "artefacts": {
        "rpi5":   { "url": "https://cdn.deputyos.com/stable/openclaw/rpi5/2026.04.27/deputyos-openclaw-rpi5-2026.04.27-stable.img.xz", "sha256": "...", "size": 1834567890, "minisig_url": "...", "cosign_url": "..." },
        "rpi4":   { "url": "...", "sha256": "...", "size": 1761432109 },
        "x86_64-mini-pc": { "url": "...", "sha256": "..." },
        "digitalocean":   { "id": "do-snap-12345", "regions": ["nyc1","fra1","sgp1"] },
        "oracle-arm-free":{ "url": "..." },
        "fly-machines":   { "oci_ref": "registry.fly.io/deputyos/openclaw:2026.04.27" },
        "...": "..."
      }
    },
    "hermes": { "...": "..." }
  },
  "signature": "minisign:RWQ..."
}
```

The signature is over the canonicalised JSON (sorted keys, compact form) and is verified by `deputyctl update --check` before any artefact is downloaded. The `cosign.bundle` accompanying each artefact lets users do a Sigstore-native verification independently.

## What `deputyctl update` does on the device

```
deputyctl update --check
  ├── HTTP GET https://cdn.deputyos.com/<channel>/manifest.json
  ├── HTTP GET https://cdn.deputyos.com/<channel>/manifest.json.minisig
  ├── verify minisign signature against /etc/deputyos/trusted-keys
  ├── compare manifest.profiles.<id>.agent_version with /etc/deputyos/build.json
  └── print "v0.11.0 → v0.12.0 available; run `deputyctl update --apply`"

deputyctl update --apply
  ├── download <artefact>.img.xz + .sha256 + .minisig (resumeable)
  ├── verify SHA256 + minisign + cosign
  ├── decompress into the inactive A/B slot partition
  ├── set tryboot/U-Boot pointer for one-shot boot of new slot
  ├── reboot
  └── (after reboot) watchdog asserts deputyctl doctor green within 5 min
        └── on failure, bootloader auto-rolls back to previous slot
```

The `data` partition (containing `~/.<profile>`, secrets, backup config) is mounted unchanged across the swap. User state is never touched by an update.

## Handling out-of-band fixes (security)

Two cases:

1. **Kernel / library CVE in a runtime that's already in the image** — we cut a "patch only" image rev that bumps the OS package set without bumping `pinned_version` for any profile. Manifest schema supports profiles that share an `agent_version` with the previous rev. ClamAV signature freshness rides this train when needed.
2. **Upstream agent CVE** — wait for the upstream patch release; the tracker picks it up automatically.

For both, we accept higher build cadence as the cost of the "no in-place upgrades" invariant.

## Versioning

| Thing | Format |
|---|---|
| deputyOS image rev | `YYYY.MM.DD` (date-based; multiple revs same day get `-N` suffix) |
| Profile pinned_version | mirrors upstream (`2026.4.25` for OpenClaw, `0.11.0` for Hermes) |
| Manifest schema | integer in `schema` field; bumps require coordinated `deputyctl` update |
| `deputyctl` | semver `<major>.<minor>.<patch>`; major bump only on schema break |

`deputyctl --version` prints the binary version, the deputyOS image rev, the active profile and its pinned_version, the kernel, the ClamAV DB date, and the Magika model.

## Latency budget

Goal: an upstream stable release reaches the user-facing manifest in ≤6 hours, including the soak window between beta and stable channels.

- Tracker poll: 30 min (configurable; can be cron + webhook later)
- Build matrix: ~30–60 min on hosted runners
- Beta soak: 4 h on a fleet of CI test devices before stable promotion
- Manifest publish: <1 min after final artefact upload

Under emergency CVE conditions, the soak is reducible to 30 min with a manual override flag.
