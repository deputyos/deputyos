//! Progress-bar download with sha256 + minisign verification.
//!
//! Wraps `deputyctl::release::{fetch_to, verify_sha256, verify_manifest_signature}`
//! so the launcher's download path is byte-for-byte the same trust pipeline
//! `deputyctl update` uses. Today this prints simple stderr lines for each
//! step; an `indicatif`-rendered progress bar lands in M2.5-rest (the
//! launcher's downloads can be ~2 GB and silent waits hurt non-technical
//! users).

use std::path::Path;

use anyhow::{Context, Result};
use deputyctl::release::{fetch_to, verify_sha256};

/// Download `url` to `dest`, verify SHA256, verify detached minisign sig.
///
/// Order matters: we sha-verify **before** sig-verify because a corrupt
/// download + matching sig is the security-relevant nightmare; sha mismatch
/// is the common case (network interruption) and we want to surface that
/// first with a clear message.
pub fn download_and_verify(
    url: &str,
    sig_url: &str,
    dest: &Path,
    sig_dest: &Path,
    expected_sha256: &str,
    pubkey: &Path,
) -> Result<()> {
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating cache dir {}", parent.display()))?;
    }
    eprintln!("==> downloading {url}");
    fetch_to(url, dest).with_context(|| format!("downloading {url}"))?;

    eprintln!("==> verifying sha256");
    verify_sha256(dest, expected_sha256).context("sha256 mismatch on downloaded image")?;

    eprintln!("==> downloading signature {sig_url}");
    fetch_to(sig_url, sig_dest).with_context(|| format!("downloading {sig_url}"))?;

    eprintln!("==> verifying signature");
    deputyctl::release::verify_manifest_signature(dest, sig_dest, pubkey)
        .context("artefact signature verification failed")?;

    eprintln!("==> verified ok");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn sha256_mismatch_surfaces_clear_error() {
        let dir = tempfile::tempdir().expect("tempdir");
        let src = dir.path().join("src.bin");
        std::fs::File::create(&src)
            .expect("create")
            .write_all(b"hello world")
            .expect("write");
        let dest = dir.path().join("dest.bin");
        let sig_dest = dir.path().join("dest.bin.minisig");
        let pubkey = dir.path().join("k.pub");
        std::fs::write(&pubkey, "untrusted comment: x\nNOTAREALKEY").expect("write pubkey");

        let bogus_sig_src = dir.path().join("src.minisig");
        std::fs::write(&bogus_sig_src, "untrusted comment: x\nbogus").expect("sig src");

        let url = format!("file://{}", src.to_str().expect("utf8"));
        let sig_url = format!("file://{}", bogus_sig_src.to_str().expect("utf8"));

        let err = download_and_verify(
            &url,
            &sig_url,
            &dest,
            &sig_dest,
            // wrong sha — file is "hello world", not zeros.
            &"0".repeat(64),
            &pubkey,
        )
        .expect_err("sha mismatch should fail");
        let msg = format!("{err:#}");
        assert!(msg.contains("sha256"), "expected sha error, got: {msg}");
    }
}
