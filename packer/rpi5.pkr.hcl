# Raspberry Pi 5 builder.
#
# Uses mkaczanowski/packer-builder-arm because the Pi OS image is a raw
# disk image with two partitions (FAT32 boot + ext4 root); the plugin
# mount-and-chroots into it via binfmt_misc + qemu-aarch64-static. This
# build will not run in CI (needs binfmt_misc registration); it must
# run on a real Linux host.
#
# Output: build/rpi5-<profile>.img.xz

packer {
  required_version = ">= 1.10.0"
  # Note: mkaczanowski/packer-builder-arm is NOT published to the
  # HashiCorp Packer registry, so we do not declare it under
  # required_plugins{} (Packer 1.10+ rejects sources containing the
  # legacy "packer-builder-" prefix in that block). Install the plugin
  # manually before running this template:
  #
  #   git clone https://github.com/mkaczanowski/packer-builder-arm
  #   cd packer-builder-arm && make build
  #   mkdir -p ~/.config/packer/plugins/github.com/mkaczanowski/arm
  #   cp packer-builder-arm \
  #     ~/.config/packer/plugins/github.com/mkaczanowski/arm/packer-plugin-arm
  #
  # scripts/build.sh checks for it before invoking `packer build TARGET=rpi5`.
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

# Pinned Raspberry Pi OS Lite arm64 (Trixie), release 2026-04-21.
# SHA256 from the upstream raspberrypi.com release page.
variable "base_image_url" {
  type        = string
  default     = "https://downloads.raspberrypi.com/raspios_lite_arm64/images/raspios_lite_arm64-2026-04-21/2026-04-21-raspios-trixie-arm64-lite.img.xz"
  description = "Pinned Raspberry Pi OS Lite arm64 image."
}

variable "base_image_checksum" {
  type        = string
  default     = "sha256:4cd31df026fd82243805a326dc0cafd7383f7e3d30c9413e7044d507aae281e2"
  description = "SHA256 from raspberrypi.com/software/operating-systems."
}

source "arm-image" "rpi5" {
  iso_url           = var.base_image_url
  iso_checksum      = var.base_image_checksum
  output_filename   = "build/rpi5-${var.profile}.img"
  target_image_size = 16 * 1024 * 1024 * 1024
  image_type        = "raw"
  image_arch        = "arm64"
  qemu_binary       = "qemu-aarch64-static"
}

build {
  name    = "rpi5"
  sources = ["source.arm-image.rpi5"]

  provisioner "ansible-local" {
    playbook_file = "packer/playbook.yml"
    role_paths    = ["roles/deputyos"]
    extra_arguments = [
      "--extra-vars",
      join(" ", [
        "deputyos_hw=rpi5",
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
    output            = "build/rpi5-${var.profile}.img.xz"
    compression_level = 6
  }
}
