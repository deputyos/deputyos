//! Build script — only runs the Tauri build step under the `gui` feature.
//!
//! Without `gui`, this is a no-op (the core lib builds with no codegen step).

#[cfg(feature = "gui")]
fn main() {
    tauri_build::build()
}

#[cfg(not(feature = "gui"))]
fn main() {}
