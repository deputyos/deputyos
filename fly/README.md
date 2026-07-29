# deputyOS on Fly Machines

Fly Machines run an OCI artefact (Docker-image-compatible). The product
in this directory is:

- `Containerfile` — Buildah/Docker-buildable image. Bakes the shared
  Ansible role at image build time, same as Packer does for QEMU/DO.
- `fly.toml.example` — copy → edit → `flyctl launch --copy-config`.

## Quickstart

```sh
# 1. Build the deputyctl binary + stage role artefacts:
make build TARGET=fly-machines PROFILE=openclaw

# 2. Copy the fly.toml template:
cp fly/fly.toml.example fly.toml
$EDITOR fly.toml          # set app name, region, machine size

# 3. Create the persistent volume (once per region):
flyctl volumes create deputyos_data --size 10 --region iad

# 4. Launch:
flyctl launch --copy-config
# or, if the app already exists:
flyctl deploy
```

`make build TARGET=fly-machines` runs `buildah build` (or `docker build`
if Buildah isn't installed) with the staging artefacts produced by
`scripts/build.sh`. The container image is left in your local image
store; `flyctl deploy` pushes it to Fly's registry.

## How `deputyctl up` works without systemd

Every other deputyOS target runs the gateway as a systemd unit
(`openclaw-gateway.service` or `hermes-gateway.service`). On Fly
Machines the container PID 1 is the agent itself — there is no systemd
graph inside the container.

The Containerfile's CMD is `deputyctl up --foreground`. The CLI
subcommand:

- on systemd targets, shells out to `systemctl start <profile>-gateway`
  and tails the journal until interrupted;
- on container targets (cloud=fly), execs the profile's gateway binary
  directly, inheriting the container's stdio so Fly's logs panel works.

The `--foreground` flag forces the second behaviour even on systemd
hosts (useful for `flyctl ssh console` debugging when troubleshooting
a deployed Fly machine that itself runs systemd in a side-app).

## Limitations on this target (per `docs/14-limitations.md`)

- **Ephemeral storage.** `/home/agent` is a Fly volume; if you delete
  the volume, agent state is gone. Backups still go to the user's
  B2/R2 bucket.
- **Cold-start latency.** Free Machines auto-stop on idle and take
  several seconds to cold-start. Telegram-class polling channels
  tolerate it; voice is disabled on this target anyway.
- **AppArmor in complain mode.** Enforce mode requires capabilities
  Fly's default machine config doesn't grant. Profiles are installed
  but unloaded; the `deputyos_apparmor_mode=complain` fact is set by
  `roles/deputyos/tasks/variant-fly-machines.yml`.
- **No clamd.** RAM headroom is too tight; on-demand `clamscan`
  triggered by Magika hints replaces the daemon.
- **No audio.** Voice features auto-disable.

## Notes for contributors

- The `RUN ansible-playbook` step in the Containerfile uses `|| true`
  for the trailing handler stage. systemd-touching tasks fail in a
  container without a running pid1 systemd; the variant gates skip the
  enable/start, but a stray handler can still trigger. The variant
  task's `deputyos_apparmor_mode=complain` fact + the role's container
  detection short-circuits most of these. If you add a task that
  fundamentally needs systemd, gate it with
  `when: deputyos_hw != "fly-machines"`.
- The container image isn't a stripped artefact yet (M2 ships full
  Ansible inside); a multi-stage build that drops Ansible after the
  bake step lands in M4 alongside CDN-served bundles.
