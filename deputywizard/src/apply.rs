//! "Apply" step — persist wizard answers to canonical config files.
//!
//! Two modes:
//!
//! - **Production** (default on a real bake): write directly to
//!   `/etc/hostname`, `/etc/timezone`, `/etc/deputyos/active-profile`,
//!   `/etc/deputyos/secrets.env`, `/home/agent/.ssh/authorized_keys`, etc.
//!   and shell out to `hostnamectl`, `timedatectl`, `ufw`, `systemctl`.
//!
//! - **Dev** (`DEPUTYWIZARD_DEV=1` or detected non-deputyOS host): mirror the
//!   same file tree under `dev-out/` and print "would do X" for shell
//!   commands. No state outside `dev-out/` is touched. This is the mode
//!   used by `make wizard` and by the integration tests.
//!
//! In production we still atomic-write (`tmp` + rename) and tighten modes.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use anyhow::{anyhow, Context, Result};
use serde::Serialize;

use deputyctl::model::Provider;

use crate::state::Answers;

/// Optional, opt-in pieces of the apply step that came in with Phase 5
/// (Tailscale, Cloudflare Tunnel, backup). Each field is `None`/`"skip"`
/// when the user opted out of that step, so the apply path is a series of
/// independent on/off branches.
#[derive(Debug, Default, Clone)]
pub struct ApplyExtras<'a> {
    pub tailscale_authkey: Option<&'a str>,
    /// Raw credentials JSON content for a Cloudflare named tunnel.
    pub cloudflared_credentials: Option<&'a str>,
    pub backup: Option<BackupRef<'a>>,
}

#[derive(Debug, Clone)]
pub struct BackupRef<'a> {
    pub kind: &'a str,
    pub fields: &'a BTreeMap<String, String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApplyMode {
    Production,
    Dev,
}

impl ApplyMode {
    /// Detect mode at startup. Honour the `DEPUTYWIZARD_DEV` env var; otherwise
    /// inspect `/etc/os-release` for `ID=deputyos`.
    pub fn detect() -> Self {
        if std::env::var_os("DEPUTYWIZARD_DEV").is_some() {
            return ApplyMode::Dev;
        }
        match std::fs::read_to_string("/etc/os-release") {
            Ok(s) if s.lines().any(|l| l.trim() == "ID=deputyos") => ApplyMode::Production,
            _ => ApplyMode::Dev,
        }
    }
}

/// Where should outputs be written? In dev mode, this is the `dev-out/`
/// directory (or whatever `dev_out_override` says); in production, it's `/`.
pub fn root(mode: ApplyMode, dev_out_override: Option<&Path>) -> PathBuf {
    if mode == ApplyMode::Dev {
        if let Some(p) = dev_out_override {
            return p.to_path_buf();
        }
        if let Ok(p) = std::env::var("DEPUTYWIZARD_DEV_OUT") {
            return PathBuf::from(p);
        }
        return PathBuf::from("dev-out");
    }
    PathBuf::from("/")
}

/// Return the path under `root` that corresponds to a canonical absolute
/// path. In production mode this is just `path` itself; in dev mode it's
/// rooted under `dev-out/`.
fn rooted(root: &Path, abs: &str) -> PathBuf {
    let stripped = abs.strip_prefix('/').unwrap_or(abs);
    root.join(stripped)
}

#[derive(Debug, Clone, Serialize)]
pub struct ApplyReport {
    pub wrote: Vec<String>,
    pub commands: Vec<String>,
    pub mode: &'static str,
}

/// Plan + execute the apply step. The `provider_secret` is the API key the
/// user typed at step 3 — passed through state alongside the answers, but
/// never persisted to the wizard state file.
#[allow(clippy::too_many_arguments)]
pub fn apply(
    mode: ApplyMode,
    dev_out_override: Option<&Path>,
    answers: &Answers,
    provider_secret: Option<(&Provider, &str)>,
    ports_to_open: &[u16],
    profile_unit: Option<&str>,
    extras: &ApplyExtras<'_>,
) -> Result<ApplyReport> {
    let root = root(mode, dev_out_override);
    let mut wrote = Vec::new();
    let mut commands = Vec::new();

    // 1. /etc/hostname
    if let Some(h) = answers.hostname.as_deref() {
        let path = rooted(&root, "/etc/hostname");
        write_atomic(&path, &format!("{h}\n"), 0o644)?;
        wrote.push(path.display().to_string());
        commands.push(format!("hostnamectl set-hostname {h}"));
        if mode == ApplyMode::Production {
            run_cmd("hostnamectl", &["set-hostname", h])?;
        }
    }

    // 2. /etc/timezone
    if let Some(tz) = answers.timezone.as_deref() {
        let path = rooted(&root, "/etc/timezone");
        write_atomic(&path, &format!("{tz}\n"), 0o644)?;
        wrote.push(path.display().to_string());
        commands.push(format!("timedatectl set-timezone {tz}"));
        if mode == ApplyMode::Production {
            run_cmd("timedatectl", &["set-timezone", tz])?;
        }
    }

    // 3. /etc/deputyos/active-profile
    if let Some(profile) = answers.profile.as_deref() {
        let path = rooted(&root, "/etc/deputyos/active-profile");
        write_atomic(&path, &format!("{profile}\n"), 0o644)?;
        wrote.push(path.display().to_string());
    }

    // 4. /etc/deputyos/secrets.env (provider key only — appended).
    if let Some((provider, secret)) = provider_secret {
        let path = rooted(&root, "/etc/deputyos/secrets.env");
        append_secret(&path, &provider.key_env_var, secret)?;
        wrote.push(path.display().to_string());
    }

    // 5. /etc/deputyos/<profile>/channels.d/<channel>.enabled — touch one per
    // channel selected. The actual per-channel config wizard is M3-rest.
    if let Some(profile) = answers.profile.as_deref() {
        for ch in &answers.channels {
            let rel = format!("/etc/deputyos/{profile}/channels.d/{ch}.enabled");
            let path = rooted(&root, &rel);
            write_atomic(&path, "", 0o644)?;
            wrote.push(path.display().to_string());
        }
    }

    // 6. ~/.ssh/authorized_keys for the agent + root accounts (key-only auth).
    if !answers.ssh_keys.is_empty() {
        // Dedupe + normalize.
        let unique: BTreeSet<String> = answers
            .ssh_keys
            .iter()
            .map(|k| k.trim().to_string())
            .filter(|k| !k.is_empty())
            .collect();
        let body = unique.iter().cloned().collect::<Vec<_>>().join("\n") + "\n";
        for user_path in [
            "/home/agent/.ssh/authorized_keys",
            "/root/.ssh/authorized_keys",
        ] {
            let path = rooted(&root, user_path);
            write_atomic(&path, &body, 0o600)?;
            wrote.push(path.display().to_string());
        }
    }

    // 7. ufw allow rules for the profile's exposed ports — best-effort. The
    // per-channel port mapping is profile-specific; M3-rest will refine.
    for port in ports_to_open {
        commands.push(format!("ufw allow {port}/tcp"));
        if mode == ApplyMode::Production {
            run_cmd("ufw", &["allow", &format!("{port}/tcp")])?;
        }
    }

    // 8. systemctl start <profile-unit>
    if let Some(unit) = profile_unit {
        commands.push(format!("systemctl start {unit}"));
        if mode == ApplyMode::Production {
            run_cmd("systemctl", &["start", unit])?;
        }
    }

    // 9. Tailscale auth key.
    if let Some(authkey) = extras.tailscale_authkey {
        let path = rooted(&root, "/etc/deputyos/secrets.env");
        append_secret(&path, "TAILSCALE_AUTHKEY", authkey)?;
        wrote.push(path.display().to_string());
        commands.push("tailscale up --auth-key=$TAILSCALE_AUTHKEY".into());
        if mode == ApplyMode::Production {
            // Best-effort: a missing tailscale binary should fall through to
            // a friendly error rather than hard-fail the wizard. We surface
            // it via the report's commands list either way.
            let _ = run_cmd("tailscale", &["up", &format!("--auth-key={authkey}")]);
        }
    }

    // 10. Cloudflared credentials (named tunnel only).
    if let Some(creds) = extras.cloudflared_credentials {
        let path = rooted(&root, "/etc/deputyos/cloudflared/credentials.json");
        write_atomic(&path, creds, 0o600)?;
        wrote.push(path.display().to_string());
        // Tunnel name extraction is best-effort here; the wizard already
        // validated and stored it in answers, but we re-derive defensively.
        let tname = serde_json::from_str::<serde_json::Value>(creds)
            .ok()
            .and_then(|v| {
                v.get("TunnelName")
                    .and_then(|x| x.as_str().map(String::from))
                    .or_else(|| v.get("TunnelID").and_then(|x| x.as_str().map(String::from)))
            })
            .unwrap_or_else(|| "deputyos".into());
        commands.push(format!("cloudflared tunnel run {tname}"));
        if mode == ApplyMode::Production {
            let _ = run_cmd("cloudflared", &["tunnel", "run", &tname]);
        }
    } else if answers.cloudflare_tunnel_choice.as_deref() == Some("quick") {
        commands.push("cloudflared tunnel --url http://localhost:8088".into());
        if mode == ApplyMode::Production {
            // Don't block the wizard waiting on the tunnel URL; a
            // background-launching wrapper script lives at bake time
            // (Lane B). Here we just record intent.
        }
    }

    // 11. Stable recovery key. It is generated for registered or explicitly
    // backup-enabled systems and never derived from a revocable API token.
    if answers.account_registered || extras.backup.is_some() {
        let recovery_path = rooted(&root, "/etc/deputyos/backup-recovery-key");
        deputyctl::recovery_key::initialize_at(&recovery_path)?;
        wrote.push(recovery_path.display().to_string());
    }

    // 12. Backup destination and schedule.
    if let Some(b) = extras.backup.as_ref() {
        if b.kind == "managed" {
            commands.push("deputyctl backup schedule --at 03:00 --to-cloud".into());
            if mode == ApplyMode::Production {
                let _ = run_cmd(
                    "deputyctl",
                    &["backup", "schedule", "--at", "03:00", "--to-cloud"],
                );
            }
        } else {
            let env_path = rooted(&root, "/etc/deputyos/backup.env");
            let mut env_body = String::new();
            for (k, v) in b.fields.iter() {
                let key = format!("BACKUP_{}", k.to_uppercase());
                env_body.push_str(&format!("{key}={v}\n"));
            }
            write_atomic(&env_path, &env_body, 0o600)?;
            wrote.push(env_path.display().to_string());

            let conf_path = rooted(&root, "/etc/deputyos/rclone.conf");
            let conf = render_rclone_conf(b.kind, b.fields);
            write_atomic(&conf_path, &conf, 0o600)?;
            wrote.push(conf_path.display().to_string());

            let bucket = b.fields.get("bucket").map(String::as_str).unwrap_or("");
            let config_path = rooted(&root, "/etc/deputyos/backup.toml");
            let config = format!(
                "[backup]\nremote = \"remote:{}\"\nretention_days = 30\n",
                bucket.replace('"', "")
            );
            write_atomic(&config_path, &config, 0o600)?;
            wrote.push(config_path.display().to_string());
            commands.push("deputyctl backup schedule --at 03:00".into());
            if mode == ApplyMode::Production {
                let _ = run_cmd("deputyctl", &["backup", "schedule", "--at", "03:00"]);
            }
        }
    }

    // 13. Egress policy — apply the wizard's network-mode choice. `open` is
    // the no-op default (an existing policy is left untouched); `whitelist`
    // (which seeds allow_hosts from network-defaults.json) and `airgap` shell
    // out to deputyctl. Skipped entirely when no choice was made.
    if let Some(m) = answers.egress_mode.as_deref() {
        if m != "open" {
            commands.push(format!("deputyctl network mode {m}"));
            commands.push("deputyctl network apply".into());
            if mode == ApplyMode::Production {
                // Best-effort: a missing deputyctl/nft binary surfaces as recorded
                // intent, not a hard wizard failure (mirrors tailscale/cloudflared).
                let _ = run_cmd("deputyctl", &["network", "mode", m]);
                let _ = run_cmd("deputyctl", &["network", "apply"]);
            }
        }
    }

    Ok(ApplyReport {
        wrote,
        commands,
        mode: match mode {
            ApplyMode::Production => "production",
            ApplyMode::Dev => "dev",
        },
    })
}

/// Render the rclone.conf snippet for the chosen backup kind. The snippet
/// contains a single `[remote]` section so `deputyctl backup now` can just
/// `rclone copy <data_dir> remote:<bucket>` against it.
fn render_rclone_conf(kind: &str, fields: &BTreeMap<String, String>) -> String {
    fn g<'a>(m: &'a BTreeMap<String, String>, k: &str) -> &'a str {
        m.get(k).map(String::as_str).unwrap_or("")
    }
    match kind {
        "b2" => format!(
            "[remote]\ntype = b2\naccount = {}\nkey = {}\n",
            g(fields, "account"),
            g(fields, "key"),
        ),
        "r2" => format!(
            "[remote]\ntype = s3\nprovider = Cloudflare\naccess_key_id = {}\nsecret_access_key = {}\nendpoint = {}\nacl = private\n",
            g(fields, "access_key_id"),
            g(fields, "secret_access_key"),
            g(fields, "endpoint"),
        ),
        "s3" => format!(
            "[remote]\ntype = s3\nprovider = Other\naccess_key_id = {}\nsecret_access_key = {}\nendpoint = {}\nacl = private\n",
            g(fields, "access_key_id"),
            g(fields, "secret_access_key"),
            g(fields, "endpoint"),
        ),
        _ => "[remote]\n# unsupported backup kind\n".into(),
    }
}

pub fn write_atomic(path: &Path, body: &str, mode: u32) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating {}", parent.display()))?;
    }
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, body).with_context(|| format!("writing {}", tmp.display()))?;
    set_mode(&tmp, mode);
    std::fs::rename(&tmp, path)
        .with_context(|| format!("renaming into place {}", path.display()))?;
    Ok(())
}

/// Append `KEY=VALUE` to a secrets.env, replacing any existing line for the
/// same key. Mode 0600.
fn append_secret(path: &Path, key: &str, value: &str) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating {}", parent.display()))?;
    }
    let existing = std::fs::read_to_string(path).unwrap_or_default();
    let mut lines: Vec<String> = existing
        .lines()
        .filter(|l| {
            let t = l.trim_start();
            let t = t.strip_prefix("export ").unwrap_or(t);
            !t.starts_with(&format!("{key}=")) && !l.trim().is_empty()
        })
        .map(String::from)
        .collect();
    // Strip surrounding whitespace from value, but otherwise verbatim. The
    // user's API key is a single token — newlines or spaces in the middle
    // of it would be a paste error, but we don't validate that here.
    let escaped = value.replace(['\n', '\r'], "");
    lines.push(format!("{key}={escaped}"));
    let body = lines.join("\n") + "\n";
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, body)?;
    set_mode(&tmp, 0o600);
    std::fs::rename(&tmp, path)?;
    Ok(())
}

#[cfg(unix)]
fn set_mode(path: &Path, mode: u32) {
    use std::os::unix::fs::PermissionsExt;
    if let Ok(meta) = std::fs::metadata(path) {
        let mut perms = meta.permissions();
        perms.set_mode(mode);
        let _ = std::fs::set_permissions(path, perms);
    }
}

#[cfg(not(unix))]
fn set_mode(_path: &Path, _mode: u32) {}

#[cfg(unix)]
fn run_cmd(prog: &str, args: &[&str]) -> Result<()> {
    let status = std::process::Command::new(prog).args(args).status()?;
    if !status.success() {
        return Err(anyhow!("{} {:?} exited {:?}", prog, args, status.code()));
    }
    Ok(())
}

#[cfg(not(unix))]
fn run_cmd(_prog: &str, _args: &[&str]) -> Result<()> {
    Ok(())
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    fn provider_fixture() -> Provider {
        Provider {
            id: "openrouter".into(),
            display_name: "OpenRouter".into(),
            kind: "openai-compatible".into(),
            endpoint_default: "https://openrouter.ai/api/v1".into(),
            key_env_var: "OPENROUTER_API_KEY".into(),
            key_format: "sk-or-v1-...".into(),
            default_model: "anthropic/claude-sonnet-4.6".into(),
            supported_models_hint: String::new(),
        }
    }

    #[test]
    fn apply_writes_expected_files_in_dev_mode() {
        let dir = tempfile::tempdir().unwrap();
        let answers = Answers {
            hostname: Some("deputyos".into()),
            timezone: Some("UTC".into()),
            profile: Some("openclaw".into()),
            provider: Some("openrouter".into()),
            channels: vec!["telegram".into()],
            ssh_keys: vec!["ssh-ed25519 AAAA me@host".into()],
            ..Answers::default()
        };
        let p = provider_fixture();
        let report = apply(
            ApplyMode::Dev,
            Some(dir.path()),
            &answers,
            Some((&p, "test-key-12345")),
            &[8080],
            Some("openclaw-gateway.service"),
            &ApplyExtras::default(),
        )
        .unwrap();

        let root = dir.path();
        assert_eq!(
            std::fs::read_to_string(root.join("etc/hostname"))
                .unwrap()
                .trim(),
            "deputyos"
        );
        assert_eq!(
            std::fs::read_to_string(root.join("etc/deputyos/active-profile"))
                .unwrap()
                .trim(),
            "openclaw"
        );
        let secrets = std::fs::read_to_string(root.join("etc/deputyos/secrets.env")).unwrap();
        assert!(
            secrets.contains("OPENROUTER_API_KEY=test-key-12345"),
            "secrets.env: {secrets}"
        );
        assert!(root
            .join("etc/deputyos/openclaw/channels.d/telegram.enabled")
            .exists());
        let auth_keys =
            std::fs::read_to_string(root.join("home/agent/.ssh/authorized_keys")).unwrap();
        assert!(auth_keys.contains("ssh-ed25519 AAAA me@host"));
        assert!(report.commands.iter().any(|c| c == "ufw allow 8080/tcp"));
        assert!(report
            .commands
            .iter()
            .any(|c| c == "systemctl start openclaw-gateway.service"));
    }

    #[test]
    fn apply_writes_backup_and_tailscale_when_extras_present() {
        let dir = tempfile::tempdir().unwrap();
        let answers = Answers {
            hostname: Some("deputyos".into()),
            timezone: Some("UTC".into()),
            profile: Some("openclaw".into()),
            provider: Some("openrouter".into()),
            channels: vec![],
            ssh_keys: vec!["ssh-ed25519 AAAA me@host".into()],
            tailscale_enabled: true,
            cloudflare_tunnel_choice: Some("named".into()),
            cloudflare_tunnel_name: Some("agent-tunnel".into()),
            backup_kind: Some("r2".into()),
            backup_meta: Default::default(),
            egress_mode: Some("open".into()),
            account_email: None,
            account_api_base: None,
            account_registered: false,
            drives_acknowledged: false,
        };
        let mut fields = BTreeMap::new();
        fields.insert("account".into(), "acct123".into());
        fields.insert("access_key_id".into(), "AKIA".into());
        fields.insert("secret_access_key".into(), "SECRET".into());
        fields.insert("bucket".into(), "deputyos-backups".into());
        fields.insert(
            "endpoint".into(),
            "https://acct123.r2.cloudflarestorage.com".into(),
        );
        let creds = r#"{"AccountTag":"acct123","TunnelID":"abc-123","TunnelName":"agent-tunnel","TunnelSecret":"sek"}"#;
        let extras = ApplyExtras {
            tailscale_authkey: Some("tskey-auth-test"),
            cloudflared_credentials: Some(creds),
            backup: Some(BackupRef {
                kind: "r2",
                fields: &fields,
            }),
        };
        let p = provider_fixture();
        apply(
            ApplyMode::Dev,
            Some(dir.path()),
            &answers,
            Some((&p, "key")),
            &[8080],
            Some("openclaw-gateway.service"),
            &extras,
        )
        .unwrap();
        let root = dir.path();
        let secrets = std::fs::read_to_string(root.join("etc/deputyos/secrets.env")).unwrap();
        assert!(
            secrets.contains("TAILSCALE_AUTHKEY=tskey-auth-test"),
            "secrets.env: {secrets}"
        );
        let creds_path = root.join("etc/deputyos/cloudflared/credentials.json");
        let creds_body = std::fs::read_to_string(&creds_path).unwrap();
        assert!(creds_body.contains("agent-tunnel"));
        let env = std::fs::read_to_string(root.join("etc/deputyos/backup.env")).unwrap();
        assert!(env.contains("BACKUP_ACCOUNT=acct123"));
        assert!(env.contains("BACKUP_BUCKET=deputyos-backups"));
        let conf = std::fs::read_to_string(root.join("etc/deputyos/rclone.conf")).unwrap();
        assert!(conf.contains("[remote]"));
        assert!(conf.contains("type = s3"));
        assert!(conf.contains("provider = Cloudflare"));
        assert!(conf.contains("acct123.r2.cloudflarestorage.com"));
        let backup = std::fs::read_to_string(root.join("etc/deputyos/backup.toml")).unwrap();
        assert!(backup.contains("remote = \"remote:deputyos-backups\""));
        assert!(root.join("etc/deputyos/backup-recovery-key").is_file());
    }

    #[test]
    fn egress_apply_records_commands_in_dev_and_skips_open() {
        // In Dev the apply step never shells out; it records the intent so the
        // report can be inspected. `open` (the no-op default) records nothing.
        let dir = tempfile::tempdir().unwrap();
        let answers_open = Answers {
            hostname: Some("deputyos".into()),
            egress_mode: Some("open".into()),
            ..Answers::default()
        };
        let report_open = apply(
            ApplyMode::Dev,
            Some(dir.path()),
            &answers_open,
            None,
            &[],
            None,
            &ApplyExtras::default(),
        )
        .unwrap();
        assert!(
            !report_open
                .commands
                .iter()
                .any(|c| c.contains("deputyctl network")),
            "open mode must not record egress commands: {:?}",
            report_open.commands
        );

        let answers_wl = Answers {
            hostname: Some("deputyos".into()),
            egress_mode: Some("whitelist".into()),
            ..Answers::default()
        };
        let report_wl = apply(
            ApplyMode::Dev,
            Some(dir.path()),
            &answers_wl,
            None,
            &[],
            None,
            &ApplyExtras::default(),
        )
        .unwrap();
        assert!(report_wl
            .commands
            .iter()
            .any(|c| c == "deputyctl network mode whitelist"));
        assert!(report_wl
            .commands
            .iter()
            .any(|c| c == "deputyctl network apply"));
    }

    #[test]
    fn append_secret_replaces_existing_key() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("secrets.env");
        std::fs::write(
            &path,
            "OTHER=keep\nOPENROUTER_API_KEY=oldval\nMORE=stillhere\n",
        )
        .unwrap();
        append_secret(&path, "OPENROUTER_API_KEY", "newval").unwrap();
        let body = std::fs::read_to_string(&path).unwrap();
        assert!(body.contains("OTHER=keep"));
        assert!(body.contains("MORE=stillhere"));
        assert!(body.contains("OPENROUTER_API_KEY=newval"));
        assert!(!body.contains("oldval"));
    }
}
