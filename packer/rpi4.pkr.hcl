# Raspberry Pi 4 builder.
#
# Same toolchain as packer/rpi5.pkr.hcl: mkaczanowski/packer-builder-arm
# mounts the raw Pi OS image (FAT32 boot + ext4 root) and chroots into
# it via binfmt_misc + qemu-aarch64-static. Will not run in CI; needs a
# real Linux host with binfmt_misc registered for aarch64.
#
# Base-image choice: the official `raspios_lite_arm64` Bookworm/Trixie
# release supports both the Pi 4 (bcm2711) and Pi 5 (bcm2712) families
# from the same image — the kernel detects the SoC at boot and loads
# the right firmware/DT. We therefore reuse the exact URL + checksum
# pinned in packer/rpi5.pkr.hcl. The variant role
# (roles/deputyos/tasks/variant-rpi4.yml) installs `linux-image-rpi-2711`
# explicitly so the Pi-4-appropriate kernel package wins on bootloader
# config and we don't carry the 2712-only kernel as the default.
#
# Output: build/rpi4-<profile>.img.xz

packer {
  required_version = ">= 1.10.0"
  # mkaczanowski/packer-builder-arm is NOT on the HashiCorp Packer
  # registry; install it manually before running this template (see
  # packer/rpi5.pkr.hcl header for the install steps). scripts/build.sh
  # checks for it before invoking this template.
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

# Reused from packer/rpi5.pkr.hcl. The same raspios_lite_arm64 image
# boots on both Pi 4 (bcm2711) and Pi 5 (bcm2712) — kernel detects the
# SoC family. If the Pi 4 ever needs its own pin (e.g. a Pi-4-only
# regression in a future release), split this var.
variable "base_image_url" {
  type        = string
  default     = "https://downloads.raspberrypi.com/raspios_lite_arm64/images/raspios_lite_arm64-2026-04-21/2026-04-21-raspios-trixie-arm64-lite.img.xz"
  description = "Pinned Raspberry Pi OS Lite arm64 image (Pi 4 + Pi 5 compatible)."
}

variable "base_image_checksum" {
  type        = string
  default     = "sha256:4cd31df026fd82243805a326dc0cafd7383f7e3d30c9413e7044d507aae281e2"
  description = "SHA256 from raspberrypi.com/software/operating-systems."
}

source "arm-image" "rpi4" {
  iso_url           = var.base_image_url
  iso_checksum      = var.base_image_checksum
  output_filename   = "build/rpi4-${var.profile}.img"
  # Pi 4 4GB-class default; the partition can be grown by the user on
  # first boot if a larger SD/SSD is installed.
  target_image_size = 16 * 1024 * 1024 * 1024
  image_type        = "raw"
  image_arch        = "arm64"
  qemu_binary       = "qemu-aarch64-static"
}

build {
  name    = "rpi4"
  sources = ["source.arm-image.rpi4"]

  provisioner "ansible-local" {
    playbook_file = "packer/playbook.yml"
    role_paths    = ["roles/deputyos"]
    extra_arguments = [
      "--extra-vars",
      join(" ", [
        "deputyos_hw=rpi4",
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
    output            = "build/rpi4-${var.profile}.img.xz"
    compression_level = 6
  }
}
