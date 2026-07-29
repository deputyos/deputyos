# deputyOS — top-level Makefile.
#
# This is the API. CI calls `make ci`. Contributors call `make doctor`,
# then `make build`, `make try`, `make smoke`. macOS, WSL2, Linux all use
# the same surface — host-OS detection lives in scripts/, never here.
#
# Hard rule: every check, build, smoke, and signing step must work
# locally. CI is a thin wrapper around `make ci`.

SHELL := /usr/bin/env bash
.SHELLFLAGS := -eu -o pipefail -c

# Default goal — typing `make` shows the help.
.DEFAULT_GOAL := help

TARGET  ?= qemu-aarch64
PROFILE ?= openclaw
CHANNEL ?= dev
TIER    ?= standard
# AIRGAP=1 builds an air-gapped image: file:// apt mirror, ufw/nftables egress
# deny, baked LFM2 GGUF model per tier (see docs/11-roadmap.md § M4.5 and
# documentation/docs/concepts/airgap.md).
AIRGAP  ?= 0

# SCAFFOLD_PHASE=1 skips `make build` and `make smoke` in `make ci`.
# Lane B has now pinned base SHAs and the smoke harness is real, so the
# default is unset (= full). The env var stays as an escape hatch so
# contributors and CI can opt out while Lane A and Lane F finish.
# TODO: drop SCAFFOLD_PHASE once Lane A/F also land — see CI run.
SCAFFOLD_PHASE ?= 0

# Smoke gate: m1 is the new bar for this lane. Override per-invocation.
SMOKE_LEVEL ?= m1

CARGO  ?= cargo
PACKER ?= packer

.PHONY: help doctor fmt lint test build try smoke matrix stage-release-artifacts sign-dev sign-release manifest publish-local publish-cdn publish-r2 verify verify-cdn ci clean wizard pwa desktop-launcher desktop-launcher-release console console-bundle cdn-up cdn-down desktop-local-build desktop-local track track-propose track-apply sbom sbom-all slsa-attest slsa-all slsa-verify docs docs-build

## ===== Help =====

help: ## Show this help.
	@echo "deputyOS — Makefile targets"
	@echo
	@grep -E '^[a-zA-Z_-]+:.*## ' $(MAKEFILE_LIST) | sort | \
	  awk -F':.*## ' '{printf "  %-20s %s\n", $$1, $$2}'
	@echo
	@echo "Common variables:"
	@echo "  TARGET=<hw>         (default: qemu-aarch64)"
	@echo "    QEMU smoke:       qemu-aarch64 | qemu-x86_64"
	@echo "    Real hardware:    rpi5 | rpi4 | arm64-generic | x86_64-mini-pc"
	@echo "    Cloud snapshot:   digitalocean | oracle-arm-free      (Packer build)"
	@echo "    Cloud-init recipe:hetzner-cloud | vultr | linode      (paste YAML, no build)"
	@echo "    OCI artefact:     fly-machines                        (buildah/docker build)"
	@echo "    WSL distro:       wsl2                                (tarball; Linux/WSL2 host bake)"
	@echo "    macOS demo:       macos-qemu                          (qcow2-wrap of qemu-aarch64)"
	@echo "    Community VM:     proxmox | unraid | truenas          (template-only; wraps qemu-x86_64/aarch64 qcow2)"
	@echo "  PROFILE=<id>        (default: openclaw)"
	@echo "  CHANNEL=<channel>   (default: dev)"
	@echo "  TIER=<tier>         (default: standard)"
	@echo "  AIRGAP=1            build an air-gapped image (no network at runtime; LFM2 baked)"
	@echo "  SMOKE_LEVEL=<lvl>   smoke assertion level (default: m1; scaffold|m1|full)"
	@echo "  SCAFFOLD_PHASE=1    skip build+smoke in 'make ci' (lane-coordination escape hatch)"
	@echo
	@echo "  matrix              build+smoke both qemu-aarch64 and qemu-x86_64 in sequence"
	@echo
	@echo "Desktop launcher (M2.5):"
	@echo "  make desktop-launcher [DESKTOP_TARGET=<rust-target-triple>]"
	@echo "    e.g. DESKTOP_TARGET=x86_64-unknown-linux-gnu  (Linux x86_64; default: host)"
	@echo "    e.g. DESKTOP_TARGET=x86_64-pc-windows-gnu     (cross via 'cargo install cross')"
	@echo
	@echo "Release loop (Lane D — local-first; not in 'make ci'):"
	@echo "  make manifest DEPUTYOS_RELEASE_VERSION=<Y.M.D> [CHANNEL=dev]"
	@echo "  make publish-local       # mirror dist/ for file:// CDN"
	@echo "  make verify VERSION=<v>  # rebuild and SHA-compare; DEPUTYOS_VERIFY_STRICT=1 makes mismatch fatal"
	@echo
	@echo "Supply-chain (M7 Lane B — release-time only; not in 'make ci'):"
	@echo "  make sbom ARTEFACT=<path>          # CycloneDX SBOM next to the artefact"
	@echo "  make sbom-all                      # SBOM every signable artefact in build/"
	@echo "  make slsa-attest ARTEFACT=<path>   # SLSA v1 provenance + sign"
	@echo "  make slsa-all                      # SLSA every signable artefact in build/"
	@echo
	@echo "Cloud-build credentials (only required for real builds; lint/validate work without):"
	@echo "  DIGITALOCEAN_TOKEN  required for TARGET=digitalocean"
	@echo "  OCI CLI creds       required to upload TARGET=oracle-arm-free qcow2 (post-build step)"
	@echo "  flyctl auth login   required for TARGET=fly-machines deploy (build itself is local)"

## ===== Pre-flight =====

doctor: ## Check host has the right tools; one-line fix per missing item.
	@bash scripts/doctor.sh

## ===== Linting =====

fmt: ## Format Rust + YAML in place.
	$(CARGO) fmt --all
	@if command -v yamllint >/dev/null 2>&1; then \
	  yamllint roles/ .github/ || true; \
	else \
	  echo "warn: yamllint not installed; skipping"; \
	fi

lint: ## Run every linter (cargo fmt --check, clippy, profile validate, ansible-lint, yamllint, shellcheck, packer validate).
	$(CARGO) fmt --all --check
	$(CARGO) clippy --all-targets -- -D warnings
	$(CARGO) run --quiet --bin deputyctl -- profile validate profiles/openclaw.toml profiles/hermes.toml
	@if command -v ansible-lint >/dev/null 2>&1; then \
	  ansible-lint roles/; \
	else \
	  echo "warn: ansible-lint not installed; skipping"; \
	fi
	@if command -v yamllint >/dev/null 2>&1; then \
	  yamllint roles/ .github/ cloud-init/; \
	else \
	  echo "warn: yamllint not installed; skipping"; \
	fi
	@if command -v shellcheck >/dev/null 2>&1; then \
	  shellcheck scripts/*.sh test/smoke/*.sh macos/*.sh; \
	else \
	  echo "warn: shellcheck not installed; skipping"; \
	fi
	@if command -v $(PACKER) >/dev/null 2>&1; then \
	  $(PACKER) validate -syntax-only packer/qemu-aarch64.pkr.hcl; \
	  $(PACKER) validate -syntax-only packer/qemu-x86_64.pkr.hcl; \
	  $(PACKER) validate -syntax-only packer/rpi5.pkr.hcl; \
	  $(PACKER) validate -syntax-only packer/rpi4.pkr.hcl; \
	  $(PACKER) validate -syntax-only packer/arm64-generic.pkr.hcl; \
	  $(PACKER) validate -syntax-only packer/x86_64-mini-pc.pkr.hcl; \
	  $(PACKER) validate -syntax-only packer/digitalocean.pkr.hcl; \
	  $(PACKER) validate -syntax-only packer/oracle-arm-free.pkr.hcl; \
	  $(PACKER) validate -syntax-only packer/wsl2.pkr.hcl; \
	else \
	  echo "warn: packer not installed; skipping"; \
	fi
	@# Lane B-special: PowerShell + XML + JSON validators for the
	@# wsl2 / proxmox / unraid / truenas template files. All optional —
	@# missing tools warn rather than fail.
	@if command -v pwsh >/dev/null 2>&1; then \
	  pwsh -NoProfile -Command "if (Get-Module -ListAvailable -Name PSScriptAnalyzer) { Invoke-ScriptAnalyzer -Path wsl/Install-DeputyOS.ps1 -Severity Warning,Error -EnableExit } else { Write-Host 'warn: PSScriptAnalyzer not installed; running token-parse syntax check only'; \$$null = [System.Management.Automation.PSParser]::Tokenize((Get-Content wsl/Install-DeputyOS.ps1 -Raw), [ref]\$$null); Write-Host 'pwsh: wsl/Install-DeputyOS.ps1 parses' }"; \
	else \
	  echo "warn: pwsh not installed; skipping wsl/Install-DeputyOS.ps1 syntax check"; \
	fi
	@if command -v xmllint >/dev/null 2>&1; then \
	  xmllint --noout templates/unraid/deputyos.xml; \
	else \
	  echo "warn: xmllint not installed; skipping templates/unraid/deputyos.xml"; \
	fi
	@if command -v jq >/dev/null 2>&1; then \
	  jq . templates/truenas/deputyos.json >/dev/null; \
	  jq . roles/deputyos/files/limits.wsl2.json >/dev/null; \
	  jq . roles/deputyos/files/limits.macos-qemu.json >/dev/null; \
	else \
	  python3 -m json.tool templates/truenas/deputyos.json >/dev/null; \
	  python3 -m json.tool roles/deputyos/files/limits.wsl2.json >/dev/null; \
	  python3 -m json.tool roles/deputyos/files/limits.macos-qemu.json >/dev/null; \
	fi
	@# documentation/ — strict mkdocs build verifies internal links and nav.
	@# Optional; install via `pip install -r requirements-docs.txt`.
	@if command -v mkdocs >/dev/null 2>&1; then \
	  cd documentation && mkdocs build --strict --quiet; \
	else \
	  echo "warn: mkdocs not installed; skipping (pip install -r requirements-docs.txt)"; \
	fi

test: ## Run cargo test (manifest deserialization roundtrips both real profiles).
	$(CARGO) test --all

## ===== Build / try / smoke =====

build: ## Build an image. TARGET=<hw> PROFILE=<id> [CHANNEL=...] [TIER=...] [AIRGAP=1]
	@TARGET=$(TARGET) PROFILE=$(PROFILE) CHANNEL=$(CHANNEL) TIER=$(TIER) AIRGAP=$(AIRGAP) \
	  bash scripts/build.sh

try: ## Build (if needed) + boot the artefact in qemu/UTM, forward :8088.
	@TARGET=$(TARGET) PROFILE=$(PROFILE) bash scripts/try.sh

smoke: ## Run the QEMU smoke harness for a target. SMOKE_LEVEL=scaffold|m1|full (default m1).
	@SMOKE_LEVEL=$(SMOKE_LEVEL) PROFILE=$(PROFILE) bash test/smoke/$(TARGET).sh

# matrix is intentionally limited to the QEMU-bootable smoke targets.
# rpi4 / arm64-generic / x86_64-mini-pc are real-hardware (or
# packer-builder-arm chroot) targets — they need binfmt_misc registered
# and/or actual hardware to validate, so CI cannot smoke them. They
# build via `make build TARGET=<hw>` on a contributor machine that has
# the right tooling. See docs/14-limitations.md and docs/15-local-build.md.
matrix: ## Build + smoke both qemu-aarch64 and qemu-x86_64 in sequence (proves the variant matrix).
	@echo "==> [matrix 1/4] build qemu-aarch64 PROFILE=$(PROFILE)"
	$(MAKE) build TARGET=qemu-aarch64 PROFILE=$(PROFILE)
	@echo "==> [matrix 2/4] build qemu-x86_64 PROFILE=$(PROFILE)"
	$(MAKE) build TARGET=qemu-x86_64 PROFILE=$(PROFILE)
	@echo "==> [matrix 3/4] smoke qemu-aarch64 SMOKE_LEVEL=$(SMOKE_LEVEL)"
	$(MAKE) smoke TARGET=qemu-aarch64 PROFILE=$(PROFILE) SMOKE_LEVEL=$(SMOKE_LEVEL)
	@echo "==> [matrix 4/4] smoke qemu-x86_64 SMOKE_LEVEL=$(SMOKE_LEVEL)"
	$(MAKE) smoke TARGET=qemu-x86_64 PROFILE=$(PROFILE) SMOKE_LEVEL=$(SMOKE_LEVEL)
	@echo "==> matrix: all targets built and smoked"

stage-release-artifacts: ## Link matrix outputs to manifest-conforming CalVer names before signing.
	@[[ ! -e build/staging/agentless-development ]] || { \
	  echo "error: agentless development outputs can never be staged as deputyOS releases" >&2; \
	  exit 66; \
	}
	@[[ "$(DEPUTYOS_RELEASE_VERSION)" =~ ^[0-9]{4}\.[0-9]{1,2}\.[0-9]{1,2}(-[a-z0-9.-]+)?$$ ]] || { \
	  echo "error: DEPUTYOS_RELEASE_VERSION must be Y.M.D[-pre], got '$(DEPUTYOS_RELEASE_VERSION)'" >&2; \
	  exit 64; \
	}
	@for target in qemu-aarch64 qemu-x86_64; do \
	  src="build/$${target}-$(PROFILE).qcow2"; \
	  dst="build/deputyos-$(PROFILE)-$${target}-$(DEPUTYOS_RELEASE_VERSION)-$(CHANNEL).qcow2"; \
	  [[ -f "$$src" ]] || { echo "error: release source missing: $$src" >&2; exit 1; }; \
	  ln -sf "$${src#build/}" "$$dst"; \
	  echo "==> staged $$dst -> $${src#build/}"; \
	done

## ===== Signing =====

sign-dev: ## Sign build/* with a contributor dev minisign key.
	@bash scripts/sign.sh --dev

sign-release: ## Sign build/* with the release key from $$DEPUTYOS_RELEASE_KEY (CI only).
	@bash scripts/sign.sh --release

## ===== Release loop =====
#
# manifest / publish-local / verify form Lane D's local-first release loop.
# They are NOT in `make ci` because they require real signed artefacts in
# build/, which we don't produce under SCAFFOLD_PHASE=1. A separate
# release-tag workflow (.github/workflows/release.yml) lands in M4 to
# exercise this path in CI on every release tag.

# Default release version: today, in Y.M.D form (no zero-padding).
DEPUTYOS_RELEASE_VERSION ?= $(shell date +%Y.%-m.%-d)
DEPUTYOS_KEY_MODE ?= dev

manifest: ## Generate dist/manifest.json from signed artefacts in build/. DEPUTYOS_RELEASE_VERSION=<Y.M.D> CHANNEL=<dev|beta|stable>
	@DEPUTYOS_RELEASE_VERSION=$(DEPUTYOS_RELEASE_VERSION) \
	  bash scripts/manifest.sh \
	    --release-version "$(DEPUTYOS_RELEASE_VERSION)" \
	    --channel "$(CHANNEL)" \
	    --key-mode "$(DEPUTYOS_KEY_MODE)" \
	    --validate

publish-local: manifest ## Mirror signed artefacts + manifest into dist/ for `deputyctl update --check` (file:// CDN).
	@bash scripts/publish-local.sh

publish-cdn: publish-local ## Push the dist/ tree to the artefact CDN (Backblaze B2 → cdn.deputyos.com). Requires rclone configured (default remote b2:cdn-deputyos-com; override DEPUTYOS_CDN_REMOTE).
	@bash scripts/publish-cdn.sh

publish-r2: publish-local ## Deprecated alias — forwards to the R2 remote via the compat shim. Prefer 'make publish-cdn'.
	@bash scripts/publish-r2.sh

verify: ## Rebuild a published image and assert SHA256 match. VERSION=<v> [TARGET=<hw>] [PROFILE=<id>] [DEPUTYOS_VERIFY_STRICT=1]
	@if [ -z "$(VERSION)" ]; then \
	  echo "make verify: VERSION=<release_version> required (e.g. 'make verify VERSION=2026.4.27')" >&2; \
	  exit 64; \
	fi
	@TARGET=$(TARGET) PROFILE=$(PROFILE) bash scripts/verify.sh "$(VERSION)"

verify-cdn: ## Fetch manifest from CDN and spot-check SHA256 of published artefacts. CDN_URL=<url> VERSION=<v>
	@if [ -z "$(CDN_URL)" ]; then \
	  echo "make verify-cdn: CDN_URL=<cdn-base-url> required" >&2; \
	  exit 64; \
	fi
	@if [ -z "$(VERSION)" ]; then \
	  echo "make verify-cdn: VERSION=<release_version> required" >&2; \
	  exit 64; \
	fi
	@echo "==> verify-cdn: fetching manifest from $(CDN_URL)/dev/manifest.json"
	@curl -fsSL "$(CDN_URL)/dev/manifest.json" -o /tmp/deputyos-cdn-manifest.json
	@curl -fsSL "$(CDN_URL)/dev/manifest.json.minisig" -o /tmp/deputyos-cdn-manifest.json.minisig
	@echo "==> verify-cdn: running local verify for rebuild comparison"
	@TARGET=$(TARGET) PROFILE=$(PROFILE) bash scripts/verify.sh "$(VERSION)"
	@echo "verify-cdn: OK — CDN manifest downloaded and local rebuild matches"

## ===== Release tracker (Lane M4 Lane D — local-first; not in 'make ci') =====
#
# track / track-propose / track-apply mirror the GitHub Actions cron job
# in `.github/workflows/release-tracker.yml`. CI is a thin wrapper around
# these targets — no CI-only logic. Requires network access to query the
# GitHub Releases API; pass DEPUTYOS_TRACK_OFFLINE=1 to short-circuit.

DEPUTYOS_TRACK_FLAGS ?=
ifeq ($(DEPUTYOS_TRACK_OFFLINE),1)
DEPUTYOS_TRACK_FLAGS += --offline
endif

track: ## Show which profiles have a newer upstream release (no writes; needs network).
	$(CARGO) run -q -p deputyos-track -- $(DEPUTYOS_TRACK_FLAGS) check

track-propose: ## Emit propose-<id>-<v>.patch+.json files for any upstream bumps (no writes to profiles/).
	$(CARGO) run -q -p deputyos-track -- $(DEPUTYOS_TRACK_FLAGS) propose --out-dir build/track

track-apply: ## Apply pending bumps in place to profiles/<id>.toml (CI-friendly; sets --yes).
	$(CARGO) run -q -p deputyos-track -- $(DEPUTYOS_TRACK_FLAGS) apply --yes

## ===== Wizard (local dev) =====

wizard: ## Run the deputyOS first-boot wizard locally on :8088 (no auth, dev mode).
	@echo "==> deputywizard: open http://localhost:8088/wizard"
	@DEPUTYWIZARD_DEV=1 $(CARGO) run -p deputywizard -- serve --port 8088 --no-token

pwa: ## Run the deputyOS always-on PWA locally on :8089 (dev-stub data, loopback only).
	@echo "==> deputypwa: open http://localhost:8089/app/dashboard"
	@DEPUTYPWA_DEV_STUB=1 DEPUTYPWA_DATA_DIR=./dev-out/pwa \
	  $(CARGO) run -p deputypwa -- serve --port 8089 --bind 127.0.0.1

## ===== Desktop launcher (M2.5; local-first cross-compile) =====
#
# Builds the `deputyos-desktop` binary for a Rust target triple. The launcher
# is the "double-click on Win/Mac/Linux → wizard in browser" entry point;
# see docs/11-roadmap.md § M2.5.
#
# Uses a SEPARATE variable (`DESKTOP_TARGET`) from the rest of the build
# matrix because `TARGET` here means "hardware/device target" (qemu-aarch64,
# rpi5, …) whereas `cargo --target` expects a Rust target triple.
#
# Cross-compile notes:
# - Linux→Linux: native cargo (DESKTOP_TARGET=x86_64-unknown-linux-gnu, etc.).
# - Linux→Windows: `cargo install cross` then DESKTOP_TARGET=x86_64-pc-windows-gnu;
#   we deliberately don't gate this target on `cross` being installed —
#   cargo will error with a clear toolchain hint if the linker is missing.
# - macOS: build on a Mac (`x86_64-apple-darwin`, `aarch64-apple-darwin`).
#   Linker requires Apple toolchain blobs we cannot legally redistribute.

desktop-launcher: ## Build the desktop launcher binary. DESKTOP_TARGET=<triple> (default: host triple).
	@echo "==> deputyos-desktop: building $(if $(DESKTOP_TARGET),for $(DESKTOP_TARGET),for host triple)"
	$(CARGO) build --release -p deputyos-desktop $(if $(DESKTOP_TARGET),--target=$(DESKTOP_TARGET))

desktop-launcher-release: ## Build the launcher for the host triple + stage it in build/deputyos-desktop-<triple> for signing/manifesting.
	@echo "==> deputyos-desktop: building for host triple"
	$(CARGO) build --release -p deputyos-desktop
	@triple="$$(bash scripts/host-triple.sh)" && \
	  mkdir -p build && \
	  cp target/release/deputyos-desktop "build/deputyos-desktop-$$triple" && \
	  echo "==> staged build/deputyos-desktop-$$triple (run 'make sign-dev' to sign it; 'make manifest' to emit desktop_launchers)"

console: ## Build the deputyOS Console desktop GUI (Tauri). Requires webview dev deps on Linux.
	@# The GUI bin is gated behind the `gui` feature so the testable core
	@# (api_client/store/instance_ops) builds without webview deps. Building
	@# the GUI on Linux requires libwebkit2gtk-4.1-dev, libgtk-3-dev,
	@# librsvg2-dev (+ a JavaScript engine header). On Debian/Ubuntu:
	@#   sudo apt install libwebkit2gtk-4.1-dev libgtk-3-dev librsvg2-dev
	@# `cargo tauri dev` (hot reload) needs the `tauri-cli` crate installed.
	@echo "==> deputyos-console: building GUI (gui feature)"
	$(CARGO) build -p deputyos-console --features gui

console-bundle: ## Package the deputyOS Console into an OS-native installer (AppImage/.deb on Linux, .dmg on macOS, .msi on Windows). Needs `cargo tauri` + the webview dev deps.
	@# `cargo tauri build` reads deputyos-console/tauri.conf.json (bundle.active:true)
	@# and produces installers under
	@#   target/release/bundle/{appimage,deb,dmg,msi}/.
	@# Unsigned by default (see docs/how-to/desktop-console.md for the
	@# Gatekeeper/SmartScreen "open anyway" bypass). Set the signing env
	@# (APPLE_SIGNING_IDENTITY / tauri Windows cert vars) to sign in CI.
	@command -v cargo-tauri >/dev/null 2>&1 || { echo "error: cargo-tauri not installed — run 'cargo install tauri-cli --locked'"; exit 1; }
	@echo "==> deputyos-console: bundling installers (tauri build)"
	cd deputyos-console && cargo tauri build
	@echo "==> bundles in target/release/bundle/"

## ===== Run deputyOS locally (M2.5 dev loop) =====
#
# A Docker "remote" (release CDN + accounts/tunnel/backup API) that the
# desktop installer pulls from, boots a real qemu-x86_64 VM, and whose
# in-VM agent talks to the local Docker API instead of api.deputyos.com.
# See documentation/docs/how-to/develop/run-locally.md.

cdn-up: ## Start the local dev stack: cdn (:8090) + api (:3000) + www (:4321).
	docker compose -f docker-compose.dev.yml up -d cdn api www

cdn-down: ## Stop the local dev stack (cdn + api + www).
	docker compose -f docker-compose.dev.yml down

desktop-local-build: ## Build the qcow2 + launcher, sign, manifest, publish to dist/ for the local CDN. PROFILE=<id> (default openclaw).
	@# 0. Fail fast if a tool the loop needs (minisign, packer, ansible, qemu,
	@#    ISO builder, docker) is missing — before the slow Packer build runs.
	@PROFILE=$(PROFILE) bash scripts/desktop-local-preflight.sh
	@# 1. Build the qemu-x86_64 image if not already present (Packer; slow first run).
	@[ -f build/qemu-x86_64-$(PROFILE).qcow2 ] || $(MAKE) build TARGET=qemu-x86_64 PROFILE=$(PROFILE) CHANNEL=$(CHANNEL)
	@# 2. Bridge the Packer output name (build/<target>-<profile>.qcow2) to the
	@#    manifest-conforming name (deputyos-<profile>-<target>-<ver>-<chan>.qcow2)
	@#    so scripts/manifest.sh's filename regex matches. Symlink, not copy —
	@#    the smoke harness still reads the real file; manifest.sh reads the link.
	@ln -sf "qemu-x86_64-$(PROFILE).qcow2" \
	  "build/deputyos-$(PROFILE)-qemu-x86_64-$(DEPUTYOS_RELEASE_VERSION)-$(CHANNEL).qcow2"
	@# 3. Build + stage the host-triple launcher (sign-dev signs it next).
	@$(MAKE) desktop-launcher-release
	@# 4. Sign all build/* artefacts (qcow2 symlink + launcher) with the dev key.
	@$(MAKE) sign-dev
	@# 5. manifest (artefacts + desktop_launchers) + publish to dist/<version>/.
	@$(MAKE) publish-local DEPUTYOS_RELEASE_VERSION=$(DEPUTYOS_RELEASE_VERSION) CHANNEL=$(CHANNEL)
	@echo "==> desktop-local-build: dist/ ready for 'make cdn-up'"

desktop-local: ## Install + start the VM against the local CDN (run after desktop-local-build + cdn-up).
	@PROFILE=$(PROFILE) bash scripts/desktop-local-preflight.sh
	@PROFILE=$(PROFILE) bash scripts/desktop-local.sh

## ===== Supply chain (M7 Lane B — release-time only; intentionally NOT in 'make ci') =====
#
# These targets emit SBOM + SLSA-provenance artefacts alongside signed
# release images. They're scaffold-grade today: full SLSA L3 verification
# requires hermetic builds (M4) plus bit-identical reproduction across
# independent builders, both tracked in §M7 of docs/11-roadmap.md. The
# *generators* land here; the *verifier* lands when M7 closes.
#
# Adding them to `make ci` would cost minutes per run with little signal,
# so they're release-time only — exercised by the manifest workflow.

sbom: ## Generate a CycloneDX SBOM next to ARTEFACT. Uses syft if installed; else best-effort fallback.
	@if [ -z "$(ARTEFACT)" ]; then \
	  echo "make sbom: ARTEFACT=<path> required" >&2; exit 64; \
	fi
	@bash scripts/sbom.sh "$(ARTEFACT)"

sbom-all: ## Generate a CycloneDX SBOM for every signable artefact in build/.
	@shopt -s nullglob; \
	  found=0; \
	  for a in build/*.qcow2 build/*.img build/*.img.xz; do \
	    found=1; \
	    bash scripts/sbom.sh "$$a"; \
	  done; \
	  if [ "$$found" = "0" ]; then \
	    echo "info: no signable artefacts in build/ (looked for *.qcow2 *.img *.img.xz)"; \
	  fi

slsa-attest: ## Generate a SLSA v1 provenance + sign next to ARTEFACT.
	@if [ -z "$(ARTEFACT)" ]; then \
	  echo "make slsa-attest: ARTEFACT=<path> required" >&2; exit 64; \
	fi
	@bash scripts/slsa-attest.sh "$(ARTEFACT)"

slsa-all: ## Generate a SLSA v1 provenance for every signable artefact in build/.
	@shopt -s nullglob; \
	  found=0; \
	  for a in build/*.qcow2 build/*.img build/*.img.xz; do \
	    found=1; \
	    bash scripts/slsa-attest.sh "$$a"; \
	  done; \
	  if [ "$$found" = "0" ]; then \
	    echo "info: no signable artefacts in build/ (looked for *.qcow2 *.img *.img.xz)"; \
	  fi

#slsa-verify: ## Inspect SLSA provenance attestation structure. ARTEFACT=<path> or ARTEFACT_URL=<url>.
	@if [ -z "$(ARTEFACT)" ]; then \
	  echo "make slsa-verify: ARTEFACT=<path-to-.intoto.jsonl> required" >&2; \
	  echo "  or: make slsa-verify ARTEFACT_URL=<url>" >&2; \
	  exit 64; \
	fi
	@bash scripts/slsa-verify.sh "$(if $(ARTEFACT_URL),--url $(ARTEFACT_URL),$(ARTEFACT))"

# ===== CI =====

ci: ## Meta-target: lint test (build smoke if SCAFFOLD_PHASE=0) sign-dev. CI runs this.
	$(MAKE) lint
	$(MAKE) test
	@if [ "$(SCAFFOLD_PHASE)" = "0" ]; then \
	  $(MAKE) build TARGET=$(TARGET) PROFILE=$(PROFILE); \
	  $(MAKE) smoke TARGET=$(TARGET) PROFILE=$(PROFILE); \
	else \
	  echo "info: SCAFFOLD_PHASE=1; skipping build+smoke (lane B pins base image SHA in M1)"; \
	fi
	$(MAKE) sign-dev

## ===== Documentation =====

docs: ## Serve the technical docs site locally on :8000 (live reload).
	@if ! command -v mkdocs >/dev/null 2>&1; then \
	  echo "error: mkdocs not installed. Install with: pip install -r requirements-docs.txt"; \
	  exit 1; \
	fi
	cd documentation && mkdocs serve --dev-addr=127.0.0.1:8000

docs-build: ## Build the static docs site to documentation/site/.
	@if ! command -v mkdocs >/dev/null 2>&1; then \
	  echo "error: mkdocs not installed. Install with: pip install -r requirements-docs.txt"; \
	  exit 1; \
	fi
	cd documentation && mkdocs build --strict

## ===== Public sites (M4a — complete) =====
#
# `www.deputyos.com` — sibling private repo `deputyos/www-deputyos-com`
#   (Astro 5 + MDX + Tailwind + Pagefind). Deploys via its own CI.
#
# `api.deputyos.com` — sibling private repo `deputyos/api-deputyos-com`
#   (Rust Axum API + Vue 3 status dashboard). Deploys via its own CI.
#   See ../api-deputyos-com/ for local dev instructions.
#
# `docs.deputyos.com` — this repo's `documentation/` directory (MkDocs).
#   Use `make docs-build` locally. Deploys via `.github/workflows/docs-deploy.yml`.

## ===== Cleanup =====

clean: ## Remove cargo and build artefacts.
	$(CARGO) clean
	rm -rf build/ documentation/site/
