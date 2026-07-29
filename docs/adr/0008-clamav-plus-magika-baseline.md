# ADR 0008 — ClamAV + Magika as default-on baseline

**Status:** Accepted (M0)

## Context

deputyOS receives files from the outside world via the agent's messaging channels. Telegram messages can have attachments; Slack messages can include uploaded documents; WhatsApp images and PDFs land in the agent's data dir; some agents (Hermes especially) execute commands on user-supplied content. This is a real attack surface: a malicious file disguised as a benign image, a script with a faked extension, a known-bad payload arriving as a Discord upload.

A semi-technical user is not going to install or configure on-device antivirus themselves.

## Decision

The image baseline includes two complementary scanners, **on by default**:

1. **ClamAV (`clamd`)** — signature-based malware scanner running with `fanotify` watching `~/.<profile>/uploads/`. Quarantines hits to `/var/quarantine/` (mode 0700, root-owned). Daily scheduled scan at 04:30 local. The signature DB is **packed into the slot image at build time**; `freshclam` is disabled at runtime. New signatures arrive in the next image rev (or as a "patch only" rev for urgent CVEs).

2. **Magika** — Google's open-source AI content-type detector ([github.com/google/magika](https://github.com/google/magika), Apache 2.0). Wired into the agent's file-handling path *before* a file from a channel is written to disk. If the detected content type doesn't match the declared extension, the file is flagged. This catches the "script disguised as a JPEG", "executable disguised as PDF" class of attacks that signature-only scanning will miss.

The two are complementary:

| Threat | Caught by |
|---|---|
| Known-signature malware | ClamAV |
| Polyglot file (zip with a JPEG header) | Magika |
| Malicious script with extension `image.png` | Magika |
| File matching no AV signature but with a fake extension | Magika |
| Malicious file with no extension manipulation | ClamAV (if signature exists) |

## Why packed signatures (no `freshclam`)

`freshclam` running at runtime would be a first-boot network operation, and a recurring background one. That violates [ADR-0002](0002-zero-first-boot-network-installs.md). Instead, signatures travel with the image rev.

- ClamAV signature DB (`main.cvd` + `daily.cvd`) is downloaded fresh during every image build.
- The build records the DB date in `manifest.json` (`clamav_db_date` field).
- For urgent signature updates we cut a "patch only" image rev — manifest schema supports a profile staying on the same `agent_version` across multiple `deputyos_version`s.

The cadence is roughly: image revs ride upstream agent releases (multiple per week). Signature staleness stays bounded to days under normal cadence; emergency revs are minutes.

## Why Magika (and not just ClamAV)

ClamAV is signature-based: it doesn't know if a file's declared extension is honest. Magika is the inverse: it identifies actual content type from file bytes, regardless of extension or filename. Combining the two covers two different attack shapes.

Magika is also **fast** (5 ms inference on a single CPU, per Google's reported benchmarks) and **small** (a few MB model) — no GPU needed, fits comfortably in the image.

## Why on by default (not opt-in)

- A semi-technical user will not opt into security tooling they don't know they need.
- The performance overhead is negligible (Magika 5ms; ClamAV daily scan timer when idle).
- The wizard refuses to bring up channels until `deputyctl doctor` reports the baseline healthy. This is the deliberate guard that prevents the appliance from being on the internet with a broken security baseline.

## Consequences

- Build pipeline runs `freshclam` once per build to fetch the signature DB.
- Image size grows ~250 MB for ClamAV + ~10 MB for Magika.
- We accept the trade-off on `kernel.unprivileged_userns_clone=1` — required by Hermes' command-execution sandbox; documented in [09-security.md](../09-security.md).

## Alternatives considered

- **ClamAV alone.** Rejected: signature-only coverage misses the content-type-spoofing class. Magika's marginal cost is small.
- **Magika alone.** Rejected: doesn't catch known-signature malware that uses honest extensions.
- **A more aggressive scanner (Wazuh, OSSEC).** Rejected for default-on: too operationally heavy for a single-purpose appliance. A user who wants HIDS can install one alongside.
- **Sandbox-only, no AV.** Rejected: AppArmor profiles and the agent's own command-execution sandbox limit blast radius if a malicious file does run, but the user's still going to ask "did anything bad just land in my Telegram?" — and we should be able to answer.

## Operational notes

- `deputyctl doctor` checks `clamd` is running, the DB date is within an acceptable window, and Magika returns a known result on a fixture file.
- `deputyctl quarantine list` enumerates current quarantine entries; `deputyctl quarantine release <id>` is the escape hatch (logs a `WARN` to journald).
- ClamAV false-positive rates are non-zero; the quarantine flow is designed to be inspectable rather than automatically destructive.
