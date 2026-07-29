# DigitalOcean builder — full Packer build → DO snapshot.
#
# Source: stock Ubuntu 24.04 LTS droplet. The HashiCorp `digitalocean`
# Packer plugin spins up a temporary droplet from a public DO image,
# runs the shared Ansible role over SSH, then snapshots the droplet.
# Output: a DO snapshot named deputyos-<profile>-<channel>-<timestamp>
# that the picker page (M2+) and DO 1-Click submission (M5) consume.
#
# This template is local-buildable: `packer init` and
# `packer validate -syntax-only` succeed without any DO credentials.
# A real `packer build` requires DIGITALOCEAN_TOKEN in the environment;
# scripts/build.sh fails loudly with a fix hint when it's missing.
#
# Reference:
#   docs/03-image-builds.md   §"build matrix" — digitalocean row.
#   docs/14-limitations.md    §"digitalocean".

packer {
  required_version = ">= 1.10.0"
  required_plugins {
    digitalocean = {
      source  = "github.com/digitalocean/digitalocean"
      version = "~> 1.4"
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

# DigitalOcean API token. Read from the DIGITALOCEAN_TOKEN env var by
# default — never bake into the template. Required at `packer build`
# time; `packer validate -syntax-only` does not exercise the API.
variable "do_token" {
  type        = string
  sensitive   = true
  default     = ""
  description = "DigitalOcean API token. scripts/build.sh sets this from $DIGITALOCEAN_TOKEN."
}

variable "do_region" {
  type        = string
  default     = "nyc3"
  description = "DigitalOcean region for the build droplet. The resulting snapshot is created in this region; replicate via doctl post-build."
}

variable "do_size" {
  type        = string
  default     = "s-2vcpu-4gb"
  description = "Build droplet size. 4 GB is the standard deputyOS floor; smaller sizes will OOM during the role's clamav signature pull."
}

# Stock Ubuntu 24.04 base. DO publishes a slug per LTS release; this is
# the long-term-supported amd64 image. Override per-build for arm64
# (atomic-base) once DO ships an arm64 LTS slug.
variable "do_base_image" {
  type        = string
  default     = "ubuntu-24-04-x64"
  description = "DO base image slug. ubuntu-24-04-x64 is the current LTS amd64 image."
}

source "digitalocean" "digitalocean" {
  api_token     = var.do_token
  region        = var.do_region
  size          = var.do_size
  image         = var.do_base_image
  ssh_username  = "root"

  # Snapshot name is what shows up in the DO control panel and in
  # `doctl compute snapshot list`. Picker (M2+) reads this name pattern
  # to surface "deputyOS for DigitalOcean" in the per-target picker row.
  snapshot_name = "deputyos-${var.profile}-${var.channel}-{{timestamp}}"

  # Tag the build droplet so a stuck/zombie droplet is easy to grep for
  # in the DO control panel. Tags don't propagate to the snapshot.
  droplet_name  = "deputyos-build-${var.profile}-{{timestamp}}"
  tags          = ["deputyos", "deputyos-build", "deputyos-${var.channel}"]
}

build {
  name    = "digitalocean"
  sources = ["source.digitalocean.digitalocean"]

  provisioner "ansible" {
    playbook_file = "packer/playbook.yml"
    user          = "root"
    use_proxy     = false
    extra_arguments = [
      "--extra-vars",
      join(" ", [
        "deputyos_hw=digitalocean",
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
      "echo 'digitalocean snapshot created. List via: doctl compute snapshot list --resource droplet'",
      "echo 'For 1-Click submission see docs/03-image-builds.md M5 milestone.'",
    ]
  }
}
