# ADR 0005 — Backblaze B2 with Cloudflare egress

**Status:** Accepted (M0)

## Context

We host two kinds of data:

1. Image artefacts (`.img.xz`, `.qcow2`, snapshots, manifests) downloaded by users — public-read, large, potentially many users per release.
2. Per-user backups of agent data — private, small per device, but as a function of n users.

For (1) the dominant cost is egress: a 1.5 GB image fetched by 1,000 users from S3 is ~$135 in egress alone. For (2) the dominant cost is storage.

## Decision

**Use Backblaze B2 fronted by Cloudflare for project-owned image hosting**; **let users own their backup buckets** in either B2 or Cloudflare R2.

Backblaze and Cloudflare are partners in the [Bandwidth Alliance](https://www.cloudflare.com/partners/technology-partners/backblaze/) — egress from B2 through Cloudflare is **free, with no caps**. This makes B2 + Cloudflare the cheapest credible option for hosting many GB of public-download artefacts at modest scale.

For user backups, we abstract on `rclone`. The wizard accepts:

- B2 native (`keyID` / `applicationKey`)
- R2 (S3-compatible: account ID, access key, secret)
- A device-code OAuth into Cloudflare that auto-provisions an R2 bucket for non-technical users

## Why not just S3 / GCS / Azure

- **Egress.** S3 is $0.09/GB egress. B2 (without Cloudflare) is $0.01/GB. B2 + Cloudflare is $0.00/GB. Even at modest project scale, the cost difference dominates.
- **R2 alone could replace B2.** R2 has zero egress globally (no Cloudflare-fronted dependency). We support R2 as a first-class user-bucket option. We chose B2 for the project bucket because B2's storage price ($0.006/GB-month) is currently lower than R2's ($0.015/GB-month) and the Bandwidth Alliance closes the egress gap when Cloudflare-fronted.

## Why not host on GitHub Releases

- **Egress quotas.** GitHub Actions artefact storage and Releases bandwidth aren't designed for serving 1.5 GB images at scale to anonymous users. We'd hit Abuse Detection on a popular release.
- **Path stability.** GitHub release asset URLs are tied to the release tag. We need the manifest to point at stable URLs that survive re-tagging.

## Consequences

- We operate a Backblaze account and a Cloudflare account; both are project secrets that must be managed (the build CI needs scoped tokens).
- The CDN URL scheme (`https://cdn.deputyos.com/<channel>/<profile>/<hw>/<version>/<file>`) is locked into the manifest schema. Changing host providers later means a manifest schema bump and a fleet `deputyctl update`.
- Users who pick the Cloudflare-OAuth backup path get a great UX but introduce one more vendor's account to their setup. The B2-native and R2-native paths exist for users who already have those accounts.
