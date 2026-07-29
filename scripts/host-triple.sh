#!/usr/bin/env bash
# scripts/host-triple.sh — print the Rust target triple the desktop launcher
# is built for on THIS host (M2.5). Mirrors deputyos-desktop::config::triple_for
# (os, arch) so the build pipeline and the Rust code agree on the triple that
# indexes manifest.desktop_launchers[<triple>].
#
# Used by `make desktop-launcher-release` (stage the launcher binary into
# build/deputyos-desktop-<triple>) and by scripts/desktop-local.sh.
set -euo pipefail

os="$(uname -s | tr '[:upper:]' '[:lower:]')"
arch="$(uname -m)"

case "$os/$arch" in
  linux/x86_64)   echo "x86_64-unknown-linux-gnu" ;;
  linux/aarch64) echo "aarch64-unknown-linux-gnu" ;;
  darwin/aarch64) echo "aarch64-apple-darwin" ;;
  darwin/x86_64)  echo "x86_64-apple-darwin" ;;
  mingw*/x86_64|windows/x86_64) echo "x86_64-pc-windows-msvc" ;;
  *)
    echo "scripts/host-triple.sh: unknown host os/arch: $os/$arch" >&2
    echo "  (the launcher only builds for x86_64/aarch64 linux, x86_64/aarch64 darwin, x86_64 windows)" >&2
    exit 1
    ;;
esac