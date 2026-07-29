//! Filesystem path discovery for deputyctl.
//!
//! Production paths live under `/etc/deputyos/`, but for local development
//! we transparently fall back to in-repo equivalents so contributors can
//! exercise the CLI without root or a baked image. Every path is overridable
//! via an env var so the smoke harness can stage fixtures.

use std::path::PathBuf;

/// Directory containing one `<id>.toml` per installed profile.
///
/// Resolution order:
/// 1. `DEPUTYOS_PROFILES_DIR` env var, if set.
/// 2. `/etc/deputyos/profiles/`, if it exists.
/// 3. `./profiles/` (workspace-root copy used during dev).
pub fn profiles_dir() -> PathBuf {
    if let Ok(p) = std::env::var("DEPUTYOS_PROFILES_DIR") {
        return PathBuf::from(p);
    }
    let etc = PathBuf::from("/etc/deputyos/profiles");
    if etc.is_dir() {
        return etc;
    }
    PathBuf::from("./profiles")
}

/// Plain-text file containing the active profile id (e.g. `openclaw`).
///
/// Resolution: `DEPUTYOS_ACTIVE_PROFILE_FILE` env var, else
/// `/etc/deputyos/active-profile`. No dev fallback — the absence of this
/// file is a meaningful signal ("no active profile").
pub fn active_profile_file() -> PathBuf {
    if let Ok(p) = std::env::var("DEPUTYOS_ACTIVE_PROFILE_FILE") {
        return PathBuf::from(p);
    }
    PathBuf::from("/etc/deputyos/active-profile")
}

/// JSON file enumerating this device's runtime limits.
///
/// Resolution order:
/// 1. `DEPUTYOS_LIMITS_FILE` env var.
/// 2. `/etc/deputyos/limits.json` if it exists.
/// 3. `deputyctl/etc/limits.qemu-aarch64.json` (dev stub).
pub fn limits_file() -> PathBuf {
    if let Ok(p) = std::env::var("DEPUTYOS_LIMITS_FILE") {
        return PathBuf::from(p);
    }
    let etc = PathBuf::from("/etc/deputyos/limits.json");
    if etc.is_file() {
        return etc;
    }
    PathBuf::from("deputyctl/etc/limits.qemu-aarch64.json")
}

/// Read the active profile id, trimming whitespace. None if unreadable/missing.
pub fn read_active_profile_id() -> Option<String> {
    let path = active_profile_file();
    let raw = std::fs::read_to_string(&path).ok()?;
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

/// JSON file enumerating supported model providers (baked at image build).
///
/// Resolution order:
/// 1. `DEPUTYOS_PROVIDERS_FILE` env var.
/// 2. `/etc/deputyos/providers.json` if it exists.
/// 3. `deputyctl/etc/providers.json` (dev fallback shipped with the source tree).
pub fn providers_file() -> PathBuf {
    if let Ok(p) = std::env::var("DEPUTYOS_PROVIDERS_FILE") {
        return PathBuf::from(p);
    }
    let etc = PathBuf::from("/etc/deputyos/providers.json");
    if etc.is_file() {
        return etc;
    }
    PathBuf::from("deputyctl/etc/providers.json")
}

/// `KEY=VALUE` env file holding the active provider's credentials.
///
/// Resolution: `DEPUTYOS_SECRETS_FILE` env var, else `/etc/deputyos/secrets.env`.
/// No dev fallback — absence of the file means "no provider configured".
pub fn secrets_file() -> PathBuf {
    if let Ok(p) = std::env::var("DEPUTYOS_SECRETS_FILE") {
        return PathBuf::from(p);
    }
    PathBuf::from("/etc/deputyos/secrets.env")
}

/// One-line file naming the configured-active provider id (e.g. `openrouter`).
///
/// Resolution: `DEPUTYOS_ACTIVE_PROVIDER_FILE` env var, else
/// `/etc/deputyos/active-provider`. Absence means "no provider chosen yet"
/// — `model set` writes it, `model test` reads it.
pub fn active_provider_file() -> PathBuf {
    if let Ok(p) = std::env::var("DEPUTYOS_ACTIVE_PROVIDER_FILE") {
        return PathBuf::from(p);
    }
    PathBuf::from("/etc/deputyos/active-provider")
}

/// rclone config consumed by `deputyctl backup` / `restore`.
pub fn rclone_config_file() -> PathBuf {
    if let Ok(p) = std::env::var("DEPUTYOS_RCLONE_CONFIG") {
        return PathBuf::from(p);
    }
    PathBuf::from("/etc/deputyos/rclone.conf")
}

/// Backup destination + retention metadata. TOML-shaped.
pub fn backup_config_file() -> PathBuf {
    if let Ok(p) = std::env::var("DEPUTYOS_BACKUP_CONFIG") {
        return PathBuf::from(p);
    }
    PathBuf::from("/etc/deputyos/backup.toml")
}

/// Local append-only audit spool. Flushed to the cloud API when configured.
pub fn audit_spool_file() -> PathBuf {
    if let Ok(p) = std::env::var("DEPUTYOS_AUDIT_SPOOL") {
        return PathBuf::from(p);
    }
    if let Some(dev) = dev_out_dir() {
        return dev.join("audit").join("spool.jsonl");
    }
    PathBuf::from("/var/lib/deputyos/audit/spool.jsonl")
}

/// Device-scoped cloud backup/audit token.
pub fn cloud_backup_token_file() -> PathBuf {
    if let Ok(p) = std::env::var("DEPUTYOS_BACKUP_TOKEN_FILE") {
        return PathBuf::from(p);
    }
    PathBuf::from("/etc/deputyos/backup-token")
}

/// Stable backup recovery secret. It is deliberately separate from revocable
/// API/device credentials so token rotation never destroys decryptability.
pub fn backup_recovery_key_file() -> PathBuf {
    if let Ok(path) = std::env::var("DEPUTYOS_BACKUP_RECOVERY_KEY_FILE") {
        return PathBuf::from(path);
    }
    PathBuf::from("/etc/deputyos/backup-recovery-key")
}

/// Last managed-backup attempt, consumed by the resident agent and UI.
pub fn backup_status_file() -> PathBuf {
    if let Ok(path) = std::env::var("DEPUTYOS_BACKUP_STATUS_FILE") {
        return PathBuf::from(path);
    }
    PathBuf::from("/var/lib/deputyos/backup-status.json")
}

/// Device-scoped tunnel relay token (minted by the wizard Account step /
/// `accounts/devices/register`). Read by `deputyctl tunnel --integrated` to
/// authenticate the WebSocket relay. Env-overridable so dev E2E can point at
/// a token written under `DEPUTYOS_DEV_OUT` by the wizard.
pub fn tunnel_token_file() -> PathBuf {
    if let Ok(p) = std::env::var("DEPUTYOS_TUNNEL_TOKEN_FILE") {
        return PathBuf::from(p);
    }
    PathBuf::from("/etc/deputyos/tunnel-token")
}

/// Non-secret account label written by the wizard Account step:
/// `{ registered, device_id, device_name, email }`. Contains NO tokens —
/// read by the PWA account card to show presence/identity without ever
/// touching the capability secrets. Env-overridable for dev E2E.
pub fn account_file() -> PathBuf {
    if let Ok(p) = std::env::var("DEPUTYOS_ACCOUNT_FILE") {
        return PathBuf::from(p);
    }
    PathBuf::from("/etc/deputyos/account.json")
}

/// Drive-mounting policy (`{ host_fs, removable, network }`) read + mutated
/// by `deputyctl mounts`. No dev fallback — absence is meaningful (an empty
/// allow-list is the default). Env-overridable so dev E2E and hermetic tests
/// can point at a temp file without passing an explicit path to every call.
pub fn mounts_policy_file() -> PathBuf {
    if let Ok(p) = std::env::var("DEPUTYOS_MOUNTS_POLICY") {
        return PathBuf::from(p);
    }
    PathBuf::from("/etc/deputyos/mounts-policy.json")
}

/// Network egress policy (`{ mode, allow_hosts, set_at_build_time }`) read +
/// mutated by `deputyctl network`. Env-overridable for hermetic tests (M4.5).
pub fn network_policy_file() -> PathBuf {
    if let Ok(p) = std::env::var("DEPUTYOS_NETWORK_POLICY") {
        return PathBuf::from(p);
    }
    PathBuf::from("/etc/deputyos/network-policy.json")
}

/// Marker file whose presence means this device was baked air-gapped
/// (M4.5). Read by `deputyctl::model::airgap_active` + the wizard provider
/// step to switch to baked local-LLM providers. Env-overridable so
/// hermetic tests can simulate airgap without root.
pub fn airgap_flag_file() -> PathBuf {
    if let Ok(p) = std::env::var("DEPUTYOS_AIRGAP_FLAG") {
        return PathBuf::from(p);
    }
    PathBuf::from("/etc/deputyos/airgap.flag")
}

/// Catalog of baked + user-registered GGUF models
/// (`{ $schema, tier, models[] }`) read by `deputyctl::model::load_airgap_models`
/// and appended by `register_gguf`. Env-overridable for hermetic tests (M4.5).
pub fn airgap_catalog_file() -> PathBuf {
    if let Ok(p) = std::env::var("DEPUTYOS_AIRGAP_CATALOG") {
        return PathBuf::from(p);
    }
    PathBuf::from("/opt/deputyos/airgap/models/catalog.json")
}

/// Writable runtime dir holding user-registered GGUF models (the baked
/// ones live read-only under `/opt/deputyos/airgap/models/`). `register_gguf`
/// copies files here. Env-overridable for hermetic tests (M4.5).
pub fn airgap_models_dir() -> PathBuf {
    if let Ok(p) = std::env::var("DEPUTYOS_AIRGAP_MODELS_DIR") {
        return PathBuf::from(p);
    }
    PathBuf::from("/var/lib/deputyos/models")
}

/// Directory holding `pre-message/`, `post-message/`, `cost-alert/`,
/// `update-applied/` subdirs of executable hook scripts.
pub fn hooks_dir() -> PathBuf {
    if let Ok(p) = std::env::var("DEPUTYOS_HOOKS_DIR") {
        return PathBuf::from(p);
    }
    PathBuf::from("/etc/deputyos/hooks.d")
}

/// systemd unit dir for backup timer (system-wide).
pub fn systemd_unit_dir() -> PathBuf {
    if let Ok(p) = std::env::var("DEPUTYOS_SYSTEMD_UNIT_DIR") {
        return PathBuf::from(p);
    }
    PathBuf::from("/etc/systemd/system")
}

/// Wizard state JSON, cleared on factory-reset to force re-prompting.
pub fn wizard_state_file() -> PathBuf {
    if let Ok(p) = std::env::var("DEPUTYOS_WIZARD_STATE_FILE") {
        return PathBuf::from(p);
    }
    PathBuf::from("/var/lib/deputyos/wizard-state.json")
}

/// Returns true when an explicit `DEPUTYOS_DEV_OUT` is set, in which case
/// invasive subcommands write to that directory instead of the real system
/// paths. Convention used by Lane S `factory-reset`, `backup schedule`, etc.
pub fn dev_out_dir() -> Option<PathBuf> {
    std::env::var("DEPUTYOS_DEV_OUT").ok().map(PathBuf::from)
}
