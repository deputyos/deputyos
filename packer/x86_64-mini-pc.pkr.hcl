# x86_64 mini-PC builder (Beelink / MeLE / NUC class — real hardware).
#
# Approach: option (b) from the lane-plan. We reuse the HashiCorp QEMU
# builder (same as packer/qemu-x86_64.pkr.hcl) so the bake is identical
# in execution to the working CI smoke target — the only differences are:
#
#   - deputyos_hw=x86_64-mini-pc (variant role applies real-HW tunings:
#     cpufreq schedutil, intel/amd microcode, mainline amd64 kernel,
#     wifi power-save oneshot — see roles/deputyos/tasks/variant-x86_64-mini-pc.yml)
#   - The post-processor converts the qcow2 to a raw disk image and
#     xz-compresses it, producing an .img.xz that `dd` can write to a
#     USB stick or the box's M.2 SSD.
#
# Why option (b) over a packer-builder-arm-equivalent for amd64:
#   - The QEMU builder is already proven (CI smoke runs against
#     qemu-x86_64). We get the exact same provisioning path, just with
#     a different deputyos_hw extra-var and a different post-processor.
#   - No second plugin to install on contributor hosts.
#   - qemu-img + xz are already required by `make doctor`.
#
# Output: build/x86_64-mini-pc-<profile>.img.xz

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
  description = "Host-side staging dir scripts/build.sh populates."
}

# Reused from packer/qemu-x86_64.pkr.hcl — same Debian 12 amd64 cloud
# image snapshot, so userland behaviour is identical to the smoke
# target (with the variant role applying mini-PC tunings on top).
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

# UEFI firmware. Same default path as packer/qemu-x86_64.pkr.hcl —
# Debian/Ubuntu's `ovmf` package installs OVMF.fd at the path below.
variable "ovmf_firmware" {
  type        = string
  default     = "/usr/share/ovmf/OVMF.fd"
  description = "Path to OVMF UEFI firmware on the Packer host."
}

# Larger disk than the qemu smoke target — mini-PCs typically ship with
# 256GB+ M.2; 16GB gives the bake room to install OpenVINO + microcode
# + clamd signature DB without surprise resize-on-first-boot.
variable "disk_size" {
  type        = string
  default     = "16G"
  description = "Bake-time disk size. The first-boot wizard grows root to fill the actual SSD."
}

source "qemu" "x86_64-mini-pc" {
  iso_url      = var.base_image_url
  iso_checksum = var.base_image_checksum

  disk_image       = true
  disk_size        = var.disk_size
  format           = "qcow2"
  output_directory = "build/packer-x86_64-mini-pc"
  vm_name          = "x86_64-mini-pc-${var.profile}.qcow2"

  qemu_binary    = "qemu-system-x86_64"
  machine_type   = "q35"
  cpus           = 2
  memory         = 4096
  accelerator    = "tcg"
  use_default_display = false
  headless       = true

  firmware = var.ovmf_firmware

  cd_files = ["${var.deputyos_staging_dir}/cloud-init/user-data",
              "${var.deputyos_staging_dir}/cloud-init/meta-data"]
  cd_label = "cidata"

  ssh_username     = var.ssh_username
  ssh_private_key_file = "${var.deputyos_staging_dir}/ssh-key"
  ssh_timeout      = "10m"
  ssh_port         = 22

  qemuargs = [
    ["-machine", "q35,accel=kvm:tcg"],
    ["-cpu", "max"],
    ["-device", "virtio-net-pci,netdev=user.0"],
    ["-serial", "stdio"],
  ]

  shutdown_command = "sudo shutdown -P now"
}

build {
  name    = "x86_64-mini-pc"
  sources = ["source.qemu.x86_64-mini-pc"]

  provisioner "ansible" {
    playbook_file = "packer/playbook.yml"
    user          = var.ssh_username
    use_proxy     = false
    extra_arguments = [
      "--extra-vars",
      join(" ", [
        "deputyos_hw=x86_64-mini-pc",
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

  # Convert qcow2 -> raw -> xz, dd-able to a USB stick or M.2 SSD.
  # qemu-img and xz are required by `make doctor`; both ship in
  # qemu-utils + xz-utils on Debian/Ubuntu and brew formulae on macOS.
  post-processor "shell-local" {
    inline = [
      "set -euo pipefail",
      "mkdir -p build",
      "qemu-img convert -O raw build/packer-x86_64-mini-pc/x86_64-mini-pc-${var.profile}.qcow2 build/x86_64-mini-pc-${var.profile}.img",
      "xz -T0 -f build/x86_64-mini-pc-${var.profile}.img",
    ]
  }
}
