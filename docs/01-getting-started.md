# 01 — Getting started

This guide takes you from "I have hardware in my hand" to "I'm chatting with my agent" in about 10 minutes. We'll use a Raspberry Pi 5 with OpenClaw as the running example. The path for other targets is in [03-image-builds.md](03-image-builds.md).

## Don't have hardware yet?

Try the appliance in a local VM first — no Pi required:

```sh
curl -fsSL https://www.deputyos.com/try.sh | bash
```

Boots the latest signed image in **UTM (macOS Apple Silicon)**, **qemu (Linux)**, or **WSL2 (Windows)**, forwards the wizard to `http://localhost:8088`. ~5 minutes including download. The lean profile, but enough to see the agent reply on the built-in chat. Full local-build path including matrix builds and reproducibility verify is in [15-local-build.md](15-local-build.md).

## What you'll need

- A Raspberry Pi 5 (8 GB or 16 GB), a USB-C power supply, and an SD card or NVMe drive (NVMe via the official PCIe HAT is faster and more reliable; SD is fine for trying it out).
- A laptop or desktop on the same WiFi network you want the Pi on.
- An API key from a model provider. OpenRouter is the easiest because one key gives you access to hundreds of models — sign up at [openrouter.ai](https://openrouter.ai). Anthropic, OpenAI, Google AI Studio, Ollama Cloud, and a dozen others also work; see [05-model-providers.md](05-model-providers.md).
- (Optional but recommended) A Backblaze B2 account or a Cloudflare account, if you want automated backups.

## Step 1 — Download and verify the image

Visit [deputyos.com](https://www.deputyos.com) (target domain — the picker page reads the latest signed manifest), pick **Pi 5** and **OpenClaw**, and download:

```
deputyos-openclaw-rpi5-<version>-stable.img.xz
deputyos-openclaw-rpi5-<version>-stable.img.xz.sha256
deputyos-openclaw-rpi5-<version>-stable.img.xz.minisig
```

Verify integrity:

```sh
sha256sum -c deputyos-openclaw-rpi5-<version>-stable.img.xz.sha256
minisign -V \
  -P <deputyOS public minisign key> \
  -m deputyos-openclaw-rpi5-<version>-stable.img.xz \
  -x deputyos-openclaw-rpi5-<version>-stable.img.xz.minisig
```

Both checks must pass before flashing.

## Step 2 — Pre-fill your network and SSH key

Decompress the `.img.xz` and mount the FAT (`/boot/firmware`) partition. Edit `deputyos.yaml`:

```yaml
hostname: my-agent
timezone: Europe/London
wifi:
  ssid: my-network
  psk: my-password           # leave blank for ethernet
ssh:
  authorized_keys:
    - ssh-ed25519 AAAA... me@my-laptop
```

If you skip this step the wizard will collect the same answers from a phone; pre-filling just saves a minute.

## Step 3 — Flash and boot

Use Raspberry Pi Imager (set "Use custom image" → pick the decompressed `.img`), or:

```sh
xzcat deputyos-openclaw-rpi5-<version>-stable.img.xz | sudo dd of=/dev/sdX bs=4M status=progress conv=fsync
```

(Replace `/dev/sdX` with your actual device. Wrong device = data loss. Double-check.)

Insert the SD/NVMe into the Pi and power it on. The first boot takes about 60 seconds — kernel comes up, security baseline starts, mDNS publishes `deputyos.local`, the wizard server listens on `:8088`, and a QR code appears on any attached HDMI monitor (and on the serial console).

## Step 4 — Run the wizard

Open `http://deputyos.local` from any device on the same network. (If mDNS is blocked on your network, the wizard prints its IP on the HDMI screen, and the QR code points directly at the IP-based URL.)

**The wizard knows your hardware** and shows live headroom (RAM, disk, channel cost) at each step. If a combination won't fit — e.g. enabling local LLM and voice and three webhook channels on a Pi 4 — it tells you up front, with the calculated numbers and a one-line recommendation. You should never be surprised by a constraint you could have known about; that's a [load-bearing principle for this project](14-limitations.md).

The wizard walks you through:

1. **Hostname & timezone** — pre-filled if you edited `deputyos.yaml`.
2. **WiFi** — same.
3. **Profile** — pick OpenClaw or Hermes. (You can switch later with `deputyctl profile switch`.)
4. **Model provider** — pick from the list (OpenRouter is recommended for first-time users), paste your API key. The wizard makes a 1-token chat completion to validate the key before persisting it. If it fails, you retry; nothing is half-saved.
5. **Channels** — tick the messaging platforms you want the agent to listen on. ufw will only open the relevant ports.
6. **Gateway allowlist** — for each channel, paste the user IDs allowed to talk to the agent. The default is "allowlist mode" — strangers cannot interact. (You can switch to "DM pairing" or "open" later if you know what you're doing.)
7. **Backups (optional)** — paste B2 keyID/applicationKey or Cloudflare R2 credentials. If you want the simplest path, sign into Cloudflare in the wizard and we'll provision an R2 bucket for you.
8. **Tailscale (optional)** — paste a one-off auth key if you want remote access from outside your LAN.

The wizard refuses to bring up channels until `deputyctl doctor` reports green. This is a deliberate guard: if anything in the security baseline is wrong, the channel ports stay closed.

## Step 5 — Talk to your agent

You can chat immediately at `http://deputyos.local/chat` (built-in private web chat — works without any external channel).

If you wired up Telegram, message the bot you connected. The agent will reply within a few seconds for cloud-hosted models.

The companion PWA at `http://deputyos.local/app` shows status, logs, today's spend (if your provider exposes usage), and a button to rotate keys.

## What just happened

Your Pi did **no** package installs. Every runtime and dependency was already inside the image. The wizard only collected configuration — keys, channel preferences, allowlists. The first network call to anything outside your LAN was when you sent your first message.

## Common things you'll want to do next

| Want | Command |
|---|---|
| See if everything's healthy | `deputyctl doctor` |
| Watch what the agent is doing | `deputyctl logs --follow` |
| Switch to Hermes | `deputyctl profile switch hermes` (downloads no software — Hermes is already in the image) |
| Change to a different model | `deputyctl model set` (interactive) |
| Take a backup right now | `deputyctl backup now` |
| Restore from yesterday's backup | `deputyctl restore --list` then `deputyctl restore --snapshot <id>` |
| Apply an update | `deputyctl update --check` then `deputyctl update --apply` |
| Wipe everything and start over | `deputyctl factory-reset` |

## What if it didn't work

See [10-troubleshooting.md](10-troubleshooting.md). Every documented failure mode has a one-line recovery command.

## Other targets

The flow above is identical for every hardware target. Substitute "flash to SD/NVMe" with:

| Target | First-boot step |
|---|---|
| Pi 4 | Same as Pi 5; download the `rpi4` image. |
| arm64 SBC | Same; download `arm64-generic`. The wizard asks you to pick your board's device-tree overlay. |
| x86 mini-PC | Flash to USB stick or internal SSD; UEFI boot. |
| WSL2 | `wsl --install -d deputyos` on Windows. Wizard appears at `http://localhost:8088`. |
| macOS | `deputyos-launch` script boots the qcow2 in UTM/OrbStack and forwards `:8088`. |
| DigitalOcean | Click "Deploy" on the 1-Click Marketplace listing. cloud-init runs the wizard non-interactively if you pre-fill userdata; otherwise SSH in and run `deputyctl init`. |
| Oracle Cloud Always-Free arm | Upload the cloud image, boot a 24 GB Ampere instance, run wizard via SSH. |
| Hetzner / Vultr / Linode | Use the published `cloud-init` recipe with their stock Ubuntu 24.04 image. |
| Fly.io | `fly launch` against the published OCI artefact. |
| Proxmox / Unraid / TrueNAS | Use the community template that wraps our qcow2. |
