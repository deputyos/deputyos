# deputyOS on Proxmox VE

Proxmox is a community-supported target. We do not ship a `.vma`
backup or a Proxmox-native template — the agent ships as a `qcow2`
that you import into a VM you create.

See [docs/14-limitations.md §proxmox / unraid / truenas](../../docs/14-limitations.md#proxmox--unraid--truenas)
for the support boundary: USB passthrough, NIC bonding, and ZFS
dataset tunings are out-of-scope.

## What you need

- Proxmox VE 7.4+ or 8.x.
- A storage pool that holds qcow2 (`local-lvm`, `local-zfs`, or
  `local-btrfs` — adjust commands below to match).
- A bridge for guest networking (`vmbr0` is the Proxmox default).
- The deputyOS qcow2 — either the `qemu-aarch64` or `qemu-x86_64`
  variant.

## Which qcow2?

Both work. Pick to match your Proxmox host's CPU:

| Host CPU | Recommended qcow2 | Why |
|---|---|---|
| x86_64 (Intel / AMD) | `qemu-x86_64` | Native KVM acceleration. Default. |
| arm64 (a Pi 5 or Ampere running Proxmox) | `qemu-aarch64` | Native KVM. Rare host but supported. |
| x86_64 host wanting to run the arm64 image | `qemu-aarch64` | Will work via TCG; expect ~10x slowdown. Don't do this for production. |

Download links from [deputyos.com](https://www.deputyos.com) → "Other
hardware" → Proxmox.

## Step-by-step

1. **Copy the qcow2 to the Proxmox host.**

    ```bash
    scp build/qemu-x86_64-openclaw.qcow2 root@proxmox.local:/tmp/
    ```

2. **Pick a free VMID** (we'll use `200` below).

3. **Create the VM shell** on the Proxmox host:

    ```bash
    qm create 200 \
      --name deputyos \
      --memory 2048 \
      --cores 2 \
      --cpu host \
      --bios ovmf \
      --machine q35 \
      --net0 virtio,bridge=vmbr0 \
      --scsihw virtio-scsi-pci \
      --agent 1 \
      --serial0 socket --vga serial0 \
      --ostype l26
    ```

4. **Add an EFI vars disk** (OVMF needs one):

    ```bash
    qm set 200 --efidisk0 local-lvm:1,efitype=4m,pre-enrolled-keys=0,format=qcow2
    ```

    Substitute your storage pool for `local-lvm` if needed.

5. **Import the qcow2 as the boot disk:**

    ```bash
    qm importdisk 200 /tmp/qemu-x86_64-openclaw.qcow2 local-lvm --format qcow2
    ```

    Proxmox prints something like
    `Successfully imported disk as 'unused0:local-lvm:vm-200-disk-1'`.

6. **Attach the imported disk and set boot order:**

    ```bash
    qm set 200 --scsi0 local-lvm:vm-200-disk-1
    qm set 200 --boot order=scsi0
    ```

7. **Start the VM:**

    ```bash
    qm start 200
    ```

8. **Watch first boot on the serial console:**

    ```bash
    qm terminal 200
    ```

    deputyOS reaches `multi-user.target` within ~90 s. Press `Ctrl-O` /
    `Ctrl-]` per the on-screen instructions to detach.

## After boot

From any host on the same bridge as the VM:

```
http://<vm-ip>:8088
```

(`qm guest cmd 200 network-get-interfaces` reveals the IP once the
qemu-guest-agent has registered, ~10 s after boot.)

Then run `deputyctl init` from a TTY (or SSH).

## Updates

`deputyctl update` does not work on Proxmox today (A/B partitions
require platform-specific bootloader integration we haven't built).
To update:

1. Take a Proxmox snapshot: `qm snapshot 200 pre-update`.
2. Stop the VM: `qm stop 200`.
3. Re-import the new qcow2 over the existing disk:

    ```bash
    # Detach old disk
    qm set 200 --delete scsi0
    qm importdisk 200 /tmp/qemu-x86_64-openclaw-NEWVERSION.qcow2 local-lvm
    qm set 200 --scsi0 local-lvm:vm-200-disk-2
    qm set 200 --boot order=scsi0
    qm start 200
    ```

4. If anything's wrong: `qm rollback 200 pre-update`.

## Reference template

[`deputyos.vm-config`](deputyos.vm-config) in this directory is the
on-disk shape of `/etc/pve/qemu-server/200.conf` after the steps
above. Inspect it to confirm what your VM should look like; do
**not** copy it directly into `/etc/pve/qemu-server/` (Proxmox
manages those files itself).

## Limitations

- We don't actively test Proxmox builds. Bug reports welcome but
  triaged behind first-class targets.
- USB passthrough, PCIe passthrough (NPUs, GPUs) is your problem to
  configure — see Proxmox docs.
- ZFS-backed storage is fine; we don't tune `zfs set` properties for
  the deputyOS dataset.
