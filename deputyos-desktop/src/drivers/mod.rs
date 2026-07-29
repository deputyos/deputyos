//! Per-platform VM drivers.
//!
//! Each module is `#[cfg]`-gated to its host. The launcher binary you ship
//! for Linux contains only `linux.rs`; the Windows binary only `windows.rs`;
//! the Mac binary only `macos.rs`. There is no runtime dispatch — see
//! `crate::driver::current_driver` for the compile-time selection.
//!
//! This is honest about what cross-compilation buys us: a Linux binary
//! cannot run a Mac driver at runtime even if it had one. The "one-binary-
//! per-platform" shape mirrors how `deputyctl`, `deputywizard`, and
//! `deputypwa` already ship.

#[cfg(target_os = "linux")]
pub mod linux;
#[cfg(target_os = "linux")]
mod qemu_control;

#[cfg(any(target_os = "windows", test))]
pub mod windows;

#[cfg(any(target_os = "macos", test))]
pub mod macos;
