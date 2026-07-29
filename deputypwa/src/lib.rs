//! `deputypwa` — always-on companion Progressive Web App for deputyOS.
//!
//! Unlike [`deputywizard`], which is first-boot-gated via systemd
//! `ConditionPathExists` and exits once the wizard's done-marker exists,
//! `deputypwa` runs continuously alongside the agent. Operators point a phone
//! or any LAN browser at `http://<host>:8089/app/dashboard` to monitor and
//! manage the device.
//!
//! Architecture: Axum + tokio web server, no database. Reads structured data
//! by shelling out to `deputyctl <cmd> --json` (low-frequency, acceptable for
//! a status surface). Renders hand-written `format!` HTML in the same shape
//! as deputywizard for consistency. Service worker + Web Push for cost-alert
//! notifications; the firing helper is a public lib export so future hooks
//! (cost-alert, doctor-fail) can call into it.
//!
//! TODO(M3-rest): bake `deputypwa` into the Lane B Ansible role at
//! `roles/deputyos/tasks/pwa-baseline.yml`. Until then the unit reference at
//! `deputypwa/contrib/deputypwa.service` is the documented contract.

pub mod data;
pub mod paths;
pub mod push;
pub mod routes;
pub mod templates;

pub use push::fire_push_notification;
pub use routes::{router, AppState};
