//! Profile manifest validator (CI-callable).
//!
//! `deputyctl profile validate <path>...` deserializes each path through
//! [`crate::manifest`] and then enforces the **semantic invariants** the
//! struct can't catch. The pure entry point is [`validate_profile_file`] —
//! it returns a list of errors so the CLI layer (and tests) can format them
//! either as a `<path>: <field>: <reason>` line or as JSON.
//!
//! The full rule list is documented in `docs/02-profiles.md` and tracked by
//! the M2 Lane A roadmap (see `docs/11-roadmap.md`).

use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::manifest;

/// One semantic violation against a single manifest file.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct ValidationError {
    /// Dotted-path of the offending field, or `<parse>` for TOML-shape failures.
    pub field: String,
    /// Human-readable explanation of why it's wrong.
    pub reason: String,
}

impl ValidationError {
    fn new(field: impl Into<String>, reason: impl Into<String>) -> Self {
        Self {
            field: field.into(),
            reason: reason.into(),
        }
    }
}

/// Per-file validation result, suitable for `--json` aggregation.
#[derive(Debug, Clone, Serialize)]
pub struct FileResult {
    pub path: PathBuf,
    pub ok: bool,
    pub errors: Vec<ValidationError>,
}

/// Validate one profile manifest file. Returns every violation (no
/// short-circuit) so a single CI run surfaces all problems.
pub fn validate_profile_file(path: &Path) -> Vec<ValidationError> {
    let raw = match std::fs::read_to_string(path) {
        Ok(s) => s,
        Err(e) => return vec![ValidationError::new("<read>", e.to_string())],
    };
    let parsed: manifest::Manifest = match toml::from_str(&raw) {
        Ok(m) => m,
        Err(e) => return vec![ValidationError::new("<parse>", e.to_string())],
    };
    let mut errs = Vec::new();
    check_id(&parsed, path, &mut errs);
    check_paths(&parsed, &mut errs);
    check_service(&parsed, &mut errs);
    check_runtime(&parsed, &mut errs);
    check_health(&parsed, &mut errs);
    check_apparmor(&parsed, &mut errs);
    check_channels(&parsed, &mut errs);
    errs
}

fn check_id(m: &manifest::Manifest, path: &Path, errs: &mut Vec<ValidationError>) {
    let id = &m.profile.id;
    if !is_kebab_lowercase(id) {
        errs.push(ValidationError::new(
            "profile.id",
            format!(
                "\"{id}\" must match ^[a-z][a-z0-9-]*$ (lowercase, kebab-case, starts with letter)"
            ),
        ));
    }
    if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
        if stem != id {
            errs.push(ValidationError::new(
                "profile.id",
                format!("id \"{id}\" doesn't match filename \"{stem}.toml\""),
            ));
        }
    }
}

fn check_paths(m: &manifest::Manifest, errs: &mut Vec<ValidationError>) {
    let id = &m.profile.id;
    let install_root = &m.paths.install_root;
    let expected_prefix = format!("/opt/deputyos/profiles/{id}");
    if !install_root.starts_with('/') {
        errs.push(ValidationError::new(
            "paths.install_root",
            format!("must be absolute (got \"{install_root}\")"),
        ));
    } else if !install_root.starts_with(&expected_prefix) {
        errs.push(ValidationError::new(
            "paths.install_root",
            format!("must start with \"{expected_prefix}\" (got \"{install_root}\")"),
        ));
    }

    let data_dir = &m.paths.data_dir;
    if !data_dir.starts_with('/') && !data_dir.starts_with('~') {
        errs.push(ValidationError::new(
            "paths.data_dir",
            format!("must be absolute or ~/-prefixed (got \"{data_dir}\")"),
        ));
    } else if !(data_dir.starts_with("/home/agent/") || data_dir.starts_with("~/")) {
        errs.push(ValidationError::new(
            "paths.data_dir",
            format!(
                "must live under /home/agent/ or ~/ so the agent user owns it (got \"{data_dir}\")"
            ),
        ));
    }

    let binary = &m.paths.binary;
    if !binary.starts_with('/') {
        errs.push(ValidationError::new(
            "paths.binary",
            format!("must be absolute (got \"{binary}\")"),
        ));
    } else if !binary.starts_with(install_root) {
        errs.push(ValidationError::new(
            "paths.binary",
            format!("must live inside paths.install_root \"{install_root}\" (got \"{binary}\")"),
        ));
    }
}

fn check_service(m: &manifest::Manifest, errs: &mut Vec<ValidationError>) {
    let unit = &m.service.unit;
    if !unit.ends_with(".service") {
        errs.push(ValidationError::new(
            "service.unit",
            format!("must end with .service (got \"{unit}\")"),
        ));
    }
    if m.service.ports.is_empty() {
        errs.push(ValidationError::new(
            "service.ports",
            "must contain at least one port",
        ));
    }
    // u16 already constrains 0..=65535; only zero needs an explicit reject.
    for p in &m.service.ports {
        if *p == 0 {
            errs.push(ValidationError::new(
                "service.ports",
                "port 0 is not a valid listening port",
            ));
        }
    }
}

fn check_runtime(m: &manifest::Manifest, errs: &mut Vec<ValidationError>) {
    let lang = &m.runtime.language;
    match lang.as_str() {
        "node" => {
            if m.runtime.node_version.is_none() {
                errs.push(ValidationError::new(
                    "runtime.node_version",
                    "required when runtime.language = \"node\"",
                ));
            }
        }
        "python" => {
            if m.runtime.python_version.is_none() {
                errs.push(ValidationError::new(
                    "runtime.python_version",
                    "required when runtime.language = \"python\"",
                ));
            }
        }
        "binary" => {}
        other => {
            errs.push(ValidationError::new(
                "runtime.language",
                format!("must be one of \"node\", \"python\", \"binary\" (got \"{other}\")"),
            ));
        }
    }
}

fn check_health(m: &manifest::Manifest, errs: &mut Vec<ValidationError>) {
    let url = &m.health.http_check;
    if !(url.is_empty() || url.starts_with("http://") || url.starts_with("https://")) {
        errs.push(ValidationError::new(
            "health.http_check",
            format!("must be an http(s) URL (got \"{url}\")"),
        ));
    }
}

fn check_apparmor(m: &manifest::Manifest, errs: &mut Vec<ValidationError>) {
    if let Some(aa) = &m.apparmor {
        if !aa.profile.starts_with("/etc/apparmor.d/") {
            errs.push(ValidationError::new(
                "apparmor.profile",
                format!("must live under /etc/apparmor.d/ (got \"{}\")", aa.profile),
            ));
        }
    }
}

fn check_channels(m: &manifest::Manifest, errs: &mut Vec<ValidationError>) {
    let supported = m
        .channels
        .as_ref()
        .map(|c| c.supported.as_slice())
        .unwrap_or(&[]);
    if supported.is_empty() {
        errs.push(ValidationError::new(
            "channels.supported",
            "profile must declare at least one supported channel",
        ));
    }
}

fn is_kebab_lowercase(s: &str) -> bool {
    let bytes = s.as_bytes();
    if bytes.is_empty() {
        return false;
    }
    if !bytes[0].is_ascii_lowercase() {
        return false;
    }
    bytes
        .iter()
        .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || *b == b'-')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kebab_check() {
        assert!(is_kebab_lowercase("openclaw"));
        assert!(is_kebab_lowercase("hermes"));
        assert!(is_kebab_lowercase("foo-bar-2"));
        assert!(!is_kebab_lowercase(""));
        assert!(!is_kebab_lowercase("Openclaw"));
        assert!(!is_kebab_lowercase("2foo"));
        assert!(!is_kebab_lowercase("foo_bar"));
        assert!(!is_kebab_lowercase("-foo"));
    }
}
