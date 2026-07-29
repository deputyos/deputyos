# 16 — Audit and compliance evidence

deputyOS keeps audit evidence local first. `deputyctl audit emit` appends
newline-delimited JSON events to the local spool at
`/var/lib/deputyos/audit/spool.jsonl` (or `DEPUTYOS_AUDIT_SPOOL` in dev).
`deputyctl audit flush` uploads the current batch to the cloud API with the
device backup/audit token.

The cloud API stores batch metadata in SQLite and compresses the raw batch with
zstd. Production deployments write the compressed object to B2-compatible
object storage; dev/test deployments can use a local audit warehouse fallback.
Objects use this account/device partitioning:

```text
audit/account_id=<account>/device_id=<device>/year=<yyyy>/month=<mm>/day=<dd>/<batch>.jsonl.zst
```

This layout is query-engine friendly. The API exposes an authenticated,
account-scoped DataFusion query endpoint that materializes only the caller's
compressed batches as the `audit_events` table and accepts read-only
`SELECT`/`WITH` SQL.

## Event contract

Each event is one JSON object:

```json
{
  "schema_version": 1,
  "id": "evt-...",
  "kind": "backup_completed",
  "occurred_unix_ms": 1780000000000,
  "profile": "openclaw",
  "payload": {"ok": true}
}
```

Initial event kinds are deliberately open-ended but should stay in
lowercase `snake_case` or `kebab-case`. Expected producers include backup,
restore, update, profile switching, mount policy changes, cost guardrails,
and future CVE remediation jobs.

## CVE service

The API can sync CVE findings for release artefacts from package lists or
CycloneDX SBOM components. The current enrichment path is:

1. parse CycloneDX SBOMs for each release artefact;
2. normalize packages to purl/CPE where possible;
3. query OSV for vulnerable packages;
4. enrich CVE aliases with NVD severity, CISA KEV flags exposed by NVD, and
   FIRST EPSS scores where available;
5. list findings in the account dashboard for audit/compliance review.

The next CVE step is to mark findings remediated when a patch-image release supersedes the
   vulnerable artefact.

The device-side audit spool does not depend on the CVE service. It records
evidence locally; CVE intelligence attaches to release artefacts and device
versions through the cloud API.
