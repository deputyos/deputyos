# 18 — Control-data boundary

This is an architectural invariant, not a storage preference. It is also the
cardinality boundary that lets the service address an aggregate of trillions of
logical deputies without creating a trillion relational rows.

## PostgreSQL

PostgreSQL contains only transactional commercial identity data:

- users and their login credentials;
- enterprises, organisations, memberships and roles;
- subscriptions, invoices, payments and payment-provider identifiers;
- current plans, licences and derived entitlements.

These records require relational uniqueness, transactional updates and strong
consistency around access and money.

PostgreSQL must not contain a row per desktop, deputy, heartbeat, tunnel,
command, backup, build, usage event or cloud runtime. A foreign key from a
commercial table to operational data is therefore also forbidden. Product
limits are entitlements on an account or enterprise, not counters maintained by
updating one row for every deputy operation.

Short-lived login/device-code and refresh-token records are part of account
authentication and may remain relational. A desktop's durable registration,
credentials and inventory are operational deputy data and are object-backed.

## Object storage

Everything related to desktops and deputies is stored as immutable,
content-addressed or versioned objects:

- desktop registration records and hashed machine credentials;
- deputy definitions, configuration, desired state and tombstones;
- materialisation records, copy-on-write disk references and checkpoints;
- fleet snapshots, batched telemetry and heartbeats;
- queued commands, results and audit history;
- tunnel routing declarations and connection events;
- backup bundles, catalogs, verification and deletion events;
- usage/cost events and rollups;
- cloud-runtime requests and lifecycle events;
- build jobs, artefact metadata, CVE evidence and reports.

Small current-state caches may be rebuilt in memory or on local NVMe. Live
tunnel routing is necessarily ephemeral connection state. Neither is a durable
source of truth and neither is written to PostgreSQL.

## Cardinality model

The control plane distinguishes three things:

1. An **account or enterprise** is a commercial principal in PostgreSQL.
2. A **desktop** is an object-backed execution host with one authenticated,
   multiplexed control connection.
3. A **deputy** is normally only a compact manifest in the desktop's object
   shard. It becomes a copy-on-write local image only when materialised and a
   VM/distro process only when active.

A deputy never receives its own central database row, socket, heartbeat timer
or dedicated tunnel. Commands and tunnel streams carry a `deputy_id` over the
desktop connection. The desktop sends one batched presence/inventory snapshot,
not one heartbeat per deputy.

This makes “one trillion deputies” a namespace and durable-state target. It
does not imply one trillion simultaneously running virtual machines.

## Object layout

Identifiers are opaque UUIDs. `h0` and `h1` are the first two two-character
segments of `sha256(deputy_id)` and prevent hot or enormous prefixes.

```text
accounts/<account_id>/desktops/<desktop_id>/
  registration/manifest/<epoch>.json.zst
  credentials/by-hash/<credential_hash>.json.zst
  events/yyyy=<y>/mm=<m>/dd=<d>/<time>-<event_id>.json.zst
  inventory/h0=<h0>/h1=<h1>/
    segments/<content_sha256>.cbor.zst
    manifest/<epoch>.json.zst
  telemetry/yyyy=<y>/mm=<m>/dd=<d>/<time>-<batch_id>.cbor.zst
  commands/pending/<command_id>.json.zst
  commands/completed/<command_id>.json.zst

lookup/desktops/by-id/<desktop_id>/manifest/<epoch>.json.zst
lookup/credentials/by-hash/<credential_hash>.json.zst
```

The two lookup namespaces contain only signed/minimal routing records pointing
to the authoritative account/desktop object. They avoid bucket-wide scans when
a desktop connects with a token or a tunnel URL names a desktop. No lookup
object exists per deputy.

The inventory schema is defined by
[`deputy-inventory-v1.json`](schemas/deputy-inventory-v1.json). A segment holds
many deputy definitions; manifests reference segments and tombstones. This is
the same immutable-segment/numbered-manifest model used by `../newsapi-alt`,
rather than one object or one `LIST` result per deputy.

## Commit and concurrency pattern

Writers use the `newsapi-alt` pattern:

1. write immutable/content-addressed payload or segment objects;
2. derive the next manifest from the latest committed manifest;
3. conditionally create the numbered manifest as the commit point;
4. on a lost race, read the winner and re-derive the update;
5. discover current state using prefix listing plus the highest valid manifest;
6. represent deletion with tombstones or append-only lifecycle events;
7. garbage-collect unreferenced objects only after a grace period and while
   retaining a rollback window of manifest epochs.

Overwriting a mutable `latest.json` is not a commit protocol. A convenience
pointer may exist as a disposable cache, but readers must be able to recover
from the numbered manifest log alone.

## Security and tenancy

- Object keys are server-derived; clients cannot choose arbitrary prefixes.
- Credentials are never stored in plaintext. Lookup keys use a non-reversible
  digest of a high-entropy machine token and the authoritative record repeats
  that digest for verification.
- Every authoritative manifest includes its account and desktop IDs and is
  checked against the authenticated principal.
- Deputy IDs are scoped by desktop; a remote command must provide both IDs.
- Object-store encryption, versioning, retention and access logs are enabled.
- Account deletion writes an auditable tombstone before asynchronous
  prefix reclamation.

## Review rule

No new feature may add an operational metadata table to PostgreSQL. Schema
review must reject migrations that cross this boundary. Historical operational
tables are migration input only: new code must not write them, and they are
dropped after object migration and rollback expiry.
