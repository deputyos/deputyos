# WSL2 distro tarball builder.
#
# Why a `null` builder + `shell-local` provisioner instead of qemu/arm:
# WSL2 distros are rootfs tarballs, not bootable images — there's no
# kernel, bootloader, or filesystem layout to manage. The user runs
# `wsl --import` on Windows; WSL provides the kernel and the cgroup
# hierarchy.
#
# Approach: scripts/wsl2-build.sh (invoked via shell-local) downloads a
# pinned Debian 12 nocloud rootfs tarball, extracts it, runs the same
# Ansible role inside it via systemd-nspawn (preferred) or proot
# (fallback for unprivileged hosts), then re-tars the result as a
# .tar.gz. This file's job is just to wire that script into the same
# `packer build` command-shape Lane B uses for every other target so
# `make build TARGET=wsl2` follows the standard dispatch.
#
# Output: build/wsl2-<profile>.tar.gz
#
# Pinned base rootfs (matches the qemu-x86_64 base date for byte-level
# determinism across targets):
#   debian-12-nocloud-amd64-20250316-2053.tar.xz
#   from https://cloud.debian.org/images/cloud/bookworm/20250316-2053/
#   SHA512 from the matching SHA512SUMS file alongside.

packer {
  required_version = ">= 1.10.0"
  required_plugins {
    null = {
      source  = "github.com/hashicorp/null"
      version = "~> 1.0"
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

# Pinned Debian 12 (bookworm) nocloud amd64 rootfs tarball, snapshot
# 20250316-2053. This is the same snapshot used by Lane B/E-M2's
# qemu-x86_64 base, intentionally — keeps the userspace versions across
# wsl2 and qemu-x86_64 in lock-step.
variable "base_rootfs_url" {
  type        = string
  default     = "https://cloud.debian.org/images/cloud/bookworm/20250316-2053/debian-12-nocloud-amd64-20250316-2053.tar.xz"
  description = "Pinned Debian 12 nocloud amd64 rootfs tarball."
}

variable "base_rootfs_checksum" {
  type        = string
  # Pulled from https://cloud.debian.org/images/cloud/bookworm/20250316-2053/SHA512SUMS
  # The bake script verifies this before extraction.
  default     = "sha512:1f69ef5d87ad01f415826739565d27f983fc6d6297c9cd6585ebc4815132d17ca677ff7ccce29d36fe54fd7702bca20ccf66c9eb38b658d3f52759c4dadfb7e9"
  description = "SHA512 from cloud.debian.org/images/cloud/bookworm/20250316-2053/SHA512SUMS for debian-12-nocloud-amd64-20250316-2053.tar.xz."
}

source "null" "wsl2" {
  communicator = "none"
}

build {
  name    = "wsl2"
  sources = ["source.null.wsl2"]

  provisioner "shell-local" {
    # The heavy lifting lives in scripts/wsl2-build.sh. Keeping the
    # logic in a shell script (vs HCL) means the same script is
    # invokable directly from `make build TARGET=wsl2` even when
    # `packer` itself isn't installed (degraded mode).
    inline = [
      "bash scripts/wsl2-build.sh",
    ]
    environment_vars = [
      "PROFILE=${var.profile}",
      "CHANNEL=${var.channel}",
      "TIER=${var.tier}",
      "DEPUTYOS_STAGING_DIR=${var.deputyos_staging_dir}",
      "BASE_ROOTFS_URL=${var.base_rootfs_url}",
      "BASE_ROOTFS_CHECKSUM=${var.base_rootfs_checksum}",
    ]
  }
}
