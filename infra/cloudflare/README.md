# infra/cloudflare/ — public-domain infrastructure

Single source of truth for the Cloudflare-side configuration that backs:

- `www.deputyos.com` — Astro marketing site + curated docs + blog. **Source repo: `deputyos/www-deputyos-com`** (extracted from this repo's `site/`). Cloudflare Pages project `deputyos-site`.
- `docs.deputyos.com` — full MkDocs technical reference. **Source: `documentation/` in this repo**. Cloudflare Pages project `deputyos-docs`. The www site links here for the canonical reference.
- `api.deputyos.com` — Vue dashboard + Rust API
  - SPA on Cloudflare Pages (→ `status/dist`)
  - `/api/*` on a Cloudflare Worker that wraps `deputyos-api`
- `cdn.deputyos.com` — signed image artefacts and SBOMs (Backblaze B2 bucket, fronted by Cloudflare)
- `try.deputyos.com` → 302 to `https://www.deputyos.com/picker/`
- `deputyos.dev` (legacy) → 301 to the same path on `deputyos.com`

Everything is reproducible via `wrangler` and Cloudflare API tokens. No clicks
in the dashboard once the bootstrap is done.

## DNS records

| Name                 | Type  | Target                           | Proxied | Notes |
|----------------------|-------|----------------------------------|---------|-------|
| `deputyos.com`     | A     | (Cloudflare-managed)             | yes     | apex 301-redirects to `www.` via Page Rule |
| `www.deputyos.com` | CNAME | `deputyos-site.pages.dev`         | yes     | Pages project: `deputyos-site` (source repo: `deputyos/www-deputyos-com`) |
| `docs.deputyos.com` | CNAME | `deputyos-docs.pages.dev`        | yes     | Pages project: `deputyos-docs` (built from `documentation/` in this repo by `.github/workflows/docs-deploy.yml`) |
| `api.deputyos.com` | CNAME | `deputyos-status.pages.dev`       | yes     | SPA shell |
| `api.deputyos.com/api/*` | (Worker route) | `deputyos-api` Worker | yes | reverses traffic to backend |
| `cdn.deputyos.com` | CNAME | `f<NNN>.backblazeb2.com`          | yes     | B2 bucket `cdn-deputyos-com`, proxied (orange-cloud) so egress is free via the Bandwidth Alliance + edge-cached. A Transform Rule rewrites `cdn.deputyos.com/<path>` → the B2 origin path `/file/cdn-deputyos-com/<path>`. See below. |
| `try.deputyos.com` | (Page Rule) | `https://www.deputyos.com/picker/` | yes | 302 |
| `accounts.deputyos.com` | (post-M8) | Fly.io app | yes | scheduled |
| `deputyos.dev` (legacy) | (Page Rule) | `https://deputyos.com/$1`    | yes     | 301; survives any cached old URLs |

## Cloudflare Pages projects

```bash
# www  — deployed from the deputyos/www-deputyos-com repo, not this one
wrangler pages project create deputyos-site --production-branch main

# docs — deployed from THIS repo's documentation/ via .github/workflows/docs-deploy.yml
wrangler pages project create deputyos-docs --production-branch main

# api (SPA shell)
wrangler pages project create deputyos-status --production-branch main
wrangler pages deploy status/dist --project-name deputyos-status
```

CI does the deploys via:

- `.github/workflows/status-deploy.yml` (in this repo) — deputyos-status.
- `.github/workflows/docs-deploy.yml` (in this repo) — deputyos-docs.
- `.github/workflows/deploy.yml` (in the `deputyos/www-deputyos-com` repo) — deputyos-site.

## Cloudflare Worker (`deputyos-api`)

The `/api/*` Worker fronts the Rust crate. Two deployment paths exist:

1. **Workers (preferred — no infra to run).** Reuse the same crate via
   workers-rs; `worker.toml` lives next to this README. Cache hot endpoints
   in Workers KV (`namespace = DEPUTYOS_API_CACHE`).
2. **Fly.io (fallback).** Deploy `deputyos-api` as a regular Axum binary; route
   `api.deputyos.com/api/*` to the Fly app via Cloudflare proxy.

Default plan is (1). (2) lights up if we need persistent state (e.g. M8
accounts) that exceeds Workers' KV/Durable Objects budget.

## B2 bucket (`cdn-deputyos-com`) fronted by Cloudflare

The artefact CDN is a **Backblaze B2** bucket behind Cloudflare (Bandwidth
Alliance → B2↔Cloudflare egress is free; Cloudflare edge-caches the objects).

Setup:
1. B2: create a **public** bucket `cdn-deputyos-com`; create an application key
   scoped to it (keyID + applicationKey → the release CI secrets
   `B2_KEY_ID` / `B2_APPLICATION_KEY`, and the API app's `B2_KEY_ID`/`B2_SECRET`
   for the backup object-lake). Note the bucket's friendly host `f<NNN>.backblazeb2.com`.
2. Cloudflare: `cdn.deputyos.com` CNAME → `f<NNN>.backblazeb2.com`, **proxied**
   (orange-cloud). Add a **Transform Rule** rewriting the path
   `/<x>` → `/file/cdn-deputyos-com/<x>` so public URLs stay clean
   (`https://cdn.deputyos.com/<channel>/manifest.json`). B2's own CORS +
   Cloudflare cache rules cover the picker / status reads (equivalent to the
   R2 `cors.json` allow-list: `GET/HEAD` from `www.` and `api.deputyos.com`).

Publish: `make publish-cdn` (rclone `b2:` remote, default
`DEPUTYOS_CDN_REMOTE=b2:cdn-deputyos-com`). The on-CDN **layout is identical to
the previous R2 layout**, so `deputyctl update` and the launcher manifest URLs
are unchanged:

Release CI also requires:

- `DEPUTYOS_RELEASE_KEY` — the complete minisign private-key contents; the
  workflow writes it to a mode-0600 temporary file before signing.
- `DEPUTYOS_RELEASE_PUBKEY` — the matching minisign public-key contents,
  embedded into launchers and baked at `/etc/deputyos/pubkey.minisign`.
- `DEPUTYOS_API_PUBKEY` — the API JWT public PEM baked into appliances for
  account-owner remote access.
- `B2_KEY_ID` / `B2_APPLICATION_KEY` — bucket-scoped publication credentials.

```
/<channel>/manifest.json
/<channel>/manifest.json.minisig
/<channel>/pubkey.minisign
/<channel>/<version>/manifest.json
/<channel>/<version>/manifest.json.minisig
/<channel>/<version>/deputyos-<profile>-<hw>-<tier>[-airgap]-<version>-<channel>.<fmt>
/<channel>/<version>/<artefact>.sha256
/<channel>/<version>/<artefact>.minisig
/<channel>/<version>/<artefact>.cdx.json     # CycloneDX SBOM
/<channel>/<version>/<artefact>.intoto.jsonl # SLSA v1 provenance
```

`make publish-cdn` (in `.github/workflows/release.yml`) wraps `make
publish-local` and non-destructively copies `dist/` to the manifest channel in
the B2 bucket (for example `b2:cdn-deputyos-com/stable`). `make publish-r2`
remains as a back-compat alias that targets an `r2:` remote if you still have
one.

## Bootstrap order

1. Buy / transfer `deputyos.com`; nameservers point at Cloudflare.
2. Create Cloudflare API token with `Pages:Edit`, `Workers:Edit`, `DNS:Edit`.
   Store as the GitHub Actions secret `CLOUDFLARE_API_TOKEN`.
3. Create the **B2** bucket `cdn-deputyos-com` (public) + a scoped app key;
   CNAME `cdn.deputyos.com` → `f<NNN>.backblazeb2.com` (proxied) + the Transform
   Rule. Set CI secrets `B2_KEY_ID` / `B2_APPLICATION_KEY`.
4. Create Pages projects `deputyos-site` and `deputyos-status`.
5. Configure DNS + Page Rules per the table above.
6. Push a tag — CI handles `site-deploy`, `status-deploy`, `release`.

## Sneakernet path (for air-gapped users)

Air-gapped images don't reach this CDN; users grab artefacts on a trusted
machine and copy via USB. The CDN URL scheme stays the same so `deputyctl
update --from /mnt/deputyos/<usb>/manifest.json` works unchanged.
