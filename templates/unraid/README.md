# deputyOS on Unraid

Unraid VM Manager runs libvirt under the hood, so deputyOS slots in as
a standard libvirt domain. We ship the [`deputyos.xml`](deputyos.xml)
template and the qcow2 — you place both, click Create, done.

See [docs/14-limitations.md §proxmox / unraid / truenas](../../docs/14-limitations.md#proxmox--unraid--truenas)
for the support boundary.

## What you need

- Unraid 6.10+ (libvirt 7+).
- VM Manager enabled (Settings → VM Manager → Enable VMs: Yes).
- A bridge interface (`br0` is Unraid's default; if you renamed it,
  edit `deputyos.xml` accordingly).
- The deputyOS qcow2 — `qemu-x86_64` is the right choice for almost
  every Unraid host (Unraid is x86_64-only).

## Step-by-step

1. **Place the qcow2.** Unraid's VM Manager looks under
   `/mnt/user/domains/<vmname>/` by default. Create the directory and
   copy the qcow2 in:

    ```bash
    # On the Unraid host (via SSH or web terminal):
    mkdir -p /mnt/user/domains/deputyos
    cp /tmp/qemu-x86_64-openclaw.qcow2 /mnt/user/domains/deputyos/deputyos.qcow2
    ```

2. **Drop the template** into Unraid's user-template directory:

    ```bash
    mkdir -p /boot/config/plugins/dynamix/templates-user
    cp deputyos.xml /boot/config/plugins/dynamix/templates-user/deputyos.xml
    ```

3. **In the Unraid web UI** (`http://tower.local`):

    - VMs → Add VM → Linux.
    - Top-right "Template" dropdown → choose `deputyos`.
    - Confirm: Memory 2048 MB, vCPUs 2, OVMF firmware, virtio NIC on
      `br0`, virtio disk pointing at
      `/mnt/user/domains/deputyos/deputyos.qcow2`.
    - Click Create.

4. **Start the VM.** Click the deputyOS icon → Start. First boot takes
   ~90 s.

## After boot

Find the VM's IP from Unraid VMs page (or run
`virsh domifaddr deputyos` on the host). Open
`http://<vm-ip>:8088` in a browser, then run `deputyctl init` from a
TTY (or SSH).

## Adjusting the template

Edit `deputyos.xml` before creating the VM if you want:

- **More RAM** — change `<memory>` and `<currentMemory>` (KiB).
  4 GB = `4194304`.
- **More vCPUs** — change `<vcpu placement="static">` and the
  `<topology>` element underneath.
- **Different bridge** — `<source bridge="br0"/>` → your bridge.
- **Different qcow2 path** — `<source file="..."/>` under the boot
  disk.

## Updates

A/B updates aren't wired for Unraid. To upgrade:

1. Stop the VM in Unraid VMs page.
2. Take an Unraid snapshot of the qcow2 (or just `cp` it):

    ```bash
    cp /mnt/user/domains/deputyos/deputyos.qcow2 /mnt/user/domains/deputyos/deputyos.qcow2.pre-update
    ```

3. Replace the qcow2 with the new one, keeping the same path:

    ```bash
    cp /tmp/qemu-x86_64-openclaw-NEWVERSION.qcow2 /mnt/user/domains/deputyos/deputyos.qcow2
    ```

4. Start the VM. If anything's wrong, stop and restore the
   `.pre-update` copy.

## Limitations

- We don't actively test Unraid builds.
- USB / PCIe passthrough (Coral, Hailo, NVIDIA) is your problem;
  Unraid's VM Manager exposes the knobs but deputyOS does not bundle
  the runtimes.
- The qcow2 lives on Unraid's user-share by default — fine for
  performance on cache pools, but slow on parity-protected array
  storage. Move to `cache` for sustained workloads.
