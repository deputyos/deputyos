# deputyos-desktop

One-click desktop launcher for deputyOS.

`deputyos-desktop` is the **end-user-facing** binary that turns "I want to
try deputyOS on my laptop" into "double-click this and a browser opens to
the wizard". It is **not** baked into appliance images — it is the host
that *runs* an appliance image.

The launcher mandates the host's native virtualization stack:

- **Linux**: `qemu-system-x86_64` / `qemu-system-aarch64` + KVM.
- **Windows**: WSL2 (`wsl --install -d Ubuntu`).
- **macOS**: UTM 4.x (Vz.framework on Apple Silicon, qemu fallback on Intel).

There is **no bundled hypervisor**. The launcher prints a clear install
hint when its prereq is missing and refuses to download an image until
that prereq is satisfied.

## Synopsis

```
deputyos-desktop [<command>] [options]
```

Invoking with **no subcommand** mimics double-click: install if needed,
start, then open the wizard URL in the default browser.

## Global options

| Flag | Effect |
|---|---|
| `-h, --help` | Print help. |
| `-V, --version` | Print version. |

## Logging

`tracing-subscriber` is opt-in via `RUST_LOG`; default is **off** so the
launcher prints its own clear `==>`-prefixed stderr lines without a logger
prefix in normal use. Set `RUST_LOG=debug` to see internal trace events.

## Per-platform binary location

The launcher binary is **per-platform** — one binary per target triple, so
the macOS bundle never carries `wsl --import` symbol references and the
Linux binary never carries UTM bindings. End users get:

| Host | Distributed as | Default install location |
|---|---|---|
| Linux | tarball + `deputyos-desktop` ELF | `~/.local/bin/deputyos-desktop` (or wherever the user `mv`'d it) |
| Windows | `.exe` inside an installer | `%LOCALAPPDATA%\Programs\deputyos-desktop\deputyos-desktop.exe` |
| macOS | `.app` bundle (or bare ELF) | `/Applications/deputyos-desktop.app/Contents/MacOS/deputyos-desktop` |

## Per-platform paths

The launcher uses `dirs` crate semantics for cache, data, and runtime
directories.

| Host | Cache (image storage) | Data (PID file, last manifest) | Runtime |
|---|---|---|---|
| Linux | `~/.cache/deputyos-desktop` (or `$XDG_CACHE_HOME/deputyos-desktop`) | `~/.local/share/deputyos-desktop` | `/run/user/<uid>/deputyos-desktop` |
| Windows | `%LOCALAPPDATA%\deputyos-desktop` (cache subdir) | `%LOCALAPPDATA%\deputyos-desktop` (data subdir) | falls back to `data` |
| macOS | `~/Library/Caches/deputyos-desktop` | `~/Library/Application Support/deputyos-desktop` | falls back to `data` |

Every getter is overridable via environment for tests:

| Variable | Overrides |
|---|---|
| `DEPUTYOS_DESKTOP_CACHE_DIR` | cache dir |
| `DEPUTYOS_DESKTOP_DATA_DIR` | data dir |
| `DEPUTYOS_DESKTOP_RUNTIME_DIR` | runtime dir |
| `DEPUTYOS_DESKTOP_PUBKEY` | minisign public key path |
| `DEPUTYOS_DESKTOP_MANIFEST_URL` | manifest URL (default `https://cdn.deputyos.com/dev/manifest.json`) |
| `DEPUTYOS_DESKTOP_WIZARD_URL` | wizard URL the launcher opens (default `http://localhost:8088`) |

## Driver implementation status

| Driver | Status | What works today |
|---|---|---|
| Linux | **Real** | qemu spawn with KVM, hostfwd `:8088` and `:8089`, PID file under runtime dir, graceful stop. |
| Windows | Implemented; hardware smoke pending | WSL2 import/start/stop/status with a named distro per instance and `netsh portproxy` host-port remapping. |
| macOS | Implemented; hardware smoke pending | UTM create/start/stop/status with a named VM per instance. UTM cannot remap host ports, so use one default-port local instance at a time. |

A host whose OS is not Linux/Windows/macOS gets the `UnsupportedDriver`
fallback, which errors helpfully on every method.

## Per-host VM target

The `target_for_host()` driver method picks which manifest artefact this
host needs:

| Host arch / OS | `target_for_host` |
|---|---|
| Linux x86_64 | `qemu-x86_64` |
| Linux aarch64 | `qemu-aarch64` |
| Windows | `wsl2` |
| macOS | `macos-qemu` |
| other | `unsupported` |

## Exit codes

- `0` — success.
- `1` — anyhow error (printed with full chain via `{err:#}`). Includes
  prereq missing, manifest fetch / verification failure, download failure,
  driver error.

---

## Commands

### (no subcommand) — default action

Install-if-needed, start, open browser.

#### Synopsis

```
deputyos-desktop
```

#### Behavior

1. Resolve the driver via `current_driver()` (compile-time gated).
2. Check whether `<cache>/deputyos-<target>.qcow2` exists.
3. If missing: run [`install`](#install).
4. Run [`start`](#start).

This is the **double-click path** for non-technical users.

#### Examples

```
$ deputyos-desktop
==> deputyos-desktop (no subcommand) — install-if-needed + start + open browser
==> deputyos-desktop install
==> fetching manifest: https://cdn.deputyos.com/dev/manifest.json
==> selected artefact: deputyos-openclaw-qemu-x86_64-2026.4.27.qcow2 (1873294848 bytes)
==> downloading + verifying...
==> install complete
==> deputyos-desktop start
==> linux driver: spawning /usr/bin/qemu-system-x86_64
==> wizard at http://localhost:8088
```

---

### `install`

Verify prereq, fetch + verify manifest, download + verify image, stage in
cache.

#### Synopsis

```
deputyos-desktop install
```

#### Behavior

1. **Driver prereq check.** Runs the platform-specific check:
    - Linux: `qemu-system-{x86_64|aarch64}` on PATH; soft warn if `/dev/kvm` not present.
    - Windows: `wsl --status` (today: hard error with install hint).
    - macOS: `/Applications/UTM.app` exists (today: hard error with install hint).
2. **Manifest fetch.** Fetch the manifest at `DEPUTYOS_DESKTOP_MANIFEST_URL`
   (or the placeholder default).
3. **Manifest verification.** Verify the manifest's minisign signature
   against `DEPUTYOS_DESKTOP_PUBKEY` (defaults to the dev key at
   `~/.config/deputyos/dev-keys/deputyos-dev.pub`; production builds will
   embed the public key — tracked as M2.5-rest).
4. **Pick artefact.** From `manifest.artefacts`, pick the first whose
   `target` equals `driver.target_for_host()`.
5. **Download.** Download the artefact and its `.minisig` to
   `<cache>/deputyos-<target>.qcow2[.minisig]`. Verify SHA256, then
   verify the minisign signature.
6. **Driver `install_image`.** Per-platform import step (no-op on Linux
   since qemu reads the image directly).

#### Files written

- `<cache-dir>/deputyos-<target>.qcow2`
- `<cache-dir>/deputyos-<target>.qcow2.minisig`

#### Exit codes

- `0` — image staged.
- `1` — prereq missing, signature failed, network failure, or driver error.

#### Related

- [security/update-trust-chain](../../security/update-trust-chain.md) — same minisign trust chain as deputyctl `update`.
- [reference/schemas/release-manifest](../schemas/release-manifest.md)

---

### `start`

Boot the VM, open browser. Idempotent.

#### Synopsis

```
deputyos-desktop start
```

#### Behavior

1. Run the driver's prereq check.
2. Call `driver.start()`. On Linux:
   - Build the qemu argv with KVM acceleration (or TCG fallback if
     `/dev/kvm` is unavailable).
   - Set up `hostfwd` for ports `8088` (wizard) and `8089` (PWA) to
     `localhost`.
   - Spawn qemu detached, write its PID to
     `<runtime>/deputyos-desktop.pid`.
3. Resolve `wizard_url` (`DEPUTYOS_DESKTOP_WIZARD_URL` or
   `http://localhost:8088`) and call `browser::open_url`.
4. Print `==> wizard at <url>`.

If a VM is already running (Linux: PID file exists and `kill -0` succeeds),
`start` returns the existing handle without spawning a second qemu — the
operation is idempotent.

#### Files written

- `<runtime-dir>/deputyos-desktop.pid` (Linux).

#### Exit codes

- `0` — VM running, browser opened (or browser failed to open — that's
  best-effort).
- `1` — prereq missing or qemu spawn failed.

---

### `stop`

Graceful shutdown.

#### Synopsis

```
deputyos-desktop stop
```

#### Behavior

Linux: read the PID file, send `SIGTERM` to qemu. Optionally a `system_powerdown`
via the qemu monitor for a clean ACPI shutdown (the qemu monitor may take
~10s to drain disk caches; use `stop` then wait if the user wants
ACPI-clean).

#### Exit codes

- `0` — stop sent (or VM was already stopped).
- `1` — driver error.

---

### `status`

Print "running" or "stopped" and the wizard URL.

#### Synopsis

```
deputyos-desktop status
```

#### Behavior

Calls `driver.status()`:

- **Running**: prints `running (id=<pid-or-handle>)` followed by one URL
  per line (typically `http://localhost:8088`).
- **Stopped**: prints `stopped`.

#### Examples

```
$ deputyos-desktop status
running (id=12345)
  http://localhost:8088
```

```
$ deputyos-desktop status
stopped
```

#### Exit codes

- `0` — status printed.
- `1` — driver error (rare).

---

### `update`

Fetch the latest signed manifest and, if newer, download + verify + swap the
VM image. This updates the **image the launcher boots**, not the launcher
binary itself (use [`self-update`](#self-update) for that).

#### Synopsis

```
deputyos-desktop update
```

#### Behavior

1. Resolve `DEPUTYOS_DESKTOP_MANIFEST_URL` (default:
   `https://cdn.deputyos.com/dev/manifest.json`).
2. Fetch + sig-verify the manifest against the embedded pubkey (or
   `DEPUTYOS_DESKTOP_PUBKEY`).
3. Compare `release_version` to the cached `last-manifest.json`. If equal,
   print "up to date" and stop.
4. Pick the artefact for this host's driver target, download + sha256 +
   minisign verify it, atomically swap it into the cache, and record the
   new version. On next `start`, the new image boots.
5. If the manifest also advertises a newer launcher binary in
   `desktop_launchers[<host-triple>]`, print a one-line hint to run
   `deputyos-desktop self-update`. (The launcher is **not** auto-replaced
   here — the two swap operations are kept separate.)

#### Examples

```
$ deputyos-desktop update
==> fetching manifest: https://cdn.deputyos.com/dev/manifest.json
latest: 2026.6.22 (channel=dev)
downloading deputyos-openclaw-qemu-x86_64-2026.6.22-dev.qcow2 ...
update ready — run `deputyos-desktop start` to boot the new image
hint: a newer launcher binary is available; run `deputyos-desktop self-update` to replace it.
```

#### Exit codes

- `0` — up to date or image swapped.
- `1` — fetch / verification / swap failed.

---

### `self-update`

Download + verify + atomically replace **this launcher binary** from the
manifest's `desktop_launchers[<host-triple>]` entry. The VM image is
untouched. The new launcher takes effect on the next launch (the running
process is never replaced in memory). See
[desktop-launcher-internals § Launcher-binary self-update](../../distribution/desktop-launcher-internals.md#launcher-binary-self-update-deputyos-desktop-self-update)
for the per-OS swap semantics and the security note.

#### Synopsis

```
deputyos-desktop self-update
```

#### Behavior

1. Resolve `DEPUTYOS_DESKTOP_MANIFEST_URL` (placeholder → exit 0 with a
   notice, same as `update`).
2. Resolve the host's launcher target triple from `(OS, ARCH)` (e.g.
   `x86_64-unknown-linux-gnu`, `aarch64-apple-darwin`,
   `x86_64-pc-windows-msvc`). Unknown host → exit 1.
3. Fetch + sig-verify the manifest.
4. Look up `desktop_launchers[<host-triple>]` and compare its `sha256` to
   the running launcher's sha256. Equal → print "launcher up to date" and
   exit 0. Absent triple → exit 1 listing the available triples.
5. Download the launcher blob to `cache_dir()/launcher-staging/`, sha256 +
   minisign verify it against the same pubkey as `update`.
6. Atomically swap it over the running binary (POSIX `rename` with
   `chmod 0o755`; Windows `.exe.old` move-aside). On Windows, if the
   running exe can't be moved aside, exit 1 with "close all
   deputyos-desktop windows and re-run `deputyos-desktop self-update`".

#### Exit codes

- `0` — up to date or launcher swapped.
- `1` — fetch / verification / swap failed, host triple unknown, or
  `desktop_launchers` has no entry for this host's triple.

---

### `uninstall`

Remove the cached image. Optionally also wipe the persistent data dir.

#### Synopsis

```
deputyos-desktop uninstall [--data]
```

#### Options

- `--data` — also delete the persistent data directory.

#### Behavior

1. `rm -rf <cache-dir>` (the image + its .minisig).
2. If `--data`:
   - `rm -rf <data-dir>` (PID file, last-installed manifest, etc.).
   - Best-effort `rm -rf <runtime-dir>` if it differs from data dir
     (skip on `/run/user/<uid>` — we don't want to recursively delete
     the user's runtime dir).

!!! warning "Destructive"
    `--data` removes all desktop-launcher state. The next `start` will
    spawn a fresh VM with whatever the cached image contains; if the cache
    is also gone (`uninstall` deleted it), `start` will fail until you
    `install` again.

#### Examples

```
$ deputyos-desktop uninstall
==> deputyos-desktop uninstall
==> removed cache: ~/.cache/deputyos-desktop
```

```
$ deputyos-desktop uninstall --data
==> deputyos-desktop uninstall
==> removed cache: ~/.cache/deputyos-desktop
==> removed data: ~/.local/share/deputyos-desktop
```

#### Exit codes

- `0` — removed (or nothing to remove).
- `1` — filesystem error.

---

## See also

- [distribution/desktop-launcher-internals](../../distribution/desktop-launcher-internals.md) — driver architecture, CDN URL pattern, future M2.5-rest plan.
- [distribution/hardware-matrix](../../distribution/hardware-matrix.md) — which `target` an artefact corresponds to.
- [reference/schemas/release-manifest](../schemas/release-manifest.md) — the manifest the launcher consumes.
- [security/update-trust-chain](../../security/update-trust-chain.md) — minisign signature flow shared with [deputyctl update](deputyctl.md#update).
- [deputywizard](deputywizard.md) — what the launcher opens in the browser after `start`.
