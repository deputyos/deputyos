# cloud-init recipes

Cloud-init User-Data YAML you paste into a provider's instance-creation
form. We ship recipes here for providers where building a custom
snapshot isn't worth the maintenance overhead — the contract is the
recipe, not a signed image.

## Supported providers

| Provider | Recipe | Recommended size | Limits file |
|---|---|---|---|
| Hetzner Cloud | `hetzner.yaml` | CX22 (Arm, 4 GB) or CX21 (x86, 4 GB) | `roles/deputyos/files/limits.hetzner-cloud.json` |
| Vultr         | `vultr.yaml`   | VHF-1c-2gb (2 GB) or VC2-2c-4gb (4 GB) | `roles/deputyos/files/limits.vultr.json` |
| Linode (Akamai) | `linode.yaml` | Linode 4 GB or 8 GB | `roles/deputyos/files/limits.linode.json` |

All three providers support cloud-init User-Data natively. No extra
client-side tooling is required — just paste the YAML in the dashboard.

## How to use

1. Pick the provider's cheapest instance size that meets the
   recommended floor above (1 GB tiers are below the deputyOS minimum).
2. Open `cloud-init/<provider>.yaml`, replace the placeholders:
   - `<<<USER_SSH_PUBKEY>>>` — your SSH public key for the `admin` user.
3. Paste the YAML into the provider's User-Data / Cloud Config /
   Startup Script field at instance creation.
4. Boot the instance. First boot runs cloud-init, which fetches
   deputyOS, runs the Ansible role, and starts the gateway service.

## Distribution URL

The recipes pull the deputyOS installer from
`https://cdn.deputyos.com/install-cloud.sh`. Releases are published to the
Backblaze B2 origin and served through Cloudflare at that stable hostname.

## Why no Packer template

Maintaining a custom snapshot per provider triples the build matrix
without changing what the user gets. Cloud-init is the universal
substrate; once the M4 CDN ships the install script, the recipe
becomes self-bootstrapping. Updates on these targets are full
re-image cycles — see `docs/14-limitations.md` §"hetzner / vultr /
linode".
