//! Release-manifest types, loader, and signature verification.
//!
//! Schema: `docs/schemas/manifest-v1.json` (single source of truth — the
//! generator [`scripts/manifest.sh`] and this parser both reference that
//! file). The structs here are derived from the schema by hand because
//! we want stable serde behavior and meaningful field documentation; a
//! schema-roundtrip test in `tests::manifest_roundtrip` keeps them in sync.
//!
//! Hard rule: parser is **strict on schema_version** but **forgiving on
//! unknown fields**. We accept future, additive fields silently so older
//! deputyctl binaries can read newer manifests for informational purposes
//! (the channel-default manifest will always include all M-future fields).
//! We refuse `schema_version != 1` because semantic interpretation of
//! a v2 manifest is undefined for v1 code.

use std::path::{Path, PathBuf};

use anyhow::{anyhow, bail, Context, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// One signed manifest. Round-trips `dist/manifest.json` produced by
/// `scripts/manifest.sh`. Documented field-by-field in
/// `docs/schemas/manifest-v1.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Manifest {
    /// Hard-pinned to 1 (see [`Manifest::SCHEMA_VERSION`]).
    pub schema_version: u32,
    /// CalVer Y.M.D, optionally suffixed (e.g. `2026.4.27`, `2026.4.27-rc1`).
    /// Validated by [`is_valid_release_version`].
    pub release_version: String,
    /// `dev` | `beta` | `stable`.
    pub channel: String,
    /// RFC 3339 timestamp.
    pub released_at: String,
    /// Upstream agent versions baked into this release. Free-form; this
    /// crate doesn't interpret it — only displays it.
    #[serde(default)]
    pub tracker: std::collections::BTreeMap<String, String>,
    /// One entry per signed image, OCI ref, or cloud snapshot.
    pub artefacts: Vec<Artefact>,
    #[serde(default)]
    pub wizard_version: Option<String>,
    #[serde(default)]
    pub chat_ui_version: Option<String>,
    /// Desktop launcher binaries per Rust target triple (M2.5).
    #[serde(default)]
    pub desktop_launchers: std::collections::BTreeMap<String, DesktopLauncher>,
    /// Schema version of `/etc/deputyos/mounts-policy.json` this release ships
    /// (M3.5 Lane D). Bumped when the on-disk mounts-policy schema changes;
    /// consumers can detect a mismatch and migrate. Older manifests omit it
    /// and deserialize as 1.
    #[serde(default = "default_mounts_policy_schema_version")]
    pub mounts_policy_schema_version: u32,
}

fn default_mounts_policy_schema_version() -> u32 {
    crate::mounts::POLICY_SCHEMA_VERSION
}

/// Per-triple desktop launcher artefact (M2.5).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DesktopLauncher {
    /// Rust target triple, e.g. `x86_64-unknown-linux-gnu`.
    pub triple: String,
    pub filename: String,
    pub url: String,
    pub sha256: String,
    pub minisig_url: String,
}

impl Manifest {
    /// Schema version this code understands. See `docs/schemas/manifest-v1.json`.
    pub const SCHEMA_VERSION: u32 = 1;
}

/// One artefact entry. See `docs/schemas/manifest-v1.json` for the full
/// field-level spec.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Artefact {
    /// Hardware target (e.g. `qemu-aarch64`, `rpi5`, `digitalocean`).
    pub target: String,
    /// Profile id (e.g. `openclaw`, `hermes`).
    pub profile: String,
    /// Basename of the artefact, per the deputyos-* naming convention.
    pub filename: String,
    /// `img.xz` | `qcow2` | `tar.gz` | `do-snapshot` | `oci`.
    pub format: String,
    pub size_bytes: u64,
    /// Lowercase hex-encoded sha256 of the artefact bytes.
    pub sha256: String,
    /// URL of the detached `.minisig`. May be relative.
    pub minisig_url: String,
    /// URL of the artefact itself. May be relative; resolved against the
    /// manifest URL.
    #[serde(default)]
    pub url: Option<String>,
}

/// Source of a manifest — what URL or path it came from. Used so callers
/// can resolve relative artefact URLs against the manifest's own location.
#[derive(Debug)]
pub struct ManifestSource {
    pub manifest: Manifest,
    pub origin: String,
    /// Local path the manifest bytes are buffered at (so callers can
    /// pass them to minisign). Always present — HTTP fetches stage to a
    /// tempfile.
    pub local_path: PathBuf,
    /// Caller takes ownership of the tempdir, if one was created. We hold
    /// it here so the manifest file isn't deleted while in scope.
    _tempdir: Option<tempdir::TempDir>,
}

/// Lightweight tempdir wrapper so we don't pull in a heavyweight dep.
mod tempdir {
    use std::path::{Path, PathBuf};

    /// RAII tempdir. Drops the directory on Drop.
    #[derive(Debug)]
    pub struct TempDir {
        path: PathBuf,
    }

    impl TempDir {
        pub fn new(prefix: &str) -> std::io::Result<Self> {
            // Use the standard library tempfile via tempfile crate (already
            // a dev-dep, but we want it as a regular dep). Keep this
            // module self-contained — std doesn't expose a tempdir API
            // directly, so we cobble one up.
            let mut base = std::env::temp_dir();
            let nanos = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0);
            base.push(format!("{prefix}-{nanos}-{}", std::process::id()));
            std::fs::create_dir_all(&base)?;
            Ok(Self { path: base })
        }

        pub fn path(&self) -> &Path {
            &self.path
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.path);
        }
    }
}

/// Load a manifest from a URL or path. Supports:
/// - `https://...`, `http://...` — fetched via ureq.
/// - `file:///abs/path/to/manifest.json` — read directly.
/// - bare path (`./dist/manifest.json`, `/etc/.../manifest.json`) — read directly.
///
/// Returns the parsed manifest along with a local-path handle so the caller
/// can run signature verification against the same bytes we parsed.
pub fn load_manifest(url_or_path: &str) -> Result<ManifestSource> {
    let (local_path, tempdir) = stage_local(url_or_path)?;
    let raw = std::fs::read_to_string(&local_path)
        .with_context(|| format!("reading manifest from {}", local_path.display()))?;
    let manifest: Manifest =
        serde_json::from_str(&raw).with_context(|| format!("parsing manifest at {url_or_path}"))?;
    if manifest.schema_version != Manifest::SCHEMA_VERSION {
        bail!(
            "unsupported manifest schema_version {} (this deputyctl understands v{}); upgrade deputyctl",
            manifest.schema_version,
            Manifest::SCHEMA_VERSION
        );
    }
    if !is_valid_release_version(&manifest.release_version) {
        bail!(
            "manifest release_version '{}' is not Y.M.D[-pre]",
            manifest.release_version
        );
    }
    if !matches!(manifest.channel.as_str(), "dev" | "beta" | "stable") {
        bail!(
            "manifest channel '{}' is not one of dev|beta|stable",
            manifest.channel
        );
    }
    if manifest.artefacts.is_empty() {
        bail!("manifest has no artefacts");
    }
    Ok(ManifestSource {
        manifest,
        origin: url_or_path.to_string(),
        local_path,
        _tempdir: tempdir,
    })
}

/// Stage `url_or_path` to a local file path, returning the path plus an
/// optional tempdir guard (for HTTP fetches).
fn stage_local(url_or_path: &str) -> Result<(PathBuf, Option<tempdir::TempDir>)> {
    if let Some(rest) = url_or_path.strip_prefix("file://") {
        return Ok((PathBuf::from(rest), None));
    }
    if url_or_path.starts_with("http://") || url_or_path.starts_with("https://") {
        let td = tempdir::TempDir::new("deputyctl-manifest")?;
        let dest = td.path().join("manifest.json");
        let resp = ureq::get(url_or_path)
            .call()
            .map_err(|e| anyhow!("HTTP fetch of {url_or_path} failed: {e}"))?;
        let mut reader = resp.into_reader();
        let mut file = std::fs::File::create(&dest)
            .with_context(|| format!("creating staging file {}", dest.display()))?;
        std::io::copy(&mut reader, &mut file)
            .with_context(|| format!("writing manifest body to {}", dest.display()))?;
        return Ok((dest, Some(td)));
    }
    Ok((PathBuf::from(url_or_path), None))
}

/// Fetch `url_or_path` straight into `dest` on disk. Used for sidecar
/// files (manifest.minisig, artefacts).
pub fn fetch_to(url_or_path: &str, dest: &Path) -> Result<()> {
    if let Some(rest) = url_or_path.strip_prefix("file://") {
        // Self-copy guard: when the dev fallback resolves the manifest
        // URL to its local path, the sidecar URL ends up pointing at the
        // same file we want to write. fs::copy truncates first, so a
        // self-copy would zero the file. Skip the copy when src == dest.
        let src = Path::new(rest);
        if src.canonicalize().ok() == dest.canonicalize().ok() && src.is_file() {
            return Ok(());
        }
        std::fs::copy(rest, dest)
            .with_context(|| format!("copying {rest} -> {}", dest.display()))?;
        return Ok(());
    }
    if url_or_path.starts_with("http://") || url_or_path.starts_with("https://") {
        let resp = ureq::get(url_or_path)
            .call()
            .map_err(|e| anyhow!("HTTP fetch of {url_or_path} failed: {e}"))?;
        let mut reader = resp.into_reader();
        let mut file =
            std::fs::File::create(dest).with_context(|| format!("creating {}", dest.display()))?;
        std::io::copy(&mut reader, &mut file)
            .with_context(|| format!("writing body to {}", dest.display()))?;
        return Ok(());
    }
    // Bare path.
    std::fs::copy(url_or_path, dest)
        .with_context(|| format!("copying {url_or_path} -> {}", dest.display()))?;
    Ok(())
}

/// Verify a detached minisign signature.
///
/// Shells out to `minisign -V -p <pubkey> -m <manifest> -x <sig>` so we
/// rely on the system minisign binary (same code path the doctor checks
/// for). This avoids us re-implementing the BLAKE2b/Ed25519 dance and
/// keeps the trust surface tiny.
pub fn verify_manifest_signature(
    manifest_path: &Path,
    sig_path: &Path,
    pubkey_path: &Path,
) -> Result<()> {
    if !pubkey_path.is_file() {
        bail!("pubkey not found at {}", pubkey_path.display());
    }
    if !sig_path.is_file() {
        bail!("signature not found at {}", sig_path.display());
    }
    if !manifest_path.is_file() {
        bail!("manifest not found at {}", manifest_path.display());
    }
    let status = std::process::Command::new("minisign")
        .arg("-V")
        .arg("-p")
        .arg(pubkey_path)
        .arg("-m")
        .arg(manifest_path)
        .arg("-x")
        .arg(sig_path)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .output()
        .map_err(|e| anyhow!("running minisign: {e} (is minisign installed? see `make doctor`)"))?;
    if !status.status.success() {
        let stderr = String::from_utf8_lossy(&status.stderr);
        bail!(
            "signature verification failed for {}: {}",
            manifest_path.display(),
            stderr.trim()
        );
    }
    Ok(())
}

/// Compute the lowercase hex-encoded SHA256 of a local file.
///
/// Uses the `sha2` crate (already an `deputyctl` dependency for the M8
/// age-key derivation) so this is portable to every host — including
/// Windows, where `sha256sum` (coreutils) is absent. The previous
/// implementation shelled out to `sha256sum`, which broke the launcher's
/// install/update path on Windows; this removes that latent bug.
pub fn sha256_hex(path: &Path) -> Result<String> {
    if !path.is_file() {
        bail!("artefact not found at {}", path.display());
    }
    let bytes =
        std::fs::read(path).with_context(|| format!("reading {} for sha256", path.display()))?;
    let mut hasher = Sha256::new();
    hasher.update(&bytes);
    Ok(hex_encode(&hasher.finalize()))
}

/// Verify the SHA256 of a local file matches a hex-encoded expected value.
///
/// Thin wrapper over [`sha256_hex`]; compare the computed hex to
/// `expected_hex` and bail with a clear mismatch message otherwise.
pub fn verify_sha256(path: &Path, expected_hex: &str) -> Result<()> {
    let actual = sha256_hex(path)?;
    if actual != expected_hex {
        bail!(
            "sha256 mismatch for {}: expected {expected_hex}, got {actual}",
            path.display()
        );
    }
    Ok(())
}

/// Lowercase hex-encode a byte slice (the SHA256 digest). Kept local so we
/// don't pull a `hex` crate dep for one call site.
fn hex_encode(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        out.push(HEX[(b >> 4) as usize] as char);
        out.push(HEX[(b & 0x0f) as usize] as char);
    }
    out
}

/// Resolve an artefact URL relative to the manifest's origin. Absolute
/// URLs (http(s):// or file://) pass through unchanged; bare paths are
/// joined onto the manifest's parent directory.
pub fn resolve_url(manifest_origin: &str, target: &str) -> String {
    if target.starts_with("http://")
        || target.starts_with("https://")
        || target.starts_with("file://")
    {
        return target.to_string();
    }
    // Strip the manifest filename component to get the parent.
    let parent = match manifest_origin.rsplit_once('/') {
        Some((p, _)) => p.to_string(),
        None => String::new(),
    };
    if parent.is_empty() {
        target.to_string()
    } else {
        format!("{parent}/{target}")
    }
}

/// Validate Y.M.D[-pre]. Used by [`load_manifest`] and tests.
pub fn is_valid_release_version(s: &str) -> bool {
    // Y.M.D core
    let (core, _suffix) = match s.split_once('-') {
        Some((c, suf)) if !suf.is_empty() => (c, Some(suf)),
        Some(_) => return false, // trailing dash with empty suffix
        None => (s, None),
    };
    let parts: Vec<&str> = core.split('.').collect();
    if parts.len() != 3 {
        return false;
    }
    if parts[0].len() != 4 || !parts[0].chars().all(|c| c.is_ascii_digit()) {
        return false;
    }
    if parts[1].is_empty() || parts[1].len() > 2 || !parts[1].chars().all(|c| c.is_ascii_digit()) {
        return false;
    }
    if parts[2].is_empty() || parts[2].len() > 2 || !parts[2].chars().all(|c| c.is_ascii_digit()) {
        return false;
    }
    true
}

/// Compare two CalVer release-version strings. Returns true if `candidate`
/// is strictly newer than `installed`. Falls back to lexicographic compare
/// for the suffix (which is good enough for CalVer-with-rcN).
pub fn is_newer(candidate: &str, installed: &str) -> bool {
    candidate != installed && lex_calver_cmp(candidate, installed).is_gt()
}

fn lex_calver_cmp(a: &str, b: &str) -> std::cmp::Ordering {
    // Split on '-' to get core + suffix. Compare core numerically first.
    let (a_core, a_suf) = a.split_once('-').unwrap_or((a, ""));
    let (b_core, b_suf) = b.split_once('-').unwrap_or((b, ""));

    let a_parts: Vec<u32> = a_core.split('.').map(|p| p.parse().unwrap_or(0)).collect();
    let b_parts: Vec<u32> = b_core.split('.').map(|p| p.parse().unwrap_or(0)).collect();

    let core_cmp = a_parts.cmp(&b_parts);
    if core_cmp != std::cmp::Ordering::Equal {
        return core_cmp;
    }
    // Pre-release ordering: 1.2.3 > 1.2.3-rc1 (no suffix wins).
    match (a_suf.is_empty(), b_suf.is_empty()) {
        (true, true) => std::cmp::Ordering::Equal,
        (true, false) => std::cmp::Ordering::Greater,
        (false, true) => std::cmp::Ordering::Less,
        (false, false) => a_suf.cmp(b_suf),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn synthetic_manifest_json() -> String {
        r#"{
          "schema_version": 1,
          "release_version": "2026.4.27",
          "channel": "dev",
          "released_at": "2026-04-27T00:00:00Z",
          "artefacts": [
            {
              "target": "qemu-aarch64",
              "profile": "openclaw",
              "filename": "deputyos-openclaw-qemu-aarch64-2026.4.27-dev.qcow2",
              "format": "qcow2",
              "size_bytes": 1048576,
              "sha256": "abcdef0123456789abcdef0123456789abcdef0123456789abcdef0123456789",
              "minisig_url": "2026.4.27/deputyos-openclaw-qemu-aarch64-2026.4.27-dev.qcow2.minisig",
              "url": "2026.4.27/deputyos-openclaw-qemu-aarch64-2026.4.27-dev.qcow2"
            }
          ]
        }"#
        .to_string()
    }

    #[test]
    fn manifest_roundtrip() {
        let json = synthetic_manifest_json();
        let m: Manifest = serde_json::from_str(&json).expect("parse");
        assert_eq!(m.schema_version, 1);
        assert_eq!(m.release_version, "2026.4.27");
        assert_eq!(m.artefacts.len(), 1);
        assert_eq!(m.artefacts[0].target, "qemu-aarch64");
        assert_eq!(m.artefacts[0].format, "qcow2");
        // Re-serialize and re-parse to confirm round-trip stability.
        let s = serde_json::to_string(&m).expect("serialize");
        let m2: Manifest = serde_json::from_str(&s).expect("reparse");
        assert_eq!(m, m2);
    }

    #[test]
    fn mounts_policy_schema_version_defaults_to_1_when_omitted() {
        // The synthetic manifest omits the field; it must default to the
        // current mounts-policy schema version (1), not error.
        let json = synthetic_manifest_json();
        let m: Manifest = serde_json::from_str(&json).expect("parse");
        assert_eq!(
            m.mounts_policy_schema_version,
            crate::mounts::POLICY_SCHEMA_VERSION
        );
        assert_eq!(m.mounts_policy_schema_version, 1);
    }

    #[test]
    fn mounts_policy_schema_version_parses_when_present() {
        let json = synthetic_manifest_json().replace(
            "\"channel\": \"dev\",",
            "\"channel\": \"dev\",\n          \"mounts_policy_schema_version\": 2,",
        );
        let m: Manifest = serde_json::from_str(&json).expect("parse");
        assert_eq!(m.mounts_policy_schema_version, 2);
    }

    #[test]
    fn rejects_schema_version_2() {
        let bad =
            synthetic_manifest_json().replace("\"schema_version\": 1", "\"schema_version\": 2");
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("manifest.json");
        std::fs::write(&path, bad).expect("write");
        let err = load_manifest(path.to_str().expect("utf8")).expect_err("expected error");
        let msg = format!("{err:#}");
        assert!(msg.contains("schema_version 2"), "got: {msg}");
    }

    #[test]
    fn rejects_malformed_release_version() {
        let bad = synthetic_manifest_json().replace(
            "\"release_version\": \"2026.4.27\"",
            "\"release_version\": \"v2026-04-27\"",
        );
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("manifest.json");
        std::fs::write(&path, bad).expect("write");
        let err = load_manifest(path.to_str().expect("utf8")).expect_err("expected error");
        let msg = format!("{err:#}");
        assert!(msg.contains("Y.M.D"), "got: {msg}");
    }

    #[test]
    fn rejects_empty_artefacts() {
        let bad =
            synthetic_manifest_json().replace("\"artefacts\": [", "\"artefacts\": [],\"_tail\": [");
        // Construct minimal valid JSON with empty artefacts array.
        let json = r#"{
          "schema_version": 1,
          "release_version": "2026.4.27",
          "channel": "dev",
          "released_at": "2026-04-27T00:00:00Z",
          "artefacts": []
        }"#;
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("manifest.json");
        std::fs::write(&path, json).expect("write");
        let err = load_manifest(path.to_str().expect("utf8")).expect_err("expected error");
        let msg = format!("{err:#}");
        assert!(msg.contains("no artefacts"), "got: {msg}");
        // Silence the unused `bad` linter.
        let _ = bad;
    }

    #[test]
    fn release_version_validator() {
        assert!(is_valid_release_version("2026.4.27"));
        assert!(is_valid_release_version("2026.04.27"));
        assert!(is_valid_release_version("2026.4.27-rc1"));
        assert!(!is_valid_release_version("2026"));
        assert!(!is_valid_release_version("2026.4"));
        assert!(!is_valid_release_version("v2026.4.27"));
        assert!(!is_valid_release_version("2026.4.27-"));
    }

    #[test]
    fn newer_compares_calver() {
        assert!(is_newer("2026.4.27", "2026.4.26"));
        assert!(is_newer("2026.4.27", "2026.4.27-rc1"));
        assert!(is_newer("2026.5.1", "2026.4.99"));
        assert!(!is_newer("2026.4.27", "2026.4.27"));
        assert!(!is_newer("2026.4.26", "2026.4.27"));
        assert!(is_newer("2026.4.27", "0.0.0"));
    }

    #[test]
    fn resolve_url_relative_to_manifest() {
        assert_eq!(
            resolve_url("file:///tmp/dist/manifest.json", "2026.4.27/foo.qcow2"),
            "file:///tmp/dist/2026.4.27/foo.qcow2"
        );
        assert_eq!(
            resolve_url("https://cdn/dev/manifest.json", "2026.4.27/foo.qcow2"),
            "https://cdn/dev/2026.4.27/foo.qcow2"
        );
        assert_eq!(
            resolve_url("file:///tmp/m.json", "https://elsewhere/foo"),
            "https://elsewhere/foo"
        );
    }

    /// Test signature verification end-to-end with a generated dev keypair.
    /// Skipped if minisign is not on PATH.
    #[test]
    fn signature_verify_roundtrip() {
        if std::process::Command::new("minisign")
            .arg("-h")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .is_err()
        {
            eprintln!("test: minisign not installed; skipping");
            return;
        }
        let dir = tempfile::tempdir().expect("tempdir");
        let pubkey = dir.path().join("k.pub");
        let seckey = dir.path().join("k.key");
        let msg = dir.path().join("manifest.json");
        let sig = dir.path().join("manifest.json.minisig");
        std::fs::File::create(&msg)
            .expect("create")
            .write_all(b"hello manifest\n")
            .expect("write");
        let g = std::process::Command::new("minisign")
            .args(["-G", "-W", "-p"])
            .arg(&pubkey)
            .arg("-s")
            .arg(&seckey)
            .status()
            .expect("genkey");
        assert!(g.success(), "keygen");
        let s = std::process::Command::new("minisign")
            .args(["-S", "-W", "-s"])
            .arg(&seckey)
            .arg("-m")
            .arg(&msg)
            .status()
            .expect("sign");
        assert!(s.success(), "sign");

        // Good path.
        verify_manifest_signature(&msg, &sig, &pubkey).expect("verify ok");

        // Bad path: corrupt the manifest.
        let mut f = std::fs::OpenOptions::new()
            .append(true)
            .open(&msg)
            .expect("open append");
        f.write_all(b"X").expect("corrupt");
        drop(f);
        let err = verify_manifest_signature(&msg, &sig, &pubkey).expect_err("expected error");
        assert!(
            format!("{err:#}").contains("signature verification failed"),
            "expected sig-fail, got: {err:#}"
        );
    }

    #[test]
    fn sha256_hex_known_vector_and_consistency() {
        // Known SHA256 of b"abc" (NIST FIPS 180-4 test vector).
        let dir = tempfile::tempdir().expect("tempdir");
        let p = dir.path().join("abc.bin");
        std::fs::write(&p, b"abc").expect("write");
        let hex = sha256_hex(&p).expect("hash");
        assert_eq!(
            hex,
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );

        // verify_sha256 must agree with sha256_hex on the same file, pass on
        // the computed hex, and fail on a mismatch with a clear message.
        verify_sha256(&p, &hex).expect("self-consistent");
        let err = verify_sha256(&p, "00").expect_err("mismatch");
        assert!(
            format!("{err:#}").contains("sha256 mismatch"),
            "got: {err:#}"
        );
    }

    #[test]
    fn sha256_hex_missing_file_errors() {
        let dir = tempfile::tempdir().expect("tempdir");
        let p = dir.path().join("nope.bin");
        let err = sha256_hex(&p).expect_err("missing");
        assert!(format!("{err:#}").contains("not found"), "got: {err:#}");
    }
}
