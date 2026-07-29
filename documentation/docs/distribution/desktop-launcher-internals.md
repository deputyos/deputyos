# Desktop launcher internals

## What this page does

Architecture walkthrough of the `deputyos-desktop` crate — the
"download + double-click → wizard in browser" launcher for Linux,
Windows, and macOS. The launcher is a Rust binary per platform that
consumes existing deputyOS image artefacts (the same qcow2 / img.xz
files that ship for the QEMU and Pi targets) and orchestrates the
host's native virtualization to boot them.

The user-facing surface is documented in
[Reference → CLI → deputyos-desktop](../reference/cli/deputyos-desktop.md);
this page is the implementation-side reference for contributors and
auditors.

## Crate architecture

```text
deputyos-desktop/
├── src/
│   ├── main.rs        ← CLI dispatch (clap)
│   ├── lib.rs         ← public modules
│   ├── config.rs      ← cache locations, version pinning, prefs
│   ├── manifest.rs    ← reuses deputyctl::release types for verify
│   ├── download.rs    ← image fetch + sha256 + minisig verify
│   ├── browser.rs     ← cross-platform "open URL" shim
│   ├── driver.rs      ← Driver trait + `current_driver()` factory
│   └── drivers/
│       ├── mod.rs
│       ├── linux.rs   ← qemu+KVM driver
│       ├── windows.rs ← WSL2-mandate driver
│       └── macos.rs   ← UTM-mandate driver
└── Cargo.toml
```

## The user-facing CLI surface

```text
deputyos-desktop install                 # prereq + download + cache + import
deputyos-desktop start                   # boot VM + open browser. Default action.
deputyos-desktop stop                    # graceful shutdown
deputyos-desktop status                  # running/stopped + URL
deputyos-desktop update                  # check manifest + (M2.5-rest) apply
deputyos-desktop uninstall [--data]      # remove cache; --data also wipes data dir
```

Invoking with **no subcommand** mimics double-click: install (if
needed), start, then open the wizard URL in the default browser.
That's the "non-technical user double-clicks the icon" path.

## Driver model

Each platform implements the `Driver` trait:

```rust
pub trait Driver {
    fn check_prereqs(&self) -> Result<()>;
    fn start(&self, image_path: &Path) -> Result<VmStatus>;
    fn stop(&self) -> Result<()>;
    fn status(&self) -> Result<VmStatus>;
}
```

Where `VmStatus` reports running/stopped + the wizard URL (always
`http://localhost:8088/wizard` per the loopback-only design).

### Linux driver

- **Mandate**: qemu+KVM. The driver checks `/dev/kvm` is accessible
  and `qemu-system-x86_64` (or `qemu-system-aarch64`) is on PATH.
- **No bundled hypervisor.** Linux distros all ship qemu+KVM; we don't
  re-bundle.
- **Boot command**: roughly `qemu-system-<arch> -m 4G -accel kvm
  -drive file=<image>,if=virtio -netdev user,id=net0,
  hostfwd=tcp:127.0.0.1:8088-:8088 -device virtio-net,netdev=net0
  -nographic -daemonize -pidfile <cache>/vm.pid`.
- **Stop**: SIGTERM to the pid in the pidfile.
- **Status**: `kill -0 <pid>` plus `curl -fsS http://localhost:8088/healthz`.

### Windows driver

- **Mandate**: WSL2. The driver checks for the WSL2 distro that
  `deputyos-desktop install` imports (or that
  `wsl/Install-DeputyOS.ps1` imported earlier).
- **No bundled hypervisor.** WSL2 is the right shape on Windows;
  we don't run nested qemu in WSL.
- **Boot command**: `wsl --distribution deputyos --user agent
  /opt/deputyos/start.sh`.
- **Stop**: `wsl --terminate deputyos`.
- **Status**: `wsl --list --running` plus the healthz probe.

### macOS driver

- **Mandate**: UTM. The driver checks for UTM at
  `/Applications/UTM.app` and the `utmctl` CLI it ships.
- **No bundled hypervisor.** Apple's restrictions on third-party
  hypervisors plus UTM's existing ecosystem make UTM the right
  shape.
- **Apple Silicon (aarch64)**: HVF acceleration; near-native speed.
- **Intel x86_64 macOS**: TCG (slow) — supported but not recommended.
- **Boot**: `utmctl start deputyos`.
- **Stop**: `utmctl stop deputyos`.
- **Status**: `utmctl status deputyos`.

## Cross-compile per host triple

The `make desktop-launcher` target builds for the host triple by
default; pass `DESKTOP_TARGET=<rust-triple>` to cross-compile:

| Host | Target triple | Notes |
| --- | --- | --- |
| Linux x86_64 | `x86_64-unknown-linux-gnu` | native |
| Linux arm64 | `aarch64-unknown-linux-gnu` | native |
| Windows | `x86_64-pc-windows-msvc` | from Windows host |
| Windows (cross) | `x86_64-pc-windows-gnu` | from Linux via `cross` |
| macOS Intel | `x86_64-apple-darwin` | must build on a Mac |
| macOS Apple Silicon | `aarch64-apple-darwin` | must build on a Mac |

macOS builds need Apple's toolchain blobs we cannot legally
redistribute. macOS contributors build on a Mac.

## Manifest fetch + sha + minisig verify

The launcher reuses `deputyctl::release` types — the same code path the
on-device update verifier uses. Steps in `download.rs`:

1. Fetch `manifest.json` and `manifest.json.minisig` from the deputyOS
   CDN URL (`DEPUTYOS_DESKTOP_MANIFEST_URL` env override; or the baked
   default).
2. Verify the minisign signature against the pubkey baked into the
   binary at build time.
3. Find the artefact for the host's `(target, profile)` — for Linux
   this is `qemu-x86_64` or `qemu-aarch64`, for Windows it's `wsl2`,
   for macOS it's `macos-qemu`.
4. Download to the cache. Verify sha256. Verify per-artefact minisig.
5. Cache hit → use cached image.

## Cache locations

| Platform | Cache root |
| --- | --- |
| Linux | `${XDG_CACHE_HOME:-~/.cache}/deputyos-desktop/` |
| Windows | `%LOCALAPPDATA%\deputyos-desktop\` |
| macOS | `~/Library/Caches/deputyos-desktop/` |

Cache contents:

- `images/<release>/<artefact>` — downloaded and verified images.
- `vm.pid` (Linux) — running VM PID.
- `state.json` — current install state.

`deputyos-desktop uninstall` removes the cache. `--data` also wipes
the persistent data dir at:

| Platform | Data dir |
| --- | --- |
| Linux | `${XDG_DATA_HOME:-~/.local/share}/deputyos-desktop/` |
| Windows | `%APPDATA%\deputyos-desktop\` |
| macOS | `~/Library/Application Support/deputyos-desktop/` |

## Code-signing per platform

Distributing the binary needs platform-specific signing — without it,
Gatekeeper / SmartScreen will block the launch.

| Platform | Signing requirement | Status |
| --- | --- | --- |
| Linux | gpg-signed AppImage | M2.5-rest infra |
| Windows | Authenticode EV cert | M2.5-rest infra |
| macOS | Apple Developer ID + notarization | M2.5-rest infra |

The CI signing infrastructure for these is tracked in M2.5-rest. Today
the launcher binaries are unsigned — distribution is "build it
yourself with `make desktop-launcher`" pending the signing path.

## Browser open

`browser.rs` is a thin shim over the platform default. Linux:
`xdg-open`. Windows: `start` (via cmd). macOS: `open`. The launcher
always opens `http://localhost:8088/wizard`; the wizard's first-boot
state is persistent in the data dir, so successive `start` invocations
go straight to whatever step the user left off.

## Update flow

Two independent update paths, each its own trust-and-swap operation:

### Image update — `deputyos-desktop update`

1. Fetch the latest manifest (sha + minisign verified against the embedded
   pubkey).
2. Compare `release_version` to the cached `last-manifest.json`.
3. If newer: download + sha + minisign verify the new image, atomically
   swap it into the cache (the old image is removed only after the new one
   verifies), and record the new version.
4. On next `start`, boot the new image.

After the image work, `update` prints a one-line hint if the manifest also
advertises a newer launcher binary (see below) — but it never replaces the
launcher itself; that's a separate command so a single invocation doesn't
perform two self-replacements.

The launcher does not support A/B for the desktop image the way the
appliance images do (M6 territory). Replacement is in-place.

### Launcher-binary self-update — `deputyos-desktop self-update`

`deputyos-desktop self-update` replaces **this launcher binary** from the
manifest's `desktop_launchers[<host-triple>]` entry — so a published release
can ship a new launcher without making the user re-download it by hand. The
host triple is distinct from the driver's image target: it's the Rust target
triple the launcher was built for (`x86_64-unknown-linux-gnu`,
`aarch64-apple-darwin`, `x86_64-pc-windows-msvc`, …), mapped in `config.rs`
from `std::env::consts::{OS, ARCH}`.

1. Fetch + verify the manifest.
2. Look up `desktop_launchers[<host-triple>]`; compute the sha256 of the
   running launcher binary and compare it to the entry's `sha256`. Equal →
   "up to date" and stop. (A same-version rebuild with a different sha still
   counts as an update — there's no `version` field, the sha *is* the
   identity. An absent triple errors with the available triples listed.)
3. Download the launcher blob to `cache_dir()/launcher-staging/`, sha256 +
   minisign verify it against the same embedded pubkey the image path uses
   (`download::download_and_verify`).
4. Atomically swap it over the running binary — per-OS:
   - **POSIX** (Linux/macOS): `chmod 0o755` the staged file, then `rename`
     it over the current executable. `rename` is atomic on the same
     filesystem; the running process keeps the old inode, so the new
     launcher takes effect on the **next launch** (the running process is
     never replaced in memory).
   - **Windows**: a running `.exe` can't be renamed over in place, so the
     current binary is moved aside to `<name>.exe.old`, the new one moved
     into place, and the `.old` is best-effort deleted (it may still be
     locked by the running process — a future launch reaps it). If moving
     the running exe aside fails with a sharing violation, `self-update`
     bails with "close all deputyos-desktop windows and re-run
     `deputyos-desktop self-update`."

## Security: self-replacing a signed binary

Self-replacing the launcher is trust-critical — a compromised CDN or MITM
that could push an attacker binary would own every desktop install. The
mitigation is that the new launcher is verified exactly like an image:
**sha256 + detached minisign against the same embedded pubkey**, before it
ever touches the on-disk executable. A blob that fails either check is
discarded; the on-disk binary is untouched. The swap itself is atomic on
POSIX (`rename`), so there's no window where the executable is missing or
half-written. The running process is never replaced in memory — only the
file on disk — which means a malicious new binary can't take over a
running session; it can only run on the next launch, by which point the
user has already trusted the launcher's update path.

The up-to-date gate is a sha256 compare, not a version compare: this avoids
a schema change and means a published rebuild (different bytes, same
version) is still picked up. The trade-off is that a byte-identical
re-publish is a no-op and a different-bytes same-version rebuild triggers a
"self-update" that's really a refresh — acceptable, since the new binary
still verifies. SNI/TLS-interception is out of scope; transport trust is
the existing TLS + minisign pipeline `deputyctl update` already relies on.

## Verification

```sh
# Build for host
cargo build --release -p deputyos-desktop

# Run install + start (interactive)
./target/release/deputyos-desktop install
./target/release/deputyos-desktop start

# Status / logs
./target/release/deputyos-desktop status
```

For a full end-to-end loop — a Docker-hosted CDN + API that the installer
pulls from, booting a real qemu-x86_64 VM whose agent talks to the local API
via a cloud-init seed (the `DEPUTYOS_DESKTOP_SEED_ISO` path the LinuxDriver
attaches) — see
[How-to → Develop → Run deputyOS locally](../how-to/develop/run-locally.md).
`make desktop-local-build && make cdn-up && make desktop-local` drives the
whole thing.

## Related

- [Reference → CLI → deputyos-desktop](../reference/cli/deputyos-desktop.md)
- [Distribution → Hardware matrix](hardware-matrix.md)
- [Build → Make targets](../build/make-targets.md) (the `desktop-launcher` target)
- [Security → Update trust chain](../security/update-trust-chain.md)
- [Operations → Update and rollback](../operations/update-and-rollback.md)
