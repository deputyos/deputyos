# deputyOS on macOS (demo)

The `macos-qemu` target lets a Mac user kick the tyres on deputyOS
without dedicated hardware. It is **demo-only**:

- Voice features are off (`/dev/snd` is not plumbed through QEMU).
- Local LLMs are off (2 GB default allocation, throttled emulation).
- A/B updates are not available; rollback = restoring the qcow2 file.
- Performance is roughly 5-10x slower than native Linux on the same
  Mac (Apple Silicon HVF helps; Intel TCG is the slow path).

If those limits are blockers, run deputyOS on a Pi 5, x86 mini-PC, or
one of the cloud targets instead. See
[../docs/14-limitations.md §macos-qemu](../docs/14-limitations.md#macos-qemu).

## Why no `packer/macos-qemu.pkr.hcl`?

The qcow2 the macOS launcher boots **is** the `qemu-aarch64` qcow2
that CI smoke-tests on every commit. There is no separate Packer
build for `macos-qemu`; `make build TARGET=macos-qemu` runs
`make build TARGET=qemu-aarch64` recursively, then copies the result
to `build/macos-qemu-<profile>.qcow2`. This way a single CI
smoke-test gates both targets and the Mac demo can never drift from
the headless server image.

The launcher choice (UTM vs OrbStack vs raw `qemu-system-aarch64`) is
a runtime decision, not a build-time decision.

## Two flavours

### UTM (recommended on Apple Silicon)

[UTM](https://mac.getutm.app) is a polished QEMU front-end with
Hypervisor Framework acceleration on M1/M2/M3/M4 (so the guest CPU
runs at near-native speed). On Intel Macs UTM falls back to TCG
(slow but functional).

```bash
brew install --cask utm
make build TARGET=macos-qemu PROFILE=openclaw
./macos/run-utm.sh
```

The script tries, in order:

1. `utmctl create + start` (fastest path; CLI ships with UTM).
2. `open -a UTM <qcow2>` (GUI fallback; user clicks through).
3. Bare `qemu-system-aarch64` (last resort; install qemu via Homebrew).

### OrbStack (lighter, Docker-shape UX)

[OrbStack](https://orbstack.dev) is a leaner Linux-VM-on-Mac runner
with built-in port publishing. Good fit if you already use OrbStack
for Docker.

```bash
brew install --cask orbstack
make build TARGET=macos-qemu PROFILE=openclaw
./macos/run-orbstack.sh
```

OrbStack auto-publishes guest ports to the host, so
`http://localhost:8088` reaches the wizard without manual port-forward
configuration in most cases.

## Feature parity

| Feature | UTM | OrbStack |
|---|---|---|
| Apple Silicon HVF acceleration | yes | yes |
| Intel-host TCG fallback | yes | yes |
| GUI VM management | yes (full) | partial |
| CLI VM management (`utmctl` / `orb`) | yes | yes |
| Auto port-forward to host | manual | yes |
| Cost | free | free for personal; paid for commercial |

Both runners boot the same qcow2, so what works in one works in the
other (modulo port-forward UX).

## Adjusting RAM / port forwards

Both launchers honour env vars:

```bash
MEMORY_MB=4096 HOSTPORT_WIZARD=8089 ./macos/run-utm.sh
```

## After boot

Inside the VM:

```bash
deputyctl init      # first-time setup
deputyctl limits    # confirms macos-qemu limits
```

From the Mac host:

```bash
open http://localhost:8088
```

## Troubleshooting

- **`No deputyctl found`** — the smoke build dropped the binary at
  `/usr/local/bin/deputyctl`. If the qcow2 is corrupt, rebuild with
  `make clean && make build TARGET=macos-qemu`.
- **UTM "Architecture not supported"** — UTM on Intel Macs needs the
  emulation backend. Confirm you're on a recent UTM (4.4+); reinstall
  if old.
- **OrbStack `qcow2 not recognised`** — older OrbStack versions don't
  accept qcow2 directly. Run
  `qemu-img convert -O raw build/macos-qemu-openclaw.qcow2 build/macos-qemu-openclaw.raw`
  and pass `--image .../raw` instead.
- **`http://localhost:8088` shows nothing** — wait 60 s for boot, then
  confirm port forward. UTM users: VM settings → Network → Port
  Forward, add TCP 8088 → 8088.
