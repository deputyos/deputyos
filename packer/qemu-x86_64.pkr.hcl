# QEMU x86_64 builder — the second CI smoke target.
#
# Mirrors packer/qemu-aarch64.pkr.hcl with x86_64 specifics:
#   - q35 machine + OVMF (UEFI) firmware
#   - virtio-net-pci (vs virtio-net-device on aarch64 'virt')
#   - cpu=max, kvm-or-tcg accelerator chain
#
# This proves the "shared role + variant gates" architecture
# extends to a second target without changing packer/playbook.yml or
# the shared role's main path. The Debian cloud image is the same
# dated snapshot the aarch64 build pins to (20250316-2053).
#
# Output: build/qemu-x86_64-<profile>.qcow2

packer {
  required_version = ">= 1.10.0"
  required_plugins {
    qemu = {
      source  = "github.com/hashicorp/qemu"
      version = "~> 1.1"
    }
    ansible = {
      source  = "github.com/hashicorp/ansible"
      version = "~> 1.1"
    }
  }
}

variable "profile" {
  type        = string
  default     = "openclaw"
  description = "Profile id to bake. Must match a file in profiles/."
}

variable "channel" {
  type        = string
  default     = "dev"
  description = "Release channel for this build (dev | beta | stable)."
}

variable "tier" {
  type        = string
  default     = "standard"
  description = "Bundle tier (lean | standard | rich)."
}
variable "airgap" {
  type        = string
  default     = "0"
  description = "Air-gapped build (\"1\" or \"true\"); bakes LFM2 + locks egress. See docs/11-roadmap.md § M4.5."
}


variable "deputyos_staging_dir" {
  type        = string
  default     = "build/staging"
  description = "Host-side staging dir scripts/build.sh populates with deputyctl, profile manifest, and limits.json."
}

# Pinned Debian 12 (bookworm) generic amd64 cloud image, snapshot
# 20250316-2053 — same dated snapshot as the aarch64 builder. SHA512
# from cloud.debian.org/images/cloud/bookworm/20250316-2053/SHA512SUMS
# (verified on M2-phase-3 land).
variable "base_image_url" {
  type        = string
  default     = "https://cloud.debian.org/images/cloud/bookworm/20250316-2053/debian-12-generic-amd64-20250316-2053.qcow2"
  description = "Pinned Debian 12 generic amd64 cloud image."
}

variable "base_image_checksum" {
  type        = string
  default     = "sha512:afcd77455c6d10a6650e8affbcb4d8eb4e81bd17f10b1d1dd32d2763e07198e168a3ec8f811770d50775a83e84ee592a889a3206adf0960fb63f3d23d1df98af"
  description = "SHA512 from cloud.debian.org/images/cloud/bookworm/20250316-2053/SHA512SUMS."
}

variable "ssh_username" {
  type        = string
  default     = "deputyos-build"
  description = "Throwaway user injected via cloud-init for the Packer SSH session."
}

# UEFI firmware. Debian/Ubuntu's `ovmf` package installs OVMF.fd at
# /usr/share/ovmf/OVMF.fd; newer packages also ship 4M-split builds at
# /usr/share/OVMF/OVMF_CODE_4M.fd. Override per-host if needed.
variable "ovmf_firmware" {
  type        = string
  default     = "/usr/share/ovmf/OVMF.fd"
  description = "Path to OVMF UEFI firmware on the Packer host."
}

source "qemu" "qemu-x86_64" {
  iso_url      = var.base_image_url
  iso_checksum = var.base_image_checksum

  # Boot the qcow2 directly (cloud image is already UEFI-bootable).
  disk_image       = true
  disk_size        = "8G"
  format           = "qcow2"
  output_directory = "build/packer-qemu-x86_64"
  vm_name          = "qemu-x86_64-${var.profile}.qcow2"

  qemu_binary    = "qemu-system-x86_64"
  machine_type   = "q35"
  cpus           = 2
  memory         = 4096
  # tcg keeps CI portable; KVM is opportunistically picked up via
  # qemuargs `-machine accel=kvm:tcg` below when the host supports it.
  accelerator    = "tcg"
  use_default_display = false
  headless       = true

  firmware = var.ovmf_firmware

  # NoCloud seed ISO with cloud-init user-data — same staging layout as
  # qemu-aarch64. scripts/build.sh writes user-data + meta-data here.
  cd_files = ["${var.deputyos_staging_dir}/cloud-init/user-data",
              "${var.deputyos_staging_dir}/cloud-init/meta-data"]
  cd_label = "cidata"

  ssh_username     = var.ssh_username
  ssh_private_key_file = "${var.deputyos_staging_dir}/ssh-key"
  ssh_timeout      = "10m"
  ssh_port         = 22

  # Route serial to packer's stdio so first-boot failures are debuggable.
  # `-cpu max` exposes the widest x86 feature set tcg can emulate; on a
  # KVM-capable host the kvm:tcg fallback in -machine picks accel.
  qemuargs = [
    ["-machine", "q35,accel=kvm:tcg"],
    ["-cpu", "max"],
    ["-device", "virtio-net-pci,netdev=user.0"],
    ["-serial", "stdio"],
  ]

  shutdown_command = "sudo shutdown -P now"
}

build {
  name    = "qemu-x86_64"
  sources = ["source.qemu.qemu-x86_64"]

  provisioner "ansible" {
    playbook_file = "packer/playbook.yml"
    user          = var.ssh_username
    use_proxy     = false
    extra_arguments = [
      "--extra-vars",
      join(" ", [
        "deputyos_hw=qemu-x86_64",
        "deputyos_profile=${var.profile}",
        "deputyos_channel=${var.channel}",
        "deputyos_tier=${var.tier}",
        "deputyos_airgap=${var.airgap}",
        "deputyos_staging_dir=${var.deputyos_staging_dir}",
        "deputyctl_path=${var.deputyos_staging_dir}/deputyctl",
        "profile_staging_path=${var.deputyos_staging_dir}/profiles",
        "limits_staging_path=${var.deputyos_staging_dir}/limits.json",
      ]),
    ]
  }

  post-processor "shell-local" {
    inline = [
      "mkdir -p build",
      "cp build/packer-qemu-x86_64/qemu-x86_64-${var.profile}.qcow2 build/qemu-x86_64-${var.profile}.qcow2",
    ]
  }
}
