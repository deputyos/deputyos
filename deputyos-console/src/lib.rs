//! `deputyos_console` — cross-platform desktop console for deputyOS.
//!
//! The console logs into the deputyOS API (device-code flow) and manages
//! **multiple** local and remote (tunneled) agents from one UI:
//!
//! - **Local agents**: drives the `deputyos-desktop` named-instance registry
//!   and the host's Linux/qemu, Windows/WSL2, or macOS/UTM driver. Linux and
//!   Windows support per-instance port mapping; UTM currently requires the
//!   default ports and one running local instance at a time.
//! - **Remote agents**: lists the account's registered devices (fleet) and
//!   opens a remote device's wizard/chat through the tunnel proxy using the
//!   account JWT for auth (Phase C).
//!
//! ## Two build modes
//!
//! The crate is split so the **testable core** has no system dependencies:
//! - [`api_client`] — the deputyOS API client (device-code login, token
//!   lifecycle, fleet). Pure ureq + serde, unit-tested with `httpmock`.
//! - [`store`] — token persistence ([`FileTokenStore`] always available;
//!   [`KeyringStore`] behind the `gui` feature).
//! - [`instance_ops`] — wraps `deputyos-desktop` to list/create/start/stop/
//!   install local instances.
//!
//! The **GUI binary** (`src/main.rs` + [`commands`] + the embedded web UI)
//! is gated behind the `gui` feature, which pulls `tauri` + `keyring` and
//! requires the host's webview build deps (on Linux:
//! `libwebkit2gtk-4.1-dev`, `libgtk-3-dev`, `librsvg2-dev`). Build with
//! `cargo build -p deputyos-console --features gui`; run with
//! `cargo tauri dev -p deputyos-console`.

pub mod api_client;
pub mod instance_ops;
pub mod store;

#[cfg(feature = "gui")]
pub mod commands;

pub use deputyos_desktop::{Instance, InstanceConfig, Registry, VmHandle, VmStatus};
