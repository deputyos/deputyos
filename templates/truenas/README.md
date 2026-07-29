# deputyOS on TrueNAS Scale

TrueNAS Scale runs libvirt-shape VMs against zvols on the host's ZFS
pool. The deputyOS qcow2 is converted to a raw zvol and attached as a
virtio-blk disk.

See [docs/14-limitations.md §proxmox / unraid / truenas](../../docs/14-limitations.md#proxmox--unraid--truenas)
for the support boundary.

## What you need

- TrueNAS Scale 22.12+ (Bluefin or newer).
- A zpool with at least 8 GB free for the boot disk (the `tank`
  default below; adjust to your zpool name).
- A bridge interface for the VM (`br0` is SCALE's default).
- The deputyOS qcow2 — `qemu-x86_64` for x86 hosts (the common case),
  `qemu-aarch64` for the rare arm64 SCALE deployment.

## Step-by-step

1. **Copy the qcow2 to the SCALE host** (via Shell, `scp`, or SMB):

    ```bash
    scp build/qemu-x86_64-openclaw.qcow2 root@truenas.local:/mnt/tank/iso/
    ```

2. **Create a zvol** for the VM disk:

    ```bash
    # On the SCALE host shell:
    zfs create -V 8G tank/deputyos
    ```

    Substitute `tank` with your zpool. Verify:

    ```bash
    ls -l /dev/zvol/tank/deputyos
    ```

3. **Write the qcow2 contents into the zvol** as raw:

    ```bash
    qemu-img convert -O raw /mnt/tank/iso/qemu-x86_64-openclaw.qcow2 /dev/zvol/tank/deputyos
    ```

    (TrueNAS Scale's libvirt prefers raw zvols; qcow2-on-zvol works
    but loses snapshot-friendliness because both layers do COW.)

4. **Create the VM via the UI:**

    - Virtualization → Add → Linux.
    - Name: `deputyos`. Description: `deputyOS personal AI appliance`.
    - System Clock: `Local`. Boot Method: `UEFI`. Shutdown Timeout: 90.
    - vCPUs: 2 / Cores: 2 / Threads: 1. Memory: 2048 MB.
    - When prompted to add a disk: choose `Use existing disk image`,
      pick `tank/deputyos` as the zvol, type `VIRTIO`.
    - Network: `VirtIO`, attach to `br0`.
    - Confirm and create.

    The [`deputyos.json`](deputyos.json) template in this directory
    documents the equivalent `vm.create` + `vm.device.create` API
    payloads for users who prefer scripting via SCALE's
    middleware/WebSocket API.

5. **Start the VM** from the Virtualization page.

## After boot

In the SCALE UI: Virtualization → deputyos → Display → opens VNC in a
browser tab. Log in via the agent TTY, then run `deputyctl init`.

Once `deputyctl init` completes, the wizard listens on
`http://<vm-ip>:8088`. Find the IP via:

```bash
# SCALE shell:
midclt call vm.get_vmemory_in_use
midclt call vm.query | jq '.[] | select(.name=="deputyos") | .devices[] | select(.dtype=="NIC")'
```

Or just check your router's DHCP leases for a host called
`deputyos`.

## Updates

A/B updates aren't wired for the truenas target. To upgrade:

1. Snapshot the zvol: `zfs snapshot tank/deputyos@pre-update`.
2. Stop the VM (Virtualization page).
3. Convert the new qcow2 over the zvol:

    ```bash
    qemu-img convert -O raw /mnt/tank/iso/qemu-x86_64-openclaw-NEWVERSION.qcow2 /dev/zvol/tank/deputyos
    ```

4. Start the VM. Roll back via
   `zfs rollback tank/deputyos@pre-update` if anything's wrong.

## Using `deputyos.json` programmatically

The JSON file in this directory is shaped to match SCALE's
`vm.create` and `vm.device.create` middleware calls. You can drive
the install from the SCALE shell:

```bash
# Pseudocode — TrueNAS middleware doesn't accept this file directly.
# Read the JSON, split it into a vm.create payload + N vm.device.create
# payloads, then call midclt for each. See:
# https://www.truenas.com/docs/scale/api/
```

A future iteration may ship a `truenas-install.sh` helper that does
this automatically; for now the click-through UI path is the
recommended one.

## Limitations

- We don't actively test TrueNAS Scale builds.
- ZFS-on-zvol-of-qcow2 has double-COW overhead; the raw conversion
  step above avoids that.
- PCIe passthrough (Coral, Hailo, NVIDIA) is your problem to
  configure; deputyOS doesn't bundle the runtimes for the truenas
  target.
