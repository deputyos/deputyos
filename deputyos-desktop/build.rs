//! Stage the release public key for `include_str!` at compile time.
//!
//! Contributors set `DEPUTYOS_DESKTOP_EMBEDDED_PUBKEY` to a file path
//! containing the minisign public key. In CI, this is set from a secret.
//! Without it, the binary falls back to reading the pubkey from the
//! filesystem at runtime (dev mode).

fn main() {
    use std::path::PathBuf;

    println!("cargo:rerun-if-env-changed=DEPUTYOS_DESKTOP_EMBEDDED_PUBKEY");
    let out_path = PathBuf::from(std::env::var_os("OUT_DIR").expect("OUT_DIR"))
        .join("embedded-pubkey.minisign");

    let key = match std::env::var("DEPUTYOS_DESKTOP_EMBEDDED_PUBKEY") {
        Ok(path) => std::fs::read_to_string(&path).unwrap_or_else(|e| {
            panic!("failed to read embedded pubkey from {path}: {e}");
        }),
        Err(_) => String::new(),
    };
    std::fs::write(&out_path, key).expect("writing staged embedded pubkey");
    println!(
        "cargo:rustc-env=DEPUTYOS_EMBEDDED_PUBKEY_PATH={}",
        out_path.display()
    );
}
