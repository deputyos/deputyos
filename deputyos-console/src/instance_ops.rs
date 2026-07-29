//! Shared local deputy runtime re-export.
//!
//! The implementation lives in `deputyos-desktop` so the native GUI and CLI
//! execute exactly the same registry, install, lifecycle, and resource code.

pub use deputyos_desktop::runtime::InstanceOps;
