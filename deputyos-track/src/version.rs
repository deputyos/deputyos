//! Version comparison for `pinned_version` strings.
//!
//! The deputyOS pinned_version format follows two upstream conventions:
//!
//! * date-style: `YYYY.M.D` (OpenClaw — e.g. `2026.4.25`)
//! * semver-ish: `MAJOR.MINOR.PATCH` (Hermes — e.g. `0.11.0`)
//!
//! Both are parsed into a 3-tuple of `u64` plus an optional pre-release
//! suffix string. Comparison is lexicographic on the tuple, with the rule
//! that a present suffix sorts *before* the same tuple with no suffix
//! (`2026.4.25-beta1 < 2026.4.25`).
//!
//! Tag → version conversion strips a leading `v` (`v2026.4.27` →
//! `2026.4.27`), matching upstream tag conventions.

use std::cmp::Ordering;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Version {
    pub raw: String,
    pub parts: (u64, u64, u64),
    /// Pre-release suffix, e.g. "beta1" from "2026.4.25-beta1".
    /// `None` means a final release.
    pub suffix: Option<String>,
}

impl Version {
    /// Parse a pinned_version or upstream tag into a comparable [`Version`].
    ///
    /// Strips a leading `v` if present. Missing minor/patch components
    /// default to 0 (so `1.0` parses as `(1, 0, 0)`). Anything after the
    /// first `-` is preserved verbatim as the suffix.
    pub fn parse(s: &str) -> anyhow::Result<Self> {
        let raw = s.to_string();
        let trimmed = s.strip_prefix('v').unwrap_or(s);
        let (core, suffix) = match trimmed.split_once('-') {
            Some((c, s)) => (c, Some(s.to_string())),
            None => (trimmed, None),
        };
        let mut nums = core.split('.');
        let parse_part = |p: Option<&str>| -> anyhow::Result<u64> {
            match p {
                Some(s) => s
                    .parse::<u64>()
                    .map_err(|e| anyhow::anyhow!("version part {s:?}: {e}")),
                None => Ok(0),
            }
        };
        let major = parse_part(nums.next())?;
        let minor = parse_part(nums.next())?;
        let patch = parse_part(nums.next())?;
        Ok(Self {
            raw,
            parts: (major, minor, patch),
            suffix,
        })
    }
}

impl Ord for Version {
    fn cmp(&self, other: &Self) -> Ordering {
        match self.parts.cmp(&other.parts) {
            Ordering::Equal => match (&self.suffix, &other.suffix) {
                (Some(a), Some(b)) => a.cmp(b),
                // Suffix present sorts *before* the absence of one: a final
                // release is newer than its own beta.
                (Some(_), None) => Ordering::Less,
                (None, Some(_)) => Ordering::Greater,
                (None, None) => Ordering::Equal,
            },
            ord => ord,
        }
    }
}

impl PartialOrd for Version {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn day_lt_next_day() {
        assert!(Version::parse("2026.4.25").unwrap() < Version::parse("2026.4.26").unwrap());
    }

    #[test]
    fn day_lt_next_month() {
        assert!(Version::parse("2026.4.25").unwrap() < Version::parse("2026.5.1").unwrap());
    }

    #[test]
    fn day_lt_next_year() {
        assert!(Version::parse("2026.4.25").unwrap() < Version::parse("2027.1.1").unwrap());
    }

    #[test]
    fn beta_lt_final() {
        assert!(Version::parse("2026.4.25-beta1").unwrap() < Version::parse("2026.4.25").unwrap());
    }

    #[test]
    fn v_prefix_stripped() {
        let a = Version::parse("v2026.4.27").unwrap();
        let b = Version::parse("2026.4.27").unwrap();
        assert_eq!(a.parts, b.parts);
    }

    #[test]
    fn semver_hermes() {
        assert!(Version::parse("0.11.0").unwrap() < Version::parse("0.12.0").unwrap());
        assert!(Version::parse("0.11.0").unwrap() < Version::parse("1.0.0").unwrap());
        assert_eq!(
            Version::parse("0.11.0").unwrap(),
            Version::parse("0.11.0").unwrap()
        );
    }
}
