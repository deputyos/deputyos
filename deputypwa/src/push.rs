//! Web Push subscriptions + VAPID keypair handling.
//!
//! The PWA is the **registry** for browser push subscriptions; it does not
//! deliver pushes itself yet. Cost-alert hooks and future doctor-fail
//! notifications will call [`fire_push_notification`], which iterates the
//! registry and emits one HTTP request per subscription via the standard
//! Web Push protocol (RFC 8030 + VAPID, RFC 8292).
//!
//! VAPID keypair handling:
//!
//! * If `--vapid-keys-path` exists, we load the existing P-256 keypair (PEM
//!   form, as written by `openssl ecparam -genkey -name prime256v1`).
//! * Otherwise we attempt to generate one via `openssl`. If `openssl` isn't
//!   on PATH, we fall back to a "push disabled" mode and log a warning;
//!   subscription endpoints still respond, but the public-key route returns
//!   an empty placeholder so the browser can short-circuit cleanly.
//!
//! M3-PWA scope: registry + VAPID-keypair bootstrap. Actual outbound push
//! delivery (HTTP signing per RFC 8292) is wired by a future hook handler;
//! the contract for that handler is documented on
//! [`fire_push_notification`].

use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::paths;

/// Browser-supplied subscription payload. Mirrors the `PushSubscription`
/// JSON shape from `PushManager.subscribe()`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PushSubscription {
    pub endpoint: String,
    pub keys: SubscriptionKeys,
    /// Optional UA string from the browser, captured for operator visibility
    /// in the keys page; never used for delivery.
    #[serde(default)]
    pub user_agent: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubscriptionKeys {
    pub p256dh: String,
    pub auth: String,
}

/// Append-only persistence: each subscribe is one JSON line, owner-readable
/// only on Unix. We never read+rewrite under contention.
pub fn append_subscription(sub: &PushSubscription) -> Result<()> {
    let path = paths::push_subscriptions_path();
    append_subscription_to(&path, sub)
}

pub fn append_subscription_to(path: &Path, sub: &PushSubscription) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating {}", parent.display()))?;
    }
    let line = serde_json::to_string(sub).context("serialising subscription")?;
    let mut f = OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .with_context(|| format!("opening {}", path.display()))?;
    writeln!(f, "{line}").with_context(|| format!("writing {}", path.display()))?;
    drop(f);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Ok(meta) = std::fs::metadata(path) {
            let mut perms = meta.permissions();
            perms.set_mode(0o600);
            let _ = std::fs::set_permissions(path, perms);
        }
    }
    Ok(())
}

/// Read all persisted subscriptions. Malformed lines are skipped silently
/// so a single corrupt entry doesn't block delivery to all the others.
pub fn read_subscriptions() -> Vec<PushSubscription> {
    read_subscriptions_from(&paths::push_subscriptions_path())
}

pub fn read_subscriptions_from(path: &Path) -> Vec<PushSubscription> {
    let raw = match std::fs::read_to_string(path) {
        Ok(s) => s,
        Err(_) => return Vec::new(),
    };
    raw.lines()
        .filter(|l| !l.trim().is_empty())
        .filter_map(|l| serde_json::from_str::<PushSubscription>(l).ok())
        .collect()
}

/// VAPID keypair. PEM-encoded P-256 — produced by openssl, not by us.
#[derive(Debug, Clone)]
pub struct VapidKeypair {
    /// Public key in raw uncompressed form (65 bytes, prefix 0x04), then
    /// base64url-encoded for `applicationServerKey`.
    pub public_b64url: String,
    /// PEM private key, kept for the future delivery hook.
    pub private_pem: String,
}

/// Load a VAPID keypair from disk, or generate one via openssl if missing.
/// Returns `Ok(None)` if openssl isn't available (push-disabled mode).
pub fn load_or_generate_vapid(path: &Path) -> Result<Option<VapidKeypair>> {
    if path.exists() {
        return Ok(Some(load_vapid_from_pem(path)?));
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating {}", parent.display()))?;
    }
    if !openssl_available() {
        tracing::warn!(
            "openssl not on PATH; VAPID keypair not generated, Web Push will be disabled"
        );
        return Ok(None);
    }
    let out = std::process::Command::new("openssl")
        .args(["ecparam", "-genkey", "-name", "prime256v1", "-noout"])
        .output()
        .context("invoking openssl ecparam")?;
    if !out.status.success() {
        anyhow::bail!(
            "openssl ecparam failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    std::fs::write(path, &out.stdout).with_context(|| format!("writing {}", path.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Ok(meta) = std::fs::metadata(path) {
            let mut perms = meta.permissions();
            perms.set_mode(0o600);
            let _ = std::fs::set_permissions(path, perms);
        }
    }
    Ok(Some(load_vapid_from_pem(path)?))
}

fn load_vapid_from_pem(path: &Path) -> Result<VapidKeypair> {
    let pem = std::fs::read_to_string(path)
        .with_context(|| format!("reading vapid pem at {}", path.display()))?;
    // Derive the public key via `openssl ec -pubout` so we don't pull in a
    // P-256 crate just to print one number.
    let out = std::process::Command::new("openssl")
        .args(["ec", "-pubout"])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .context("spawning openssl ec -pubout")?;
    let mut child = out;
    if let Some(mut stdin) = child.stdin.take() {
        stdin
            .write_all(pem.as_bytes())
            .context("piping pem to openssl ec")?;
    }
    let result = child
        .wait_with_output()
        .context("waiting on openssl ec -pubout")?;
    if !result.status.success() {
        anyhow::bail!(
            "openssl ec -pubout failed: {}",
            String::from_utf8_lossy(&result.stderr).trim()
        );
    }
    // Convert the SubjectPublicKeyInfo PEM to raw bytes via openssl.
    let raw = std::process::Command::new("openssl")
        .args(["pkey", "-pubin", "-outform", "DER", "-out", "/dev/stdout"])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn();
    let public_b64url = match raw {
        Ok(mut child) => {
            if let Some(mut stdin) = child.stdin.take() {
                let _ = stdin.write_all(&result.stdout);
            }
            let output = child
                .wait_with_output()
                .context("waiting on openssl pkey")?;
            if output.status.success() {
                // Last 65 bytes of the DER are the uncompressed point; we
                // approximate by trusting the structure (openssl outputs
                // SubjectPublicKeyInfo with the point at the tail).
                let bytes = output.stdout;
                let take = bytes.len().saturating_sub(65);
                base64url(&bytes[take..])
            } else {
                String::new()
            }
        }
        Err(_) => String::new(),
    };

    Ok(VapidKeypair {
        public_b64url,
        private_pem: pem,
    })
}

fn openssl_available() -> bool {
    std::process::Command::new("openssl")
        .arg("version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// RFC 4648 §5 base64url without padding. Used for the
/// `applicationServerKey` value the browser passes to `PushManager.subscribe`.
pub fn base64url(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut out = String::with_capacity((bytes.len() * 4).div_ceil(3));
    let mut i = 0;
    while i + 3 <= bytes.len() {
        let n = ((bytes[i] as u32) << 16) | ((bytes[i + 1] as u32) << 8) | (bytes[i + 2] as u32);
        out.push(ALPHABET[((n >> 18) & 0x3f) as usize] as char);
        out.push(ALPHABET[((n >> 12) & 0x3f) as usize] as char);
        out.push(ALPHABET[((n >> 6) & 0x3f) as usize] as char);
        out.push(ALPHABET[(n & 0x3f) as usize] as char);
        i += 3;
    }
    let remaining = bytes.len() - i;
    if remaining == 1 {
        let n = (bytes[i] as u32) << 16;
        out.push(ALPHABET[((n >> 18) & 0x3f) as usize] as char);
        out.push(ALPHABET[((n >> 12) & 0x3f) as usize] as char);
    } else if remaining == 2 {
        let n = ((bytes[i] as u32) << 16) | ((bytes[i + 1] as u32) << 8);
        out.push(ALPHABET[((n >> 18) & 0x3f) as usize] as char);
        out.push(ALPHABET[((n >> 12) & 0x3f) as usize] as char);
        out.push(ALPHABET[((n >> 6) & 0x3f) as usize] as char);
    }
    out
}

/// Public hook for cost-alert handlers. Iterates the on-disk registry and
/// (in M3-PWA) only logs; future delivery patch will sign + POST per RFC
/// 8030. Called by `deputyctl` (via the hooks system) when a cost cap trips.
///
/// Contract:
/// * Best-effort. Failures are logged at debug, never propagated.
/// * `title`/`body` are not sanitised here — caller supplies trusted text.
/// * Safe to call from any thread.
pub fn fire_push_notification(title: &str, body: &str) {
    let subs = read_subscriptions();
    if subs.is_empty() {
        tracing::debug!(title, "no push subscribers; nothing to fire");
        return;
    }
    tracing::info!(
        subscribers = subs.len(),
        title,
        body,
        "would deliver web-push (M3-PWA: log-only; delivery wired by future hook)"
    );
}

/// Default location for the VAPID PEM under [`paths::data_dir`].
pub fn default_vapid_path() -> PathBuf {
    paths::data_dir().join("vapid.pem")
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn append_then_read_round_trips() {
        let dir = TempDir::new().expect("tempdir");
        let path = dir.path().join("subs.jsonl");
        let sub = PushSubscription {
            endpoint: "https://example.invalid/push/abc".into(),
            keys: SubscriptionKeys {
                p256dh: "BPP".into(),
                auth: "AAA".into(),
            },
            user_agent: Some("test".into()),
        };
        append_subscription_to(&path, &sub).expect("append");
        append_subscription_to(&path, &sub).expect("append twice");
        let read = read_subscriptions_from(&path);
        assert_eq!(read.len(), 2);
        assert_eq!(read[0].endpoint, sub.endpoint);
    }

    #[test]
    fn base64url_known_vector() {
        // Compare against a known RFC 4648 §5 fixture: input "foobar" →
        // "Zm9vYmFy" in standard b64; url alphabet is identical for ASCII.
        assert_eq!(base64url(b"foobar"), "Zm9vYmFy");
        assert_eq!(base64url(b"foo"), "Zm9v");
        assert_eq!(base64url(b"f"), "Zg");
        assert_eq!(base64url(b""), "");
    }

    #[test]
    fn read_skips_malformed_lines() {
        let dir = TempDir::new().expect("tempdir");
        let path = dir.path().join("subs.jsonl");
        std::fs::write(
            &path,
            "{\"endpoint\":\"https://x\",\"keys\":{\"p256dh\":\"A\",\"auth\":\"B\"}}\n\
             not json\n\
             {\"endpoint\":\"https://y\",\"keys\":{\"p256dh\":\"C\",\"auth\":\"D\"}}\n",
        )
        .expect("write");
        let subs = read_subscriptions_from(&path);
        assert_eq!(subs.len(), 2);
    }

    #[test]
    fn fire_push_with_no_subscribers_is_safe() {
        std::env::set_var("DEPUTYPWA_DATA_DIR", "/tmp/deputypwa-empty-test");
        let _ = std::fs::remove_file(paths::push_subscriptions_path());
        // Should not panic.
        fire_push_notification("hi", "hello");
        std::env::remove_var("DEPUTYPWA_DATA_DIR");
    }
}
