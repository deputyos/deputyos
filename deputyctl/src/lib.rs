//! `deputyctl` library surface.
//!
//! Public modules consumed by the binary and integration tests. Kept narrow
//! so the binary entry point stays a thin command-router.

pub(crate) mod agekey;
pub mod apibase;
pub mod audit;
pub mod backup;
pub mod commands;
pub mod cost;
pub mod doctor;
pub mod factory_reset;
pub mod hooks;
pub mod limits;
pub mod manifest;
pub mod message_relay;
pub mod model;
pub mod model_register;
pub mod model_set;
pub mod model_test;
pub mod mounts;
pub mod network;
pub mod paths;
pub mod profile;
pub mod profile_switch;
pub mod quiet_hours;
pub mod reconcile;
pub mod recovery_key;
pub mod release;
pub mod restore;
pub mod rollback;
pub mod shell;
pub mod systemd;
pub mod tunnel;
pub mod update;
pub mod validate;
pub mod voice;
pub mod watchdog;

/// Process-wide lock used by tests that mutate env vars. Tests acquire it on
/// entry to avoid racing each other; production code never touches it.
#[cfg(test)]
pub(crate) fn env_mutex() -> &'static std::sync::Mutex<()> {
    use std::sync::{Mutex, OnceLock};
    static M: OnceLock<Mutex<()>> = OnceLock::new();
    M.get_or_init(|| Mutex::new(()))
}
