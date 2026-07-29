//! `deputyos-desktop` library surface.
//!
//! The desktop launcher is a thin wrapper around three jobs:
//!
//! 1. **Manifest fetch + signature verify** — reuses `deputyctl::release` end-to-end.
//!    No new trust surface: the same minisign pubkey verifies both
//!    `deputyctl update` and `deputyos-desktop install`.
//! 2. **Cache management** — keeps the latest signed image in a platform-canonical
//!    cache dir (XDG on Linux, `%LOCALAPPDATA%` on Windows, `~/Library/Caches`
//!    on macOS). Ageing/eviction is M2.5-rest.
//! 3. **VM lifecycle** — delegates to a per-platform [`Driver`] that knows how
//!    to drive the host's native virtualization (qemu+KVM on Linux, WSL2 on
//!    Windows, UTM on macOS). Drivers are `#[cfg]`-gated to the host they
//!    target — the launcher binary is per-platform by construction.
//!
//! Hard rule: the launcher **never bundles QEMU**. It mandates the host's
//! native hypervisor and prints exact install instructions if it's missing.
//! See `docs/11-roadmap.md` § M2.5 for the full rationale.

pub mod browser;
pub mod config;
pub mod download;
pub mod driver;
pub mod instance;
pub mod manifest;
pub mod runtime;
pub mod selfupdate;

pub mod drivers;

pub use driver::{current_driver, Driver, DriverCapabilities, VmHandle, VmStatus};
pub use instance::{allocate_port_pair, Instance, InstanceConfig, Registry, ResourceSpec};
