//! deputyOS Console — Tauri desktop shell entry point.
//!
//! Built only with the `gui` feature (the `deputyos-console` bin target requires
//! it). The deps-light core ([`deputyos_console::api_client`],
//! [`deputyos_console::store`], [`deputyos_console::instance_ops`]) builds and
//! unit-tests *without* the feature, so the GUI's webview deps
//! (libwebkit2gtk-4.1-dev / libgtk-3-dev on Linux) aren't needed for the
//! testable surface. This file pulls Tauri + the system webview.
//!
//! Run the GUI: `cargo build -p deputyos-console --features gui` (needs the
//! Linux webview dev packages) or `make console` → `cargo tauri dev`.

use deputyos_console::api_client::DEFAULT_API_BASE;
use deputyos_console::commands::GuiState;

fn main() {
    // Allow pointing the console at a non-default API (dev/staging) via env.
    let api_base =
        std::env::var("DEPUTYOS_API_BASE").unwrap_or_else(|_| DEFAULT_API_BASE.to_string());

    tauri::Builder::default()
        .manage(GuiState::new(&api_base))
        .invoke_handler(tauri::generate_handler![
            deputyos_console::commands::login_start,
            deputyos_console::commands::login_poll,
            deputyos_console::commands::login_status,
            deputyos_console::commands::logout,
            deputyos_console::commands::list_instances,
            deputyos_console::commands::create_instance,
            deputyos_console::commands::delete_instance,
            deputyos_console::commands::start_instance,
            deputyos_console::commands::stop_instance,
            deputyos_console::commands::pause_instance,
            deputyos_console::commands::resume_instance,
            deputyos_console::commands::set_instance_memory,
            deputyos_console::commands::configure_instance_resources,
            deputyos_console::commands::instance_agent_health,
            deputyos_console::commands::status_instance,
            deputyos_console::commands::install_instance,
            deputyos_console::commands::open_wizard,
            deputyos_console::commands::open_url,
            deputyos_console::commands::host_prereq,
            deputyos_console::commands::list_fleet,
            deputyos_console::commands::open_remote_wizard,
            deputyos_console::commands::open_remote_surface,
            deputyos_console::commands::queue_remote_command,
        ])
        .setup(|app| {
            // The dashboard webview. UI is hand-rolled under src/ui/ (no JS
            // build step, no CDN — matches the embedded wizard webviews).
            tauri::WebviewWindowBuilder::new(
                app,
                "main",
                tauri::WebviewUrl::App("index.html".into()),
            )
            .title("deputyOS Console")
            .inner_size(1100.0, 760.0)
            .min_inner_size(720.0, 480.0)
            .build()?;
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running deputyOS console");
}
