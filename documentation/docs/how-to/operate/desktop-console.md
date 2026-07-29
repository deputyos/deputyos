# Use the deputyOS Console desktop app

## What this guide does

Install and run the **deputyOS Console** — the cross-platform desktop app that
logs into your deputyOS account and manages your agents: create, install, and
start local VM-based agents (openclaw, hermes) on your own machine, and open or
manage the devices in your remote fleet.

The Console is a thin GUI over the same engine the `deputyos-desktop` launcher
uses: it downloads the signed appliance image for your host, boots it in your
host's native virtualization, and opens the first-boot wizard in your browser.

## Prerequisites — host virtualization

The Console does **not** bundle a hypervisor; it drives your platform's native
one. Install the right one for your OS first:

| OS | Virtualization | Install |
|----|----------------|---------|
| **Linux** | qemu + KVM | `sudo apt install qemu-system-x86` (or your distro's package). KVM (`/dev/kvm`) is strongly recommended; without it the VM falls back to slow emulation. |
| **Windows** | WSL 2 | In an **admin** PowerShell: `wsl --install`, then reboot. Must be WSL 2 (not 1). |
| **macOS** | UTM (Apple Silicon only) | Install [UTM](https://mac.getutm.app/) 4.x (App Store or `brew install --cask utm`). Intel Macs are not supported. |

If a prerequisite is missing, the Console shows a banner on the **Local** tab
with the exact install hint instead of failing silently.

## Install the Console

Download the installer for your OS from the
[GitHub Releases](https://github.com/deputyos/deputyos/releases) page:

- **Linux** — `.AppImage` (chmod +x and run) or `.deb` (`sudo apt install ./deputyos-console_*.deb`).
- **macOS** — `.dmg` — drag **deputyOS Console** to Applications.
- **Windows** — `.msi` (or the NSIS `.exe`).

### Unsigned builds — bypassing the OS warning

Pre-release Console builds are **not yet code-signed**, so the OS will warn the
first time you open them. This is expected; the download is still integrity-checked
(the appliance images it fetches are minisign-verified regardless).

- **macOS (Gatekeeper):** right-click the app → **Open** → **Open** again in the
  dialog. (Or: System Settings → Privacy & Security → **Open Anyway**.)
- **Windows (SmartScreen):** click **More info** → **Run anyway**.
- **Linux:** no warning; just make the AppImage executable.

## First run

1. **Sign in.** The Console shows a device-code login: it displays a short code
   and a link (`app.deputyos.com/device`). Open the link, enter the code, confirm —
   the Console picks up your session automatically.
2. **Create a local agent.** On the **Local** tab, enter a name and pick a
   profile (`openclaw` or `hermes`), then **+ Create**.
3. **Install** the agent's image — the Console downloads the signed qcow2/rootfs
   for your host and verifies it (sha256 + minisign).
4. **Start** it — the VM boots and the row shows **running** (green). Click the
   **Wizard** link to open the first-boot wizard in your browser and finish setup.
5. Repeat to run **both** openclaw and hermes side by side — each gets its own
   ports and storage.

## Managing your remote fleet

The **Fleet** tab lists the devices registered to your account. For an online
device, **open** launches its wizard through the account tunnel (no VPN, no port
forwarding) using your signed-in session.

## Multiple local agents — platform notes

Running several local agents at once works best on **Linux**, where each VM gets
its own distinct host ports automatically. On **Windows**, the Console installs a
`netsh portproxy` remap per agent so each gets a distinct `localhost` port (this
may need an elevated shell; if the remap can't be set up, the agent still boots
but is reachable only on the default `localhost:8088`). On **macOS/UTM**, host
ports can't be remapped — run one UTM-based agent at a time, or use the default
ports.

## Troubleshooting

- **"local agents require Linux/…" or an install hint on the Local tab** — the
  host virtualization prerequisite above isn't installed or ready.
- **VM won't boot / very slow (Linux)** — `/dev/kvm` is missing; add your user to
  the `kvm` group or enable virtualization in BIOS.
- **Wizard link does nothing** — the Console opens URLs in your default browser;
  if none is set, copy the `http://localhost:<port>` URL shown and open it manually.
- **CLI alternative** — everything the Console does is also available from the
  `deputyos-desktop` terminal launcher; see
  [Desktop launcher internals](../../distribution/desktop-launcher-internals.md).
