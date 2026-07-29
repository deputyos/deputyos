//! `deputywizard` — first-boot web wizard for deputyOS appliance images.
//!
//! TODO(M3-rest): bake `deputywizard` into the image via the Lane B Ansible
//! role; copy the binary to `/usr/local/bin/deputywizard` and have systemd
//! start it on first boot, with the QR-code printer reading the auth token
//! from `/run/deputyos/wizard.token` (mode 0600, single-use, 1h expiry).
//!
//! Architecture: Axum + tokio web server, in-process state machine, no
//! database. Wizard answers persist to a JSON file (canonical at
//! `/var/lib/deputyos/wizard-state.json`, env-overridable for tests). The
//! "apply" step is the only point at which we touch real config files
//! (`/etc/hostname`, `/etc/deputyos/active-profile`, `/etc/deputyos/secrets.env`,
//! `/home/agent/.ssh/authorized_keys`, ufw rules, systemctl). In dev mode,
//! all of those go to a `dev-out/` directory mirror so contributors can
//! exercise the full flow on a laptop without root.
//!
//! `deputyctl init` shells out to `deputywizard serve`.
//!
//! Lib surface kept narrow so integration tests can drive the router
//! directly.

pub mod apply;
pub mod auth;
pub mod chat;
pub mod provider_check;
pub mod qr;
pub mod routes;
pub mod runtime_bridge;
pub mod state;
pub mod templates;

pub use routes::AppState;
