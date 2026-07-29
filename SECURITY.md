# Security policy

## Reporting a vulnerability

Email **[security@deputyos.com](mailto:security@deputyos.com)**.

Please don't open public GitHub issues for security reports — coordinated
disclosure protects users while we ship a fix.

We commit to:

- Acknowledging your report within **3 business days**.
- Engaging with your timeline; the default disclosure window is **90 days**
  from first contact.
- Crediting you in the release notes (unless you prefer to remain anonymous).

## Scope

In scope:

- The deputyOS appliance image (any of its hardware variants).
- The `deputyctl`, `deputywizard`, `deputypwa`, `deputyos-track`, `deputyos-api`,
  and `deputyos-desktop` crates.
- The release manifest, signature chain, and update apply path.
- The public infrastructure under `*.deputyos.com`.

Out of scope:

- User-supplied profile manifests / channel configurations / hooks.
- Third-party agent code (OpenClaw, Hermes, Khoj upstream issues — please
  report to those projects directly; we'll fold security-relevant fixes
  into the next image rev).

## Hardening defaults

Every image ships with AppArmor enforcing, ufw default-deny, fail2ban,
hardened sysctl, ClamAV signatures, Magika type-checking, signed updates
(minisign + cosign), and air-gapped builds for the most paranoid posture.
See [`documentation/docs/security/`](documentation/docs/security/) for the
full list.

## PGP

A PGP key for `security@deputyos.com` will be published at
`https://www.deputyos.com/.well-known/pgp-key.asc`. Until then, encrypted
reports can use `me@dipankar.name`'s key (advertised on keyservers).
