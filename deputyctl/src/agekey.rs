//! Deterministic age identity derivation from a backup recovery secret.
//!
//! age's passphrase mode (`-p`) prompts for the passphrase on a controlling
//! terminal and ignores a piped stdin — so it cannot run unattended under the
//! systemd timer that drives `deputyctl backup now`, nor under `deputyctl
//! restore --from cloud`. Piping the token to `age`'s stdin does not work; age
//! opens `/dev/tty` for the prompt and fails outright when none exists
//! (`could not read passphrase: standard input is not a terminal, and
//! /dev/tty is not available`).
//!
//! Instead we deterministically derive a 32-byte X25519 private key from the
//! stable recovery secret and format it as an age
//! identity (`AGE-SECRET-KEY-1...`). The same token regenerates the same
//! identity on any device, so a restore on device B decrypts a bundle device A
//! produced. Encryption targets the derived recipient
//! (`age1...`); decryption uses the derived identity (`age -d -i -`).
//!
//! We hash with the vetted `sha2` crate (a hand-rolled SHA-256 would still
//! round-trip — both sides derive the same key — but could silently lose
//! entropy). The bech32 encoding of the age secret key is hand-rolled below
//! using the canonical BIP-173 generator; it is fail-closed: `age-keygen -y`
//! validates the checksum, so any encoding bug surfaces as a rejected identity
//! rather than a different-but-usable key. The `agekey::tests` module pins this
//! by round-tripping a derived identity through `age-keygen -y` and by decoding
//! a real `age-keygen` identity to confirm age uses plain bech32 (residue 1),
//! not bech32m.

use anyhow::{bail, Context, Result};
use sha2::{Digest, Sha256};
use std::io::Write;
use std::process::{Command, Stdio};

/// Domain-separation prefix so this derivation never collides with any other
/// use of `SHA256(token)` (now or future).
const DOMAIN: &[u8] = b"deputyos-backup-age-key-v1:";

/// bech32 character set (BIP-173).
const CHARSET: &[u8] = b"qpzry9x8gf2tvdw0s3jn54khce6mua7l";

/// bech32 generator constants (BIP-173).
const GEN: [u32; 5] = [0x3b6a57b2, 0x26508e6d, 0x1ea119fa, 0x3d4233dd, 0x2a1462b3];

/// The plain-bech32 checksum target (BIP-173). age identities use plain
/// bech32, not bech32m (residue `0x2bc830a3`); see
/// `agekey::tests::age_identity_uses_plain_bech32`.
const BECH32_RESIDUE: u32 = 1;

/// HRP for age secret keys. age emits these UPPERCASE and is case-sensitive on
/// the identity, so the encoded string is uppercased before use. The checksum
/// is computed over the lowercase HRP per BIP-173.
const SECRET_KEY_HRP: &str = "age-secret-key-";

fn polymod(values: &[u32]) -> u32 {
    let mut chk: u32 = 1;
    for &v in values {
        let top = chk >> 25;
        chk = ((chk & 0x1ffffff) << 5) ^ v;
        for (i, &gen) in GEN.iter().enumerate() {
            if (top >> i) & 1 == 1 {
                chk ^= gen;
            }
        }
    }
    chk
}

fn hrp_expand(hrp: &[u8]) -> Vec<u32> {
    let mut v = Vec::with_capacity(hrp.len() * 2 + 1);
    for c in hrp {
        v.push((c >> 5) as u32);
    }
    v.push(0);
    for c in hrp {
        v.push((c & 31) as u32);
    }
    v
}

fn create_checksum(hrp: &[u8], data: &[u32]) -> [u32; 6] {
    let mut values = hrp_expand(hrp);
    values.extend_from_slice(data);
    values.extend([0u32; 6]);
    let pm = polymod(&values) ^ BECH32_RESIDUE;
    let mut out = [0u32; 6];
    for (i, slot) in out.iter_mut().enumerate() {
        *slot = (pm >> (5 * (5 - i))) & 31;
    }
    out
}

/// Convert a byte slice (8-bit groups) into 5-bit groups with zero padding —
/// the bech32 data encoding. 32 bytes → 52 five-bit groups.
fn convertbits_8to5(data: &[u8]) -> Vec<u32> {
    let mut acc: u32 = 0;
    let mut bits: u32 = 0;
    let mut ret = Vec::new();
    for &b in data {
        acc = (acc << 8) | (b as u32);
        bits += 8;
        while bits >= 5 {
            bits -= 5;
            ret.push((acc >> bits) & 31);
        }
    }
    if bits > 0 {
        ret.push((acc << (5 - bits)) & 31);
    }
    ret
}

/// bech32-encode `data` under `hrp` (lowercase) and return the UPPERCASE form
/// age's identity files use.
fn bech32_encode(hrp: &str, data: &[u8]) -> String {
    let hrp_b = hrp.as_bytes();
    let data5 = convertbits_8to5(data);
    let checksum = create_checksum(hrp_b, &data5);
    let mut s = String::with_capacity(hrp.len() + 1 + data5.len() + 6);
    s.push_str(hrp);
    s.push('1');
    for v in data5.iter().chain(checksum.iter()) {
        s.push(CHARSET[(*v as usize) & 31] as char);
    }
    s.to_uppercase()
}

/// Decode a bech32 string into `(lowercase-hrp, 5-bit data incl. checksum)` for
/// the variant-detection test. Returns `None` on a malformed string.
#[cfg(test)]
fn bech32_decode(s: &str) -> Option<(String, Vec<u32>)> {
    let lower = s.to_lowercase();
    let sep = lower.rfind('1')?;
    let hrp = lower[..sep].to_string();
    if hrp.is_empty() {
        return None;
    }
    let mut data5 = Vec::new();
    for ch in lower[sep + 1..].bytes() {
        let pos = CHARSET.iter().position(|&c| c == ch)? as u32;
        data5.push(pos);
    }
    Some((hrp, data5))
}

/// Derive the age identity (secret key, `AGE-SECRET-KEY-1...`) from a recovery
/// secret. This is the only secret the holder needs; the recipient (public key)
/// is recoverable from it via `age-keygen -y`.
pub(crate) fn derive_identity(token: &str) -> Result<String> {
    let mut h = Sha256::new();
    h.update(DOMAIN);
    h.update(token.as_bytes());
    let key: [u8; 32] = h.finalize().into();
    Ok(bech32_encode(SECRET_KEY_HRP, &key))
}

/// Derive the age recipient (public key, `age1...`) from the derived identity,
/// by asking `age-keygen -y` to convert it. Shells out to the same `age`
/// toolchain cloud backup already requires.
pub(crate) fn derive_recipient(identity: &str) -> Result<String> {
    let mut child = Command::new("age-keygen")
        .arg("-y")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .context(
            "spawning age-keygen — install `age` (e.g. `sudo apt install age`) for cloud backup",
        )?;
    if let Some(mut stdin) = child.stdin.take() {
        stdin.write_all(identity.as_bytes())?;
        stdin.write_all(b"\n")?;
    }
    let output = child.wait_with_output().context("waiting for age-keygen")?;
    if !output.status.success() {
        bail!(
            "age-keygen -y rejected the derived identity: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    let recipient = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if !recipient.starts_with("age1") {
        bail!("age-keygen -y produced an unexpected recipient: {recipient:?}");
    }
    Ok(recipient)
}

/// Derive both the age identity and its recipient from the backup token.
pub(crate) fn derive(token: &str) -> Result<(String, String)> {
    let identity = derive_identity(token)?;
    let recipient = derive_recipient(&identity)?;
    Ok((identity, recipient))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn age_present() -> bool {
        Command::new("age-keygen")
            .arg("-h")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_ok()
    }

    /// A real age identity is plain bech32 (residue 1), not bech32m. This pins
    /// `BECH32_RESIDUE` so a future change can't silently flip the variant.
    #[test]
    fn age_identity_uses_plain_bech32() {
        if !age_present() {
            eprintln!("test: age-keygen not installed; skipping");
            return;
        }
        let out = Command::new("age-keygen")
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .output()
            .expect("age-keygen");
        let stdout = String::from_utf8_lossy(&out.stdout);
        let id = stdout
            .lines()
            .find(|l| l.starts_with("AGE-SECRET-KEY-1"))
            .expect("identity line");
        let (hrp, data5) = bech32_decode(id).expect("decode real age identity");
        assert_eq!(hrp, SECRET_KEY_HRP);
        let pm = polymod(&[hrp_expand(hrp.as_bytes()), data5].concat());
        assert_eq!(pm, BECH32_RESIDUE, "age identities are plain bech32");
    }

    /// The derived identity must be a syntactically valid age secret key that
    /// `age-keygen -y` accepts (it validates the bech32 checksum), and the
    /// same token must always derive the same recipient.
    #[test]
    fn derived_identity_is_valid_and_deterministic() {
        if !age_present() {
            eprintln!("test: age-keygen not installed; skipping");
            return;
        }
        let (id1, rcpt1) = derive("token-A").expect("derive A");
        assert!(
            id1.starts_with("AGE-SECRET-KEY-1"),
            "identity header: {id1}"
        );
        assert!(rcpt1.starts_with("age1"), "recipient: {rcpt1}");

        // Determinism: same token → same identity + recipient.
        let (id2, rcpt2) = derive("token-A").expect("derive A again");
        assert_eq!(id1, id2);
        assert_eq!(rcpt1, rcpt2);

        // Different tokens → different recipients (no collision).
        let (_, rcpt_b) = derive("token-B").expect("derive B");
        assert_ne!(rcpt1, rcpt_b);
    }
}
