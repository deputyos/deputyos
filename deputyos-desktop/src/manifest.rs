//! Thin wrapper around `deputyctl::release` for launcher concerns.
//!
//! The launcher's manifest needs are a strict subset of `deputyctl update`'s:
//!
//! - Fetch the manifest from a channel URL.
//! - Verify its detached minisign signature using the bundled pubkey.
//! - Pick the artefact whose `target` matches this host's needs (e.g.
//!   `qemu-x86_64` on Linux x86_64, `wsl2` on Windows, `macos-qemu` on macOS).
//! - Resolve the artefact + minisig URLs against the manifest's origin.
//!
//! All four operations are already production-tested in `deputyctl::release`;
//! we just compose them and translate errors into launcher-flavoured
//! messages.

use std::path::Path;

use anyhow::{anyhow, bail, Context, Result};
use deputyctl::release::{
    fetch_to, load_manifest, resolve_url, verify_manifest_signature, Artefact, ManifestSource,
};

/// Fetch + parse + sig-verify a manifest from `manifest_url`.
///
/// On success returns the parsed [`ManifestSource`] (which holds the local
/// staging path so callers can sig-verify artefacts against the same
/// bytes we parsed). On failure: signature failure surfaces as
/// "signature verification failed".
pub fn fetch_and_verify(manifest_url: &str, pubkey_path: &Path) -> Result<ManifestSource> {
    let src = load_manifest(manifest_url)
        .with_context(|| format!("loading manifest from {manifest_url}"))?;

    // Sidecar minisig: same naming convention deputyctl uses
    // (`<manifest_url>.minisig`).
    let sig_url = format!("{manifest_url}.minisig");
    let sig_path = src.local_path.with_extension("json.minisig");
    fetch_to(&sig_url, &sig_path)
        .with_context(|| format!("fetching manifest signature from {sig_url}"))?;

    verify_manifest_signature(&src.local_path, &sig_path, pubkey_path)
        .map_err(|e| anyhow!("signature verification failed: {e}"))?;

    Ok(src)
}

/// All artefacts in the manifest whose `target` matches `desired_target`,
/// in manifest order. Empty when none match.
pub fn artefacts_for_target<'a>(
    src: &'a ManifestSource,
    desired_target: &str,
) -> Vec<&'a Artefact> {
    src.manifest
        .artefacts
        .iter()
        .filter(|a| a.target == desired_target)
        .collect()
}

/// Pick one artefact for `desired_target`.
///
/// - `profile = Some(p)`: require an artefact with that exact profile id.
/// - `profile = None`, exactly one match: return it (back-compat with the
///   single-profile manifests the launcher was built around).
/// - `profile = None`, multiple matches: return an error naming the candidate
///   profiles so the caller can surface the choice. The launcher CLI prints a
///   numbered list and asks the user to pass `--profile <id>` rather than
///   silently picking the first one.
///
/// Returns an error listing all available targets when nothing matches the
/// target at all.
pub fn pick_artefact<'a>(
    src: &'a ManifestSource,
    desired_target: &str,
    profile: Option<&str>,
) -> Result<&'a Artefact> {
    let cands = artefacts_for_target(src, desired_target);
    if cands.is_empty() {
        let available: Vec<&str> = src
            .manifest
            .artefacts
            .iter()
            .map(|a| a.target.as_str())
            .collect();
        bail!(
            "no artefact with target='{}' in manifest; available: [{}]",
            desired_target,
            available.join(", ")
        )
    } else if let Some(p) = profile {
        cands
            .iter()
            .find(|a| a.profile == p)
            .ok_or_else(|| {
                let profiles: Vec<&str> = cands.iter().map(|a| a.profile.as_str()).collect();
                anyhow!(
                    "no artefact with target='{desired_target}' profile='{p}'; \
                     available profiles: [{}]",
                    profiles.join(", ")
                )
            })
            .copied()
    } else if cands.len() == 1 {
        Ok(cands[0])
    } else {
        // Ambiguous: caller (the launcher) prints the numbered list and
        // asks for --profile. The message names the candidates as a fallback
        // for callers that don't do their own listing (e.g. `update`).
        let profiles: Vec<String> = cands
            .iter()
            .map(|a| format!("{} ({} bytes)", a.profile, a.size_bytes))
            .collect();
        bail!(
            "multiple images available for target='{desired_target}'; \
             specify --profile <id>; available: [{}]",
            profiles.join(", ")
        )
    }
}

/// Resolve `artefact.url` (and `artefact.minisig_url`) against the
/// manifest's origin. Returns `(image_url, minisig_url)`.
pub fn artefact_urls(src: &ManifestSource, artefact: &Artefact) -> Result<(String, String)> {
    let img = artefact
        .url
        .as_ref()
        .ok_or_else(|| anyhow!("artefact '{}' has no url field", artefact.filename))?;
    if artefact.minisig_url.is_empty() {
        bail!("artefact '{}' has no minisig_url", artefact.filename);
    }
    Ok((
        resolve_url(&src.origin, img),
        resolve_url(&src.origin, &artefact.minisig_url),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use deputyctl::release::{Artefact, Manifest};
    use std::collections::BTreeMap;

    fn artefact(profile: &str, filename: &str, size_bytes: u64) -> Artefact {
        Artefact {
            target: "qemu-x86_64".into(),
            profile: profile.into(),
            filename: filename.into(),
            format: "qcow2".into(),
            size_bytes,
            sha256: "0".repeat(64),
            minisig_url: format!("{filename}.minisig"),
            url: Some(filename.to_string()),
        }
    }

    fn fake_source_with(artefacts: Vec<Artefact>) -> ManifestSource {
        // We can't construct ManifestSource directly because its tempdir
        // field is private. Build via the loader from an inline file.
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("manifest.json");
        let m = Manifest {
            schema_version: 1,
            release_version: "2026.4.27".into(),
            channel: "dev".into(),
            released_at: "2026-04-27T00:00:00Z".into(),
            tracker: BTreeMap::new(),
            artefacts,
            wizard_version: None,
            chat_ui_version: None,
            desktop_launchers: BTreeMap::new(),
            mounts_policy_schema_version: 1,
        };
        std::fs::write(&path, serde_json::to_string(&m).expect("ser")).expect("write");
        let real_path = path.to_str().expect("utf8").to_string();
        // Keep the tempdir alive by leaking it — only fine in tests.
        let leaked = Box::leak(Box::new(dir));
        let _ = leaked;
        load_manifest(&real_path).expect("load")
    }

    fn fake_source(_origin: &str) -> ManifestSource {
        // This artefact uses the path-style relative url/minisig_url that
        // `artefact_urls_resolved_relative` asserts against; the shared
        // `artefact()` helper instead sets url=filename for the profile
        // selection tests, so build this one inline.
        fake_source_with(vec![Artefact {
            target: "qemu-x86_64".into(),
            profile: "openclaw".into(),
            filename: "deputyos-openclaw-qemu-x86_64-2026.4.27-dev.qcow2".into(),
            format: "qcow2".into(),
            size_bytes: 1024,
            sha256: "0".repeat(64),
            minisig_url: "2026.4.27/img.qcow2.minisig".into(),
            url: Some("2026.4.27/img.qcow2".into()),
        }])
    }

    #[test]
    fn pick_artefact_match() {
        let src = fake_source("ignored");
        let a = pick_artefact(&src, "qemu-x86_64", None).expect("found");
        assert_eq!(a.target, "qemu-x86_64");
        assert_eq!(a.profile, "openclaw");
    }

    #[test]
    fn pick_artefact_miss_lists_available() {
        let src = fake_source("ignored");
        let err = pick_artefact(&src, "wsl2", None).expect_err("should fail");
        let msg = format!("{err:#}");
        assert!(msg.contains("qemu-x86_64"), "got: {msg}");
    }

    #[test]
    fn artefacts_for_target_returns_all_matches() {
        let src = fake_source_with(vec![
            artefact("hermes", "h.qcow2", 1),
            artefact("openclaw", "o.qcow2", 2),
            Artefact {
                target: "rpi5".into(),
                ..artefact("khoj", "k.img", 3)
            },
        ]);
        let got = artefacts_for_target(&src, "qemu-x86_64");
        assert_eq!(got.len(), 2);
        assert_eq!(got[0].profile, "hermes");
        assert_eq!(got[1].profile, "openclaw");
    }

    #[test]
    fn pick_artefact_with_profile_selects_it() {
        let src = fake_source_with(vec![
            artefact("hermes", "h.qcow2", 1),
            artefact("openclaw", "o.qcow2", 2),
        ]);
        let a = pick_artefact(&src, "qemu-x86_64", Some("openclaw")).expect("found");
        assert_eq!(a.profile, "openclaw");
    }

    #[test]
    fn pick_artefact_ambiguous_without_profile_errors() {
        let src = fake_source_with(vec![
            artefact("hermes", "h.qcow2", 1),
            artefact("openclaw", "o.qcow2", 2),
        ]);
        let err = pick_artefact(&src, "qemu-x86_64", None).expect_err("ambiguous");
        let msg = format!("{err:#}");
        assert!(msg.contains("hermes"), "got: {msg}");
        assert!(msg.contains("openclaw"), "got: {msg}");
        assert!(msg.contains("--profile"), "got: {msg}");
    }

    #[test]
    fn pick_artefact_unknown_profile_lists_available() {
        let src = fake_source_with(vec![
            artefact("hermes", "h.qcow2", 1),
            artefact("openclaw", "o.qcow2", 2),
        ]);
        let err = pick_artefact(&src, "qemu-x86_64", Some("khoj")).expect_err("not found");
        let msg = format!("{err:#}");
        assert!(msg.contains("hermes"), "got: {msg}");
        assert!(msg.contains("openclaw"), "got: {msg}");
        assert!(msg.contains("khoj"), "got: {msg}");
    }

    #[test]
    fn artefact_urls_resolved_relative() {
        let src = fake_source("file:///tmp/dist/manifest.json");
        let a = &src.manifest.artefacts[0];
        let (img, sig) = artefact_urls(&src, a).expect("resolved");
        // origin is whatever load_manifest stamped — bare path stays bare.
        assert!(img.ends_with("2026.4.27/img.qcow2"), "got: {img}");
        assert!(sig.ends_with("2026.4.27/img.qcow2.minisig"), "got: {sig}");
    }
}
