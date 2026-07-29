//! `deputyctl model register` — add a GGUF model on a running device.
//!
//! Accepts a local `.gguf` file, copies it to `/var/lib/deputyos/models/`,
//! adds an entry to `/opt/deputyos/airgap/models/catalog.json`, and
//! optionally creates + enables a systemd template instance so the model
//! is served by llama.cpp immediately.

use std::path::PathBuf;

use anyhow::{bail, Result};

use crate::model;

#[derive(Debug, Clone)]
pub struct RegisterOpts {
    pub path: PathBuf,
    pub id: String,
    pub enable: bool,
}

pub fn run(opts: RegisterOpts) -> Result<u8> {
    if !opts.path.is_file() {
        bail!("{} does not exist or is not a file", opts.path.display());
    }
    if !opts.path.extension().map(|e| e == "gguf").unwrap_or(false) {
        bail!(
            "{}: expected a .gguf file",
            opts.path.file_name().unwrap_or_default().to_string_lossy()
        );
    }
    if opts.id.is_empty() || opts.id.len() > 64 {
        bail!("--id is required and must be <= 64 characters");
    }

    let unit = model::register_gguf(&opts.path, &opts.id, opts.enable)?;
    println!("model registered: {}", opts.id);
    if opts.enable && unit.ends_with(".service") {
        println!("systemd unit enabled: {unit}");
    }
    println!("view with: deputyctl model list");
    Ok(0)
}
