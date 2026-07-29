# Oracle Cloud — Always-Free Ampere A1 (aarch64) builder.
#
# We do NOT use an Oracle-specific Packer plugin. Oracle's `oci-classic`
# builder builds inside their cloud (requires API keys, paid tenancies,
# etc.). The Always-Free path is much simpler:
#
#   1. Build a Debian 12 arm64 qcow2 locally with the stock QEMU plugin
#      (mirrors packer/qemu-aarch64.pkr.hcl byte-for-byte except the
#      ansible extra_var pivots deputyos_hw to oracle-arm-free).
#   2. User uploads the qcow2 to OCI Object Storage.
#   3. User runs `oci compute image import` to create a custom image.
#   4. User launches an Always-Free A1 instance from the custom image.
#
# Steps 2-4 require the OCI CLI + tenancy creds; we do NOT gate the
# build on them. The shell-local post-processor prints copy-pastable
# commands so the user can complete the import without leaving the
# terminal.
#
# Output: build/oracle-arm-free-<profile>.qcow2
#
# Reference:
#   docs/03-image-builds.md   §"build matrix" — oracle-arm-free row.
#   docs/14-limitations.md    §"oracle-arm-free" — reclaim risk + sizing.

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

# Same Debian 12 (bookworm) generic arm64 cloud image used by
# packer/qemu-aarch64.pkr.hcl. Oracle accepts qcow2 directly via
# `oci compute image import`.
variable "base_image_url" {
  type        = string
  default     = "https://cloud.debian.org/images/cloud/bookworm/20250316-2053/debian-12-generic-arm64-20250316-2053.qcow2"
  description = "Pinned Debian 12 generic arm64 cloud image."
}

variable "base_image_checksum" {
  type        = string
  default     = "sha512:ddc9ffb68053ee02443eba9327381d7dbc2384bcf716565ec894d17d38930636b14b0c5aad867b519f8b018e2c29f9f59621c3e04cdbc51c6076aef5e85c4065"
  description = "SHA512 from cloud.debian.org/images/cloud/bookworm/20250316-2053/SHA512SUMS."
}

variable "ssh_username" {
  type        = string
  default     = "deputyos-build"
  description = "Throwaway user injected via cloud-init for the Packer SSH session."
}

# Oracle Object Storage upload hints. These are not applied during the
# build; only printed in the post-processor so the user can paste them.
variable "oci_bucket_name" {
  type        = string
  default     = "deputyos-images"
  description = "Hint: OCI Object Storage bucket name for the qcow2 upload step (printed only)."
}

variable "oci_compartment_ocid" {
  type        = string
  default     = "<YOUR_COMPARTMENT_OCID>"
  description = "Hint: target compartment OCID for `oci compute image import` (printed only)."
}

source "qemu" "oracle-arm-free" {
  iso_url      = var.base_image_url
  iso_checksum = var.base_image_checksum

  disk_image       = true
  disk_size        = "47G"
  format           = "qcow2"
  output_directory = "build/packer-oracle-arm-free"
  vm_name          = "oracle-arm-free-${var.profile}.qcow2"

  qemu_binary    = "qemu-system-aarch64"
  machine_type   = "virt"
  cpus           = 2
  memory         = 4096
  accelerator    = "tcg"
  use_default_display = false
  headless       = true

  firmware = "/usr/share/AAVMF/AAVMF_CODE.fd"

  cd_files = ["${var.deputyos_staging_dir}/cloud-init/user-data",
              "${var.deputyos_staging_dir}/cloud-init/meta-data"]
  cd_label = "cidata"

  ssh_username     = var.ssh_username
  ssh_private_key_file = "${var.deputyos_staging_dir}/ssh-key"
  ssh_timeout      = "10m"
  ssh_port         = 22

  qemuargs = [
    ["-cpu", "cortex-a72"],
    ["-serial", "stdio"],
  ]

  shutdown_command = "sudo shutdown -P now"
}

build {
  name    = "oracle-arm-free"
  sources = ["source.qemu.oracle-arm-free"]

  provisioner "ansible" {
    playbook_file = "packer/playbook.yml"
    user          = var.ssh_username
    use_proxy     = false
    extra_arguments = [
      "--extra-vars",
      join(" ", [
        "deputyos_hw=oracle-arm-free",
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
      "cp build/packer-oracle-arm-free/oracle-arm-free-${var.profile}.qcow2 build/oracle-arm-free-${var.profile}.qcow2",
      "echo",
      "echo '==> oracle-arm-free build complete: build/oracle-arm-free-${var.profile}.qcow2'",
      "echo",
      "echo 'Next steps (require OCI CLI + tenancy credentials):'",
      "echo",
      "echo '  1. Upload to Object Storage:'",
      "echo '     oci os object put \\'",
      "echo '       --bucket-name ${var.oci_bucket_name} \\'",
      "echo '       --file build/oracle-arm-free-${var.profile}.qcow2 \\'",
      "echo '       --name deputyos-${var.profile}-${var.channel}.qcow2'",
      "echo",
      "echo '  2. Import as a custom image:'",
      "echo '     oci compute image import from-object \\'",
      "echo '       --compartment-id ${var.oci_compartment_ocid} \\'",
      "echo '       --bucket-name ${var.oci_bucket_name} \\'",
      "echo '       --name deputyos-${var.profile}-${var.channel} \\'",
      "echo '       --namespace-name $(oci os ns get --query data --raw-output) \\'",
      "echo '       --object-name deputyos-${var.profile}-${var.channel}.qcow2 \\'",
      "echo '       --launch-mode PARAVIRTUALIZED \\'",
      "echo '       --source-image-type QCOW2'",
      "echo",
      "echo '  3. Launch an Always-Free A1 instance from the imported image.'",
      "echo '     See docs/14-limitations.md §oracle-arm-free for reclaim caveats.'",
    ]
  }
}
