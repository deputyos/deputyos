//! TOML patcher for `[profile].pinned_version`.
//!
//! We do *not* round-trip the manifest through serde — that would lose
//! comments, alignment, and section ordering, which the profile authors
//! care about. Instead we do a targeted line edit: find the
//! `pinned_version` assignment in the `[profile]` block and replace just
//! the value, preserving inline comments and surrounding whitespace.

use anyhow::{Context, Result};

/// Result of a patch operation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Patch {
    pub original: String,
    pub patched: String,
    pub old_version: String,
    pub new_version: String,
}

/// Replace `pinned_version` in `text` with `new_version`. The first
/// `pinned_version = "..."` line under `[profile]` is rewritten; the rest
/// of the file is byte-identical.
pub fn bump_pinned_version(text: &str, new_version: &str) -> Result<Patch> {
    let mut in_profile = false;
    let mut out = String::with_capacity(text.len() + 16);
    let mut old_version: Option<String> = None;

    for line in text.split_inclusive('\n') {
        let trimmed = line.trim_start();
        if trimmed.starts_with('[') {
            // Section header — section name is everything up to ']'.
            let section = trimmed
                .trim_end()
                .trim_start_matches('[')
                .trim_end_matches(']')
                .trim();
            in_profile = section == "profile";
            out.push_str(line);
            continue;
        }

        if in_profile && old_version.is_none() {
            if let Some((old, replaced)) = try_replace_pinned(line, new_version) {
                old_version = Some(old);
                out.push_str(&replaced);
                continue;
            }
        }
        out.push_str(line);
    }

    let old_version =
        old_version.context("no pinned_version line found under [profile] section")?;
    Ok(Patch {
        original: text.to_string(),
        patched: out,
        old_version,
        new_version: new_version.to_string(),
    })
}

/// If `line` is a `pinned_version = "..."` assignment, return `(old, new_line)`.
fn try_replace_pinned(line: &str, new_version: &str) -> Option<(String, String)> {
    let key_idx = line.find("pinned_version")?;
    // Make sure what precedes the key is only whitespace — don't match
    // a key that is itself a substring of something else.
    if !line[..key_idx].chars().all(char::is_whitespace) {
        return None;
    }
    let after = &line[key_idx + "pinned_version".len()..];
    // Expect `<space>* = <space>* "..."`
    let after = after.trim_start();
    let after = after.strip_prefix('=')?.trim_start();
    let rest = after.strip_prefix('"')?;
    let close = rest.find('"')?;
    let old = rest[..close].to_string();
    let tail = &rest[close + 1..];
    // Reconstruct: keep everything up through the key, then the canonical
    // ` = "<new>"` form, then any trailing comment/whitespace.
    let prefix = &line[..key_idx];
    // Preserve original spacing between key and `=` to keep the file
    // visually aligned with sibling assignments. Walk from after the key.
    let between = &line[key_idx + "pinned_version".len()..];
    let eq_idx = between.find('=')?;
    let between_eq = &between[..=eq_idx];
    // After `=`, original spacing up to the opening quote.
    let post_eq = &between[eq_idx + 1..];
    let space_count = post_eq.chars().take_while(|c| *c == ' ').count();
    let spaces = " ".repeat(space_count.max(1));
    let new_line = format!("{prefix}pinned_version{between_eq}{spaces}\"{new_version}\"{tail}");
    Some((old, new_line))
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn replaces_pinned_version_preserving_comment() {
        let toml = "[profile]\nid = \"x\"\npinned_version  = \"1.2.3\"  # comment\n";
        let p = bump_pinned_version(toml, "1.2.4").unwrap();
        assert_eq!(p.old_version, "1.2.3");
        assert!(p.patched.contains("pinned_version  = \"1.2.4\"  # comment"));
        assert!(p.patched.starts_with("[profile]\nid = \"x\"\n"));
    }

    #[test]
    fn ignores_pinned_version_outside_profile_section() {
        let toml = "[other]\npinned_version = \"9.9.9\"\n[profile]\npinned_version = \"1.0.0\"\n";
        let p = bump_pinned_version(toml, "1.0.1").unwrap();
        assert_eq!(p.old_version, "1.0.0");
        // The decoy was untouched.
        assert!(p.patched.contains("[other]\npinned_version = \"9.9.9\""));
    }

    #[test]
    fn errors_when_missing() {
        let toml = "[profile]\nid = \"x\"\n";
        assert!(bump_pinned_version(toml, "1.0.0").is_err());
    }
}
