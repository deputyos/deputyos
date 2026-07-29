# Generic arm64 SBC builder (Radxa, Orange Pi, Khadas, Le Potato, ...).
#
# Same builder as rpi4/rpi5 (mkaczanowski/packer-builder-arm), but the
# base is the **stock Debian 12 arm64 cloud image** rather than a
# pi-gen Raspberry-Pi-OS image. We reuse the same dated snapshot
# packer/qemu-aarch64.pkr.hcl pins, so the resulting role bake is
# kernel-API-compatible with the QEMU smoke target.
#
# Caveat: cloud images are qcow2; packer-builder-arm wants a raw disk
# image. We rely on the `arm-image` source's `image_type=raw` plus
# qemu-img conversion done implicitly by the plugin when fed a qcow2
# URL. If your version of packer-builder-arm rejects qcow2 input,
# pre-convert the base image and host it on an internal mirror, then
# override base_image_url at `packer build -var ...` time.
#
# This is a best-effort target. Per-board quirks (device-tree overlays,
# audio routing, video output, thermal envelope) are NOT attempted at
# bake time; the first-boot wizard collects the user's overlay name
# and applies it. See docs/14-limitations.md §"arm64-generic".
#
# Output: build/arm64-generic-<profile>.img.xz

packer {
  required_version = ">= 1.10.0"
  # mkaczanowski/packer-builder-arm is NOT on the HashiCorp Packer
  # registry; see packer/rpi5.pkr.hcl header for install steps.
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

# Reused from packer/qemu-aarch64.pkr.hcl — same dated snapshot, so
# qemu-aarch64 smoke results are a meaningful proxy for arm64-generic
# kernel/userland behaviour (per-board hardware quirks excepted).
variable "base_image_url" {
  type        = string
  default     = "https://cloud.debian.org/images/cloud/bookworm/20250316-2053/debian-12-generic-arm64-20250316-2053.qcow2"
  description = "Pinned Debian 12 generic arm64 cloud image (qcow2 — same as qemu-aarch64)."
}

variable "base_image_checksum" {
  type        = string
  default     = "sha512:ddc9ffb68053ee02443eba9327381d7dbc2384bcf716565ec894d17d38930636b14b0c5aad867b519f8b018e2c29f9f59621c3e04cdbc51c6076aef5e85c4065"
  description = "SHA512 from cloud.debian.org/images/cloud/bookworm/20250316-2053/SHA512SUMS."
}

source "arm-image" "arm64-generic" {
  iso_url           = var.base_image_url
  iso_checksum      = var.base_image_checksum
  output_filename   = "build/arm64-generic-${var.profile}.img"
  target_image_size = 16 * 1024 * 1024 * 1024
  image_type        = "raw"
  image_arch        = "arm64"
  qemu_binary       = "qemu-aarch64-static"
}

build {
  name    = "arm64-generic"
  sources = ["source.arm-image.arm64-generic"]

  provisioner "ansible-local" {
    playbook_file = "packer/playbook.yml"
    role_paths    = ["roles/deputyos"]
    extra_arguments = [
      "--extra-vars",
      join(" ", [
        "deputyos_hw=arm64-generic",
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

  post-processor "compress" {
    output            = "build/arm64-generic-${var.profile}.img.xz"
    compression_level = 6
  }
}
