//! `deputyos-track` — release-tracker library surface.
//!
//! Public modules used by the binary and integration tests. The library
//! exposes pure functions so tests can exercise the comparison and TOML
//! patching logic without touching the network.

pub mod github;
pub mod patch;
pub mod profile;
pub mod version;
