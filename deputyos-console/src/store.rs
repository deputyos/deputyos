//! Token persistence for the console.
//!
//! [`ApiClient`](crate::api_client::ApiClient) is pure HTTP and never sees
//! persistence; this module owns storing the account's access/refresh tokens
//! between sessions. Two backends:
//!
//! - [`FileTokenStore`] — always available. A 0600 JSON file under the data
//!   dir. Robust, no system secret-service dependency; the default for the
//!   testable core and a fine v1 choice for dev.
//! - [`KeyringStore`] — behind the `gui` feature. Uses the OS keychain via
//!   `keyring` (Secret Service on Linux, Keychain on macOS, Credential
//!   Manager on Windows). The GUI build prefers this.
//!
//! Both implement [`TokenStore`], so the command layer is backend-agnostic.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::api_client::TokenPair;

/// The persisted credentials. Mirrors [`TokenPair`] plus a stored-at
/// timestamp so the command layer can decide when to refresh.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoredTokens {
    pub access_token: String,
    pub refresh_token: String,
    pub expires_in: u64,
    #[serde(default)]
    pub account_id: Option<String>,
    /// Unix seconds when the tokens were last written. The access token is
    /// valid for `stored_at_unix + expires_in`.
    pub stored_at_unix: u64,
}

impl StoredTokens {
    pub fn from_pair(p: TokenPair) -> Self {
        Self {
            access_token: p.access_token,
            refresh_token: p.refresh_token,
            expires_in: p.expires_in,
            account_id: p.account_id,
            stored_at_unix: now_unix(),
        }
    }

    /// Whether the access token is within `skew` seconds of expiry (or past it).
    pub fn expiring(&self, now_unix: u64, skew_secs: u64) -> bool {
        self.stored_at_unix.saturating_add(self.expires_in) <= now_unix.saturating_add(skew_secs)
    }
}

/// Backend-agnostic token storage.
pub trait TokenStore {
    fn load(&self) -> Result<Option<StoredTokens>>;
    fn save(&self, tokens: &StoredTokens) -> Result<()>;
    fn clear(&self) -> Result<()>;
}

/// Default file location for the console's tokens: the platform data dir
/// (env-overridable via `DEPUTYOS_CONSOLE_TOKEN_FILE`). Mirrors the
/// `deputyos-desktop` env-overridable path convention.
pub fn default_token_store_path() -> PathBuf {
    if let Ok(p) = std::env::var("DEPUTYOS_CONSOLE_TOKEN_FILE") {
        return PathBuf::from(p);
    }
    let base = dirs::data_local_dir().unwrap_or_else(|| PathBuf::from("."));
    base.join("deputyos-console").join("tokens.json")
}

/// JSON-file token store. Writes are atomic (temp + rename) with 0600 perms
/// on Unix so the file isn't world-readable.
#[derive(Debug, Clone)]
pub struct FileTokenStore {
    path: PathBuf,
}

impl FileTokenStore {
    pub fn new(path: PathBuf) -> Self {
        Self { path }
    }

    /// A store at [`default_token_store_path()`].
    pub fn default_store() -> Self {
        Self::new(default_token_store_path())
    }
}

impl TokenStore for FileTokenStore {
    fn load(&self) -> Result<Option<StoredTokens>> {
        match std::fs::read(&self.path) {
            Ok(bytes) => {
                let t: StoredTokens = serde_json::from_slice(&bytes)
                    .with_context(|| format!("parsing token file {}", self.path.display()))?;
                Ok(Some(t))
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(anyhow::anyhow!("reading {}: {e}", self.path.display())),
        }
    }

    fn save(&self, tokens: &StoredTokens) -> Result<()> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("creating token dir {}", parent.display()))?;
        }
        let tmp = self.path.with_extension("json.tmp");
        let bytes = serde_json::to_vec_pretty(tokens).context("serializing tokens")?;
        std::fs::write(&tmp, bytes).with_context(|| format!("writing {}", tmp.display()))?;
        restrict_perms(&tmp);
        std::fs::rename(&tmp, &self.path)
            .with_context(|| format!("renaming {} -> {}", tmp.display(), self.path.display()))?;
        // Ensure the final file is also locked down (rename preserves mode on
        // Unix, but be explicit for the freshly-created case).
        restrict_perms(&self.path);
        Ok(())
    }

    fn clear(&self) -> Result<()> {
        match std::fs::remove_file(&self.path) {
            Ok(_) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(anyhow::anyhow!("removing {}: {e}", self.path.display())),
        }
    }
}

/// Set 0600 on Unix; no-op elsewhere (Windows/macOS ACLs are the FS default).
#[cfg(unix)]
fn restrict_perms(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
}

#[cfg(not(unix))]
fn restrict_perms(_path: &Path) {}

/// OS-keychain token store (Linux Secret Service / macOS Keychain / Windows
/// Credential Manager). Only compiled with the `gui` feature.
#[cfg(feature = "gui")]
pub mod keyring_store {
    use super::{Result, StoredTokens, TokenStore};
    use anyhow::Context;

    /// Stores the tokens as a JSON blob under a fixed keyring entry.
    pub struct KeyringStore {
        service: String,
        user: String,
    }

    impl KeyringStore {
        pub fn new(service: &str, user: &str) -> Self {
            Self {
                service: service.to_string(),
                user: user.to_string(),
            }
        }

        /// Default entry: service `deputyos-console`, user `default`.
        pub fn default_store() -> Self {
            Self::new("deputyos-console", "default")
        }

        /// Open the underlying keyring entry. `pub(crate)` so `commands.rs`
        /// can probe whether the OS keychain is usable before choosing the
        /// keyring store over the file fallback in `GuiState::new`.
        pub(crate) fn entry(&self) -> Result<keyring::Entry> {
            keyring::Entry::new(&self.service, &self.user).context("opening keyring entry")
        }
    }

    impl TokenStore for KeyringStore {
        fn load(&self) -> Result<Option<StoredTokens>> {
            let entry = self.entry()?;
            match entry.get_password() {
                Ok(s) => {
                    let t: StoredTokens =
                        serde_json::from_str(&s).context("parsing keyring tokens")?;
                    Ok(Some(t))
                }
                Err(keyring::Error::NoEntry) => Ok(None),
                Err(e) => Err(anyhow::anyhow!("keyring load: {e}")),
            }
        }

        fn save(&self, tokens: &StoredTokens) -> Result<()> {
            let entry = self.entry()?;
            let s = serde_json::to_string(tokens).context("serializing tokens for keyring")?;
            entry.set_password(&s).context("writing keyring entry")
        }

        fn clear(&self) -> Result<()> {
            let entry = self.entry()?;
            match entry.delete_credential() {
                Ok(_) => Ok(()),
                Err(keyring::Error::NoEntry) => Ok(()),
                Err(e) => Err(anyhow::anyhow!("keyring clear: {e}")),
            }
        }
    }
}

#[cfg(feature = "gui")]
pub use keyring_store::KeyringStore;

fn now_unix() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api_client::TokenPair;

    #[test]
    fn file_store_round_trips() {
        let dir = tempfile::tempdir().expect("tempdir");
        let store = FileTokenStore::new(dir.path().join("tokens.json"));
        assert!(store.load().expect("empty").is_none());

        let pair = TokenPair {
            access_token: "at".into(),
            refresh_token: "rt".into(),
            expires_in: 900,
            account_id: Some("acct-1".into()),
        };
        let stored = StoredTokens::from_pair(pair);
        store.save(&stored).expect("save");

        let back = store.load().expect("load").expect("present");
        assert_eq!(back.access_token, "at");
        assert_eq!(back.refresh_token, "rt");
        assert_eq!(back.account_id.as_deref(), Some("acct-1"));
        assert_eq!(back.expires_in, 900);

        store.clear().expect("clear");
        assert!(store.load().expect("cleared").is_none());
    }

    #[test]
    fn file_store_clear_is_idempotent() {
        let dir = tempfile::tempdir().expect("tempdir");
        let store = FileTokenStore::new(dir.path().join("tokens.json"));
        // No file yet — clear must not error.
        store.clear().expect("clear on missing is fine");
    }

    #[test]
    #[cfg(unix)]
    fn file_store_is_created_0600() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("tokens.json");
        let store = FileTokenStore::new(path.clone());
        let stored = StoredTokens::from_pair(TokenPair {
            access_token: "at".into(),
            refresh_token: "rt".into(),
            expires_in: 900,
            account_id: None,
        });
        store.save(&stored).expect("save");
        let mode = std::fs::metadata(&path).expect("stat").permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "token file must be 0600, got {mode:o}");
    }

    #[test]
    fn expiring_detects_skew_window() {
        let t = StoredTokens {
            access_token: "at".into(),
            refresh_token: "rt".into(),
            expires_in: 900,
            account_id: None,
            stored_at_unix: 1000,
        };
        // stored_at=1000, expires_in=900 → access token valid until t=1900.
        // expiring(now, skew) is true once now + skew >= 1900.
        assert!(!t.expiring(1800, 60)); // 1860 < 1900 → fresh
        assert!(!t.expiring(1839, 60)); // 1899 < 1900 → fresh
        assert!(t.expiring(1840, 60)); // 1900 <= 1900 → expiring (boundary)
        assert!(t.expiring(2000, 60)); // past expiry
    }

    #[test]
    fn default_token_store_path_honors_env() {
        let prev = std::env::var("DEPUTYOS_CONSOLE_TOKEN_FILE").ok();
        std::env::set_var("DEPUTYOS_CONSOLE_TOKEN_FILE", "/tmp/explicit-tokens.json");
        assert_eq!(
            default_token_store_path(),
            PathBuf::from("/tmp/explicit-tokens.json")
        );
        match prev {
            Some(v) => std::env::set_var("DEPUTYOS_CONSOLE_TOKEN_FILE", v),
            None => std::env::remove_var("DEPUTYOS_CONSOLE_TOKEN_FILE"),
        }
    }
}
