//! Auth: single-use token → session cookie, per docs/09-security.md §wizard.
//!
//! - The server is launched with a token (`--token <hex>` or auto-generated).
//! - First request must present the token (query param `?token=<hex>` or
//!   `Authorization: Bearer <hex>`). On match, the token is consumed and a
//!   session cookie is set.
//! - Subsequent requests carry the session cookie.
//! - Session cookie expires in 1h.
//! - In dev mode (`--no-token`), all requests are accepted without auth and
//!   `Secure` is dropped from the cookie attributes.
//! - `AccountOwner` mode (remote management via tunnel): the request presents
//!   the account owner's JWT (same `?token=`/Bearer channel). The wizard
//!   validates the JWT signature against the API's RSA256 public key embedded
//!   in the image (`/etc/deputyos/api-pubkey.pem`) and checks `jwt.sub` equals
//!   this device's account id (read from `/etc/deputyos/account.json`). On match
//!   a session cookie is issued; the JWT is *not* consumed (it is short-lived
//!   and stateless), so a fresh request without the cookie re-presents it and
//!   re-mints a session. This is the load-bearing fix for remote wizard access:
//!   the in-VM launch token never leaves the appliance, so the console's JWT is
//!   the credential that lets the owner tunnel in.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use rand::RngCore;

/// Cookie name. `__Host-` prefix forbids `Domain=` and forces `Secure` +
/// `Path=/`, but the prefix itself requires Secure, so we skip the prefix in
/// dev mode where Secure is absent.
pub const COOKIE_NAME_PROD: &str = "__Host-deputyos-session";
pub const COOKIE_NAME_DEV: &str = "deputyos-session";

const SESSION_TTL: Duration = Duration::from_secs(60 * 60); // 1h per docs/09

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthMode {
    /// Production: single-use token, session cookie, secure flag.
    Token,
    /// Dev: no auth (used by `make wizard`).
    None,
    /// Remote management via tunnel: validate the account owner's JWT against
    /// the embedded API RSA256 pubkey + match `sub` to this device's account.
    AccountOwner,
}

/// Config for [`AuthMode::AccountOwner`]: the API public key (PEM bytes) and
/// the account id this device belongs to. Stored once at launch from
/// `/etc/deputyos/{api-pubkey.pem,account.json}`.
#[derive(Debug, Clone)]
pub struct AccountOwnerConfig {
    pub pubkey_pem: Vec<u8>,
    pub account_id: String,
}

#[derive(Debug)]
struct Inner {
    mode: AuthMode,
    /// The launch token. Consumed once: cleared to `None` after first
    /// successful exchange.
    token: Option<String>,
    /// Active session cookies (cookie value → expiry).
    sessions: Vec<(String, Instant)>,
    /// Present only in [`AuthMode::AccountOwner`].
    owner: Option<AccountOwnerConfig>,
}

#[derive(Debug, Clone)]
pub struct AuthState {
    inner: Arc<Mutex<Inner>>,
}

impl AuthState {
    pub fn new(mode: AuthMode, token: Option<String>) -> Self {
        Self {
            inner: Arc::new(Mutex::new(Inner {
                mode,
                token,
                sessions: Vec::new(),
                owner: None,
            })),
        }
    }

    /// Construct an `AccountOwner`-mode state. `pubkey_pem` is the API's RSA
    /// public key (PEM); `account_id` is this device's owner account id.
    pub fn new_account_owner(pubkey_pem: Vec<u8>, account_id: String) -> Self {
        Self {
            inner: Arc::new(Mutex::new(Inner {
                mode: AuthMode::AccountOwner,
                token: None,
                sessions: Vec::new(),
                owner: Some(AccountOwnerConfig {
                    pubkey_pem,
                    account_id,
                }),
            })),
        }
    }

    pub fn mode(&self) -> AuthMode {
        self.inner.lock().expect("auth lock poisoned").mode
    }

    /// True if a request bearing this `token_param` (query/header) and
    /// `cookie_value` (parsed `deputyos-session=...`) is authorized.
    /// Returns `Some(set_cookie_value)` if a new session must be issued.
    pub fn check_or_issue(
        &self,
        token_param: Option<&str>,
        cookie_value: Option<&str>,
    ) -> AuthOutcome {
        let mut g = self.inner.lock().expect("auth lock poisoned");
        if g.mode == AuthMode::None {
            return AuthOutcome::Authorized { issue: None };
        }

        // Expire stale sessions.
        let now = Instant::now();
        g.sessions.retain(|(_, exp)| *exp > now);

        // Existing session?
        if let Some(c) = cookie_value {
            if g.sessions.iter().any(|(v, _)| v == c) {
                return AuthOutcome::Authorized { issue: None };
            }
        }

        // AccountOwner: validate the JWT signature + owner match. The JWT is
        // not consumed (stateless, short-lived) — a cookieless request can
        // re-present it to re-mint a session.
        if g.mode == AuthMode::AccountOwner {
            if let (Some(jwt), Some(cfg)) = (token_param, g.owner.as_ref()) {
                if validate_owner_jwt(jwt, &cfg.pubkey_pem, &cfg.account_id) {
                    let session = random_hex(32);
                    g.sessions.push((session.clone(), now + SESSION_TTL));
                    return AuthOutcome::Authorized {
                        issue: Some(session),
                    };
                }
            }
            return AuthOutcome::Unauthorized;
        }

        // Token exchange?
        if let Some(provided) = token_param {
            if let Some(expected) = g.token.as_deref() {
                if constant_time_eq(provided.as_bytes(), expected.as_bytes()) {
                    // Single-use: clear the token and mint a session.
                    g.token = None;
                    let session = random_hex(32);
                    g.sessions.push((session.clone(), now + SESSION_TTL));
                    return AuthOutcome::Authorized {
                        issue: Some(session),
                    };
                }
            }
        }

        AuthOutcome::Unauthorized
    }
}

/// Validate an account owner JWT: RS256 signature against `pubkey_pem` (PEM),
/// `exp` honoured by jsonwebtoken's default `Validation`, and `sub` equal to
/// `expected_account_id`. Any decode failure → `false` (no panic, no leak of
/// *which* check failed).
fn validate_owner_jwt(token: &str, pubkey_pem: &[u8], expected_account_id: &str) -> bool {
    use jsonwebtoken::{decode, Algorithm, DecodingKey, Validation};
    #[derive(serde::Deserialize)]
    #[allow(dead_code)] // `exp` is deserialised so jsonwebtoken can validate it; we only read `sub`.
    struct Claims {
        sub: String,
        exp: usize,
    }
    let Ok(decoding_key) = DecodingKey::from_rsa_pem(pubkey_pem) else {
        return false;
    };
    // Same validation the sibling API uses (no aud; exp checked by default).
    let validation = Validation::new(Algorithm::RS256);
    match decode::<Claims>(token, &decoding_key, &validation) {
        Ok(data) => data.claims.sub == expected_account_id,
        Err(_) => false,
    }
}

#[derive(Debug, Clone)]
pub enum AuthOutcome {
    Authorized { issue: Option<String> },
    Unauthorized,
}

fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff: u8 = 0;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

pub fn random_hex(bytes: usize) -> String {
    let mut buf = vec![0u8; bytes];
    rand::thread_rng().fill_bytes(&mut buf);
    let mut s = String::with_capacity(bytes * 2);
    for b in buf {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

/// Parse a Cookie header for the session cookie value.
pub fn extract_session_cookie(cookie_header: Option<&str>, name: &str) -> Option<String> {
    let header = cookie_header?;
    for part in header.split(';') {
        let p = part.trim();
        if let Some(v) = p.strip_prefix(&format!("{name}=")) {
            return Some(v.to_string());
        }
    }
    None
}

/// Build a Set-Cookie header value for a freshly minted session.
pub fn set_cookie_header(name: &str, value: &str, secure: bool) -> String {
    let secure_part = if secure { "; Secure" } else { "" };
    // Max-Age in seconds matches SESSION_TTL.
    format!(
        "{name}={value}; Path=/; HttpOnly; SameSite=Strict; Max-Age={ttl}{secure_part}",
        ttl = SESSION_TTL.as_secs()
    )
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn dev_mode_authorizes_everything() {
        let a = AuthState::new(AuthMode::None, None);
        let r = a.check_or_issue(None, None);
        assert!(matches!(r, AuthOutcome::Authorized { issue: None }));
    }

    #[test]
    fn token_mode_rejects_no_token() {
        let a = AuthState::new(AuthMode::Token, Some("abc123".into()));
        let r = a.check_or_issue(None, None);
        assert!(matches!(r, AuthOutcome::Unauthorized));
    }

    #[test]
    fn token_mode_rejects_wrong_token() {
        let a = AuthState::new(AuthMode::Token, Some("abc123".into()));
        let r = a.check_or_issue(Some("nope"), None);
        assert!(matches!(r, AuthOutcome::Unauthorized));
    }

    #[test]
    fn token_mode_issues_session_then_token_is_consumed() {
        let a = AuthState::new(AuthMode::Token, Some("abc123".into()));
        let r = a.check_or_issue(Some("abc123"), None);
        let session = match r {
            AuthOutcome::Authorized { issue: Some(s) } => s,
            _ => panic!("expected new session"),
        };
        // Second use of the same token must fail.
        let r2 = a.check_or_issue(Some("abc123"), None);
        assert!(matches!(r2, AuthOutcome::Unauthorized));
        // But the issued session keeps working.
        let r3 = a.check_or_issue(None, Some(&session));
        assert!(matches!(r3, AuthOutcome::Authorized { issue: None }));
    }

    // ---- AccountOwner mode ----

    use jsonwebtoken::{encode, Algorithm, EncodingKey, Header};
    use rand::rngs::OsRng;
    use rsa::pkcs8::{EncodePrivateKey, EncodePublicKey};
    use rsa::RsaPrivateKey;

    /// Generate a throwaway RSA keypair (for tests only) + mint an RS256 JWT
    /// for `account_id` expiring in 15 minutes.
    fn owner_keypair_and_jwt(account_id: &str) -> (Vec<u8>, String) {
        let mut rng = OsRng;
        let priv_key = RsaPrivateKey::new(&mut rng, 2048).expect("rsa keygen");
        let priv_pem = priv_key
            .to_pkcs8_pem(rsa::pkcs8::LineEnding::LF)
            .expect("priv pem")
            .as_bytes()
            .to_vec();
        let pub_pem = priv_key
            .to_public_key()
            .to_public_key_pem(rsa::pkcs8::LineEnding::LF)
            .expect("pub pem")
            .as_bytes()
            .to_vec();

        #[derive(serde::Serialize)]
        struct C {
            sub: String,
            exp: usize,
            iat: usize,
            jti: String,
        }
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() as usize;
        let claims = C {
            sub: account_id.to_string(),
            exp: now + 900,
            iat: now,
            jti: "test".to_string(),
        };
        let jwt = encode(
            &Header::new(Algorithm::RS256),
            &claims,
            &EncodingKey::from_rsa_pem(&priv_pem).expect("encoding key"),
        )
        .expect("jwt encode");
        (pub_pem, jwt)
    }

    #[test]
    fn account_owner_accepts_valid_jwt_and_issues_session() {
        let (pub_pem, jwt) = owner_keypair_and_jwt("acct-42");
        let a = AuthState::new_account_owner(pub_pem, "acct-42".into());
        let r = a.check_or_issue(Some(&jwt), None);
        let session = match r {
            AuthOutcome::Authorized { issue: Some(s) } => s,
            other => panic!("expected new session, got {other:?}"),
        };
        // Session cookie works without re-presenting the JWT.
        let r2 = a.check_or_issue(None, Some(&session));
        assert!(matches!(r2, AuthOutcome::Authorized { issue: None }));
    }

    #[test]
    fn account_owner_rejects_wrong_account_id() {
        let (pub_pem, jwt) = owner_keypair_and_jwt("acct-42");
        // This device belongs to a different account.
        let a = AuthState::new_account_owner(pub_pem, "acct-99".into());
        let r = a.check_or_issue(Some(&jwt), None);
        assert!(matches!(r, AuthOutcome::Unauthorized));
    }

    #[test]
    fn account_owner_rejects_jwt_signed_by_other_key() {
        let (_pub_pem_other, jwt_other) = owner_keypair_and_jwt("acct-42");
        let (pub_pem, _jwt) = owner_keypair_and_jwt("acct-42");
        // pubkey_pem is the wizard's trusted key; jwt_other is signed by a
        // different key → signature fails.
        let a = AuthState::new_account_owner(pub_pem, "acct-42".into());
        let r = a.check_or_issue(Some(&jwt_other), None);
        assert!(matches!(r, AuthOutcome::Unauthorized));
    }

    #[test]
    fn account_owner_rejects_missing_credential() {
        let (pub_pem, _jwt) = owner_keypair_and_jwt("acct-42");
        let a = AuthState::new_account_owner(pub_pem, "acct-42".into());
        assert!(matches!(
            a.check_or_issue(None, None),
            AuthOutcome::Unauthorized
        ));
    }

    #[test]
    fn account_owner_rejects_garbage_token() {
        let (pub_pem, _jwt) = owner_keypair_and_jwt("acct-42");
        let a = AuthState::new_account_owner(pub_pem, "acct-42".into());
        assert!(matches!(
            a.check_or_issue(Some("not.a.jwt"), None),
            AuthOutcome::Unauthorized
        ));
    }

    #[test]
    fn extract_session_cookie_picks_named_value() {
        let header = "foo=bar; deputyos-session=deadbeef; baz=quux";
        let v = extract_session_cookie(Some(header), "deputyos-session");
        assert_eq!(v.as_deref(), Some("deadbeef"));
    }

    #[test]
    fn random_hex_is_correct_length() {
        let h = random_hex(16);
        assert_eq!(h.len(), 32);
        assert!(h.chars().all(|c| c.is_ascii_hexdigit()));
    }
}
