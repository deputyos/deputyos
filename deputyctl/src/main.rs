//! `deputyctl` — single management surface for deputyOS appliance images.
//!
//! Command tree is the **frozen M0 surface** from `docs/02-profiles.md`.
//! Lane A of M1 phase 2 wires real implementations behind: `version`,
//! `profile list`, `profile status`, `doctor`, `limits`, `up`, `down`,
//! `restart`, `status`, `logs`. Subcommands not in that list stay as
//! labelled stubs that point at the milestone where they land (see
//! `docs/11-roadmap.md`).
//!
//! Architectural notes: sync I/O end-to-end (no tokio) keeps the binary
//! small and CI fast. Every external command is a clean shell-out
//! (`systemctl`, `journalctl`, `ufw`, `timedatectl`) — robust across system
//! vs `--user` mode. Linux-specific checks return `Skip` on non-Linux dev
//! hosts so `cargo test` and `cargo run` work on macOS too.

use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::Result;
use clap::{Parser, Subcommand};

use deputyctl::{
    audit, backup, commands, cost, doctor, factory_reset, limits, message_relay, model,
    model_register, model_set, model_test, mounts, network, paths, profile, profile_switch,
    quiet_hours, reconcile, restore, rollback, shell, systemd, tunnel, update, validate, voice,
    watchdog,
};

/// Exit code for "command parsed but feature not implemented yet".
/// Matches `EX_USAGE` from sysexits.h, which is what scripts probe for.
const EX_USAGE: u8 = 64;

#[derive(Debug, Parser)]
#[command(
    name = "deputyctl",
    version,
    about = "Management CLI for deputyOS appliance images",
    long_about = None,
)]
struct Cli {
    /// Increase log verbosity (overrides RUST_LOG). -v=debug, -vv=trace.
    #[arg(short, long, action = clap::ArgAction::Count, global = true)]
    verbose: u8,

    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Run the first-boot wizard (TTY + web on :8088).
    Init,

    /// Profile inspection and switching.
    #[command(subcommand)]
    Profile(ProfileCmd),

    /// Start the active profile.
    Up,
    /// Stop the active profile.
    Down,
    /// Restart the active profile.
    Restart,
    /// Short health summary of the active profile.
    Status {
        /// Emit structured JSON.
        #[arg(long)]
        json: bool,
    },
    /// Stream the active profile's journal.
    Logs {
        /// Follow the journal (like `journalctl -f`).
        #[arg(long, short = 'f')]
        follow: bool,
    },
    /// Drop into the profile's CLI.
    Shell {
        /// Print the planned exec instead of running it.
        #[arg(long)]
        dry_run: bool,
    },

    /// Full health check; nonzero on any failure.
    Doctor {
        /// Emit a JSON report instead of a table.
        #[arg(long)]
        json: bool,
    },
    /// Check resident services and perform bounded, allow-listed repairs.
    Repair {
        /// Emit the repair report as JSON.
        #[arg(long)]
        json: bool,
        /// Repair on the first failed probe instead of waiting for two failures.
        #[arg(long)]
        force: bool,
    },
    /// Per-device capability + limitation report.
    Limits {
        /// Emit raw JSON instead of the human-readable block.
        #[arg(long)]
        json: bool,
    },
    /// Print SBOM + build manifest summary.
    Version {
        /// Emit JSON instead of key-value text.
        #[arg(long)]
        json: bool,
    },

    /// Model provider + model picker.
    #[command(subcommand)]
    Model(ModelCmd),

    /// Update client.
    Update {
        /// Read the signed manifest from CDN and report what would change.
        #[arg(long)]
        check: bool,
        /// Apply an update (A/B swap with watchdog rollback).
        #[arg(long)]
        apply: bool,
        /// Skip the confirmation prompt for --apply.
        #[arg(long, short = 'y')]
        yes: bool,
        /// Emit structured JSON output.
        #[arg(long)]
        json: bool,
        /// Read the signed manifest from this local path instead of the
        /// configured update URL — the sneakernet path (M4.5). The
        /// `<path>.minisig` sidecar is read from disk next to it; the artefact
        /// + its signature are resolved locally too. No network egress. The
        ///   minisign + sha256 gates are unchanged.
        #[arg(long, value_name = "PATH")]
        from: Option<String>,
    },
    /// Force boot into the inactive A/B slot on next reboot.
    Rollback,

    /// Backup operations.
    #[command(subcommand)]
    Backup(BackupCmd),
    /// Audit event spool and cloud flush.
    #[command(subcommand)]
    Audit(AuditCmd),
    /// Restore operations.
    #[command(subcommand)]
    Restore(RestoreCmd),

    /// Wipe data partition; keep system partitions intact.
    FactoryReset {
        /// Skip the typed-confirmation prompt.
        #[arg(long, short = 'y')]
        yes: bool,
        /// Print planned actions without making changes.
        #[arg(long)]
        dry_run: bool,
    },

    /// Open a Quick Tunnel (Cloudflare or deputyOS integrated) and print the URL.
    Tunnel {
        /// Local port to expose. Defaults to 8088 (the wizard port).
        #[arg(long, default_value_t = 8088)]
        port: u16,
        /// Detach and write PID to /run/deputyos/cloudflared.pid.
        #[arg(long)]
        background: bool,
        /// Use the deputyOS integrated tunnel instead of cloudflared.
        /// Requires an account and registered device token.
        #[arg(long)]
        integrated: bool,
    },

    /// Cost guardrails: per-day + per-month caps with auto-pause.
    ///
    /// **Surface extension** beyond the M0 frozen surface in
    /// `docs/02-profiles.md`. Authorized by Lane M5 of `docs/11-roadmap.md`.
    Cost {
        #[command(subcommand)]
        sub: Option<CostCmd>,
        /// Emit JSON for the default summary view.
        #[arg(long, global = true)]
        json: bool,
        /// Run the caps gate now; nonzero exit if tripped.
        #[arg(long)]
        check: bool,
    },

    /// Quiet-hours schedule: when to pause / refuse messages.
    ///
    /// **Surface extension** beyond the M0 frozen surface (see `Cost`).
    #[command(name = "quiet-hours")]
    QuietHours {
        #[command(subcommand)]
        sub: Option<QuietHoursCmd>,
        /// Emit JSON for the default summary view.
        #[arg(long, global = true)]
        json: bool,
    },

    /// Network egress policy (open | whitelist | airgap).
    ///
    /// **Surface extension** beyond the M0 frozen surface. Authorized by
    /// Lane M4.5 of `docs/11-roadmap.md` (airgap baseline) and Lane M5.5
    /// (whitelist mode, now live — see `deputyctl/src/network.rs`).
    #[command(subcommand)]
    Network(NetworkCmd),

    /// Drive mounting policy: host-FS, removable, SMB / NFS shares.
    ///
    /// **Surface extension** beyond the M0 frozen surface. Authorized by
    /// Lane M3.5 of `docs/11-roadmap.md`.
    #[command(subcommand)]
    Mounts(MountsCmd),

    /// Voice interface: status, test, configure wake-word.
    ///
    /// **Surface extension** beyond the M0 frozen surface. Authorized by
    /// Lane M6 of `docs/11-roadmap.md`.
    #[command(subcommand)]
    Voice(VoiceCmd),

    /// Remote command-queue poller (device-side; M9.4). The appliance runs
    /// `deputyctl commands poll` under systemd (`deputyos-command-poller.service`)
    /// to drain commands an account owner enqueues via the API. Allow-listed
    /// execution: `ping` + `restart-agent` only; anything else is acked
    /// `unsupported` without running. See `deputyctl/src/commands.rs`.
    #[command(subcommand)]
    Commands(CommandsCmd),
}

#[derive(Debug, Subcommand)]
enum ProfileCmd {
    /// Show installed profiles + versions.
    List {
        #[arg(long)]
        json: bool,
    },
    /// Detail on the active profile.
    Status {
        #[arg(long)]
        json: bool,
    },
    /// Stop old profile, start the new one.
    Switch {
        /// Profile id (e.g. "openclaw", "hermes").
        id: String,
        /// Skip the interactive confirmation prompt.
        #[arg(long, short = 'y')]
        yes: bool,
        /// Print planned actions without making changes.
        #[arg(long)]
        dry_run: bool,
    },
    /// Validate one or more profile manifests against the schema + invariants.
    Validate {
        /// One or more `.toml` paths. Globs are expanded by the shell.
        #[arg(required = true)]
        paths: Vec<PathBuf>,
        /// Emit a structured JSON report.
        #[arg(long)]
        json: bool,
    },
}

#[derive(Debug, Subcommand)]
enum ModelCmd {
    /// Show configured + available providers.
    List {
        /// Emit a structured JSON list.
        #[arg(long)]
        json: bool,
    },
    /// Interactive provider/model picker.
    Set {
        /// Skip the provider selection prompt.
        #[arg(long)]
        provider: Option<String>,
        /// Read the key from stdin instead of a TTY prompt.
        #[arg(long)]
        key_from_stdin: bool,
        /// Skip the confirmation prompt.
        #[arg(long, short = 'y')]
        yes: bool,
    },
    /// Single-token round-trip test against the current config.
    Test {
        /// Test a specific provider id instead of the active one.
        #[arg(long)]
        provider: Option<String>,
        /// Emit structured JSON output.
        #[arg(long)]
        json: bool,
    },
    /// Register a local GGUF model for airgap / local LLM serving.
    Register {
        /// Path to the .gguf file.
        path: PathBuf,
        /// Model id (e.g. "my-custom-model").
        #[arg(long)]
        id: Option<String>,
        /// Enable a systemd llama.cpp unit for this model immediately.
        #[arg(long)]
        enable: bool,
    },
}

#[derive(Debug, Subcommand)]
enum BackupCmd {
    /// One-off rclone push to the configured destination.
    Now {
        /// Pass `--dry-run` to rclone (preview only).
        #[arg(long)]
        dry_run: bool,
        /// Upload an age-encrypted bundle to api.deputyos.com.
        /// Requires a registered account + backup token.
        #[arg(long)]
        to_cloud: bool,
    },
    /// Configure a cron-like schedule.
    Schedule {
        /// Run every <interval> (e.g. `6h`, `30m`, `1d`). Default 6h.
        #[arg(long)]
        every: Option<String>,
        /// Run at <HH:MM> daily (mutually exclusive with --every).
        #[arg(long)]
        at: Option<String>,
        /// Schedule managed encrypted backups to the deputyOS object store.
        #[arg(long)]
        to_cloud: bool,
        /// Print the current schedule.
        #[arg(long)]
        list: bool,
        /// Stop and remove the timer.
        #[arg(long)]
        disable: bool,
    },
    /// Create, export or import the stable backup recovery key.
    RecoveryKey {
        #[command(subcommand)]
        action: RecoveryKeyCmd,
    },
}

#[derive(Debug, Subcommand)]
enum RecoveryKeyCmd {
    /// Create a recovery key if one does not already exist.
    Init,
    /// Print the recovery key for secure offline storage.
    Export,
    /// Import a recovery key from a file.
    Import {
        file: PathBuf,
        /// Replace the current key. Export it first: old backups require it.
        #[arg(long)]
        replace: bool,
    },
}

#[derive(Debug, Subcommand)]
enum RestoreCmd {
    /// List snapshots in the user bucket.
    #[command(name = "list")]
    List {
        #[arg(long)]
        json: bool,
    },
    /// Atomic restore from a snapshot id.
    Snapshot {
        /// The snapshot id from `restore --list`.
        #[arg(long = "snapshot")]
        id: String,
        /// Skip the confirmation prompt.
        #[arg(long, short = 'y')]
        yes: bool,
    },
    /// Atomic restore from an age-encrypted cloud bundle (needs a backup token).
    #[command(name = "from-cloud")]
    FromCloud {
        /// The snapshot id to download, decrypt, and restore.
        #[arg(long = "snapshot")]
        id: String,
        /// Skip the confirmation prompt.
        #[arg(long, short = 'y')]
        yes: bool,
    },
}

#[derive(Debug, Subcommand)]
enum AuditCmd {
    /// Append one audit event to the local spool.
    Emit {
        /// Event kind, e.g. backup_completed, update_applied, policy_changed.
        #[arg(long)]
        kind: String,
        /// JSON object payload.
        #[arg(long, default_value = "{}")]
        payload: String,
    },
    /// Show recent spooled events.
    List {
        /// Show the last N events.
        #[arg(long, default_value_t = 20)]
        last: usize,
        /// Emit JSON.
        #[arg(long)]
        json: bool,
    },
    /// Upload the current spool to the cloud audit API.
    Flush {
        /// API base URL.
        #[arg(long, default_value = "https://api.deputyos.com")]
        api_base: String,
        /// Print what would be uploaded without sending it.
        #[arg(long)]
        dry_run: bool,
    },
}

#[derive(Debug, Subcommand)]
enum CostCmd {
    /// Update cost configuration (caps, behaviour).
    Set {
        /// Daily spend cap in USD.
        #[arg(long = "daily-cap")]
        daily_cap: Option<f64>,
        /// Monthly spend cap in USD.
        #[arg(long = "monthly-cap")]
        monthly_cap: Option<f64>,
        /// What to do on cap trip: pause | warn | nothing.
        #[arg(long = "on-cap-trip")]
        on_cap_trip: Option<String>,
        /// Fire CostAlert at this percentage of cap (0..=100).
        #[arg(long = "warn-at-pct")]
        warn_at_pct: Option<u32>,
    },
    /// Clear the tripped marker (does NOT touch the ledger).
    Reset,
    /// Dump recent ledger entries.
    Ledger {
        /// Show the last N entries (default 20).
        #[arg(long, default_value_t = 20)]
        last: usize,
        #[arg(long)]
        json: bool,
    },
}

#[derive(Debug, Subcommand)]
enum MountsCmd {
    /// List configured mounts (host-FS, removable, network).
    List {
        #[arg(long)]
        json: bool,
    },
    /// Add a host-FS share. The guest path must live under /mnt/deputyos/.
    Add {
        /// Stable id (e.g. `documents`, `code`).
        #[arg(long)]
        id: String,
        /// Path on the host (informational; the helper materialises it).
        #[arg(long = "host-path")]
        host_path: String,
        /// Path inside the appliance, must start with /mnt/deputyos/.
        #[arg(long = "guest-path")]
        guest_path: String,
        /// Mount mode: ro (default) or rw.
        #[arg(long, default_value = "ro")]
        mode: String,
    },
    /// Remove a mount by id.
    Remove { id: String },
    /// Add a network share (CIFS or NFS).
    #[command(name = "network-add")]
    NetworkAdd {
        /// Stable id (e.g. "nas-photos").
        #[arg(long)]
        id: String,
        /// Kind: cifs or nfs.
        #[arg(long)]
        kind: String,
        /// Source: //nas/photos or nas:/srv/photos.
        #[arg(long)]
        source: String,
        /// Path inside the appliance, must start with /mnt/deputyos/.
        #[arg(long = "guest-path")]
        guest_path: String,
        /// Mount mode: ro (default) or rw.
        #[arg(long, default_value = "ro")]
        mode: String,
        /// Env var in secrets.env for credentials (CIFS).
        #[arg(long = "creds-env")]
        credentials_env: Option<String>,
    },
    /// Apply the current policy (restart deputyos-mounts.service).
    Apply,
    /// Health check on all configured mounts.
    Health {
        #[arg(long)]
        json: bool,
    },
}

#[derive(Debug, Subcommand)]
enum NetworkCmd {
    /// Show the current policy (mode + allow_hosts).
    Status {
        #[arg(long)]
        json: bool,
    },
    /// Set the policy mode. `whitelist` seeds allow_hosts from
    /// network-defaults.json when the list is empty; run `network apply`
    /// afterwards to render + reload nftables.
    Mode {
        /// One of: open, whitelist, airgap.
        mode: String,
    },
    /// Convenience for `mode open` — flips an airgap image back to open.
    Unlock,
    /// Convenience for `mode airgap` — locks egress to RFC1918 + mDNS only.
    Lock,
    /// Generate and reload nftables rules from the current policy.
    Apply,
    /// Mutate the allow-list.
    #[command(subcommand)]
    Allow(NetworkAllowCmd),
}

#[derive(Debug, Subcommand)]
enum NetworkAllowCmd {
    /// Add a host to the allow-list.
    Add {
        /// Host to allow (e.g. `api.openai.com`).
        host: String,
    },
    /// Remove a host from the allow-list.
    Remove { host: String },
}

#[derive(Debug, Subcommand)]
enum VoiceCmd {
    /// Show voice-relay state + installed models.
    Status {
        #[arg(long)]
        json: bool,
    },
    /// Play a test phrase through the speaker.
    TestSpeaker,
    /// Record 3s and transcribe via whisper.
    TestMic,
    /// Set the wake word.
    SetWakeWord { word: String },
    /// Enable voice (start systemd unit).
    Enable,
    /// Disable voice (stop systemd unit).
    Disable,
}

#[derive(Debug, Subcommand)]
enum CommandsCmd {
    /// Poll the remote command queue: drain once and exit. Useful for one-shot
    /// runs + tests; the systemd unit runs `poll --loop` for the daemon.
    Poll {
        /// Override the API base URL (default: `DEPUTYOS_API_BASE` or
        /// `https://api.deputyos.com`).
        #[arg(long, value_name = "URL")]
        api_base: Option<String>,
        /// Loop forever (drain, sleep, repeat) instead of draining once. This
        /// is what the systemd unit runs.
        #[arg(long)]
        r#loop: bool,
        /// Loop sleep interval, seconds (default 30; only used with `--loop`).
        #[arg(long, default_value_t = 30, value_name = "SECS")]
        interval: u64,
    },
}

#[derive(Debug, Subcommand)]
enum QuietHoursCmd {
    /// Update the schedule.
    Set {
        /// Start time HH:MM (local TZ).
        #[arg(long)]
        start: Option<String>,
        /// End time HH:MM (local TZ).
        #[arg(long)]
        end: Option<String>,
        /// Enable the schedule.
        #[arg(long)]
        enable: bool,
        /// Disable the schedule.
        #[arg(long)]
        disable: bool,
        /// What to do during quiet hours: pause | refuse | nothing.
        #[arg(long)]
        behaviour: Option<String>,
    },
}

fn main() -> ExitCode {
    // Hidden, library-only flag: `deputyctl --internal-run-relay <SOCKET>`.
    // Used by the systemd service that fronts the message relay so the
    // OpenClaw / Hermes agents can fire `pre-message` / `post-message` /
    // `cost-alert` hooks via a Unix socket. Intentionally not part of the
    // frozen subcommand surface (`docs/02-profiles.md`) and not shown in
    // `--help`. See `deputyctl/src/message_relay.rs` for the wire protocol.
    if let Some(code) = maybe_run_relay() {
        return ExitCode::from(code);
    }
    if let Some(code) = maybe_run_internal_maintenance() {
        return ExitCode::from(code);
    }

    let cli = Cli::parse();
    init_tracing(cli.verbose);

    match dispatch(cli.command) {
        Ok(code) => ExitCode::from(code),
        Err(e) => {
            eprintln!("deputyctl: error: {e:#}");
            ExitCode::from(1)
        }
    }
}

/// If argv contains the hidden `--internal-run-relay` flag, run the relay
/// to completion and return an exit code; otherwise return `None` so the
/// normal clap dispatch proceeds.
fn maybe_run_relay() -> Option<u8> {
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        if a == "--internal-run-relay" {
            init_tracing(0);
            let socket = match args.next() {
                Some(p) => PathBuf::from(p),
                None => message_relay::default_socket_path(),
            };
            match message_relay::run_relay(&socket) {
                Ok(()) => return Some(0),
                Err(e) => {
                    eprintln!("deputyctl --internal-run-relay: {e:#}");
                    return Some(1);
                }
            }
        }
        // Stop scanning once we hit something that looks like a subcommand
        // — keeps us from accidentally swallowing user input.
        if !a.starts_with('-') {
            return None;
        }
    }
    None
}

/// Hidden maintenance entrypoints used by tightly sandboxed systemd units.
/// They run before clap and never become part of the public command surface.
fn maybe_run_internal_maintenance() -> Option<u8> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|a| a == "--internal-watchdog-check") {
        init_tracing(0);
        match watchdog::run_check() {
            Ok(()) => return Some(0),
            Err(e) => {
                eprintln!("deputyctl watchdog: {e:#}");
                return Some(1);
            }
        }
    }
    if args.iter().any(|a| a == "--internal-reconcile") {
        init_tracing(0);
        return Some(match reconcile::run(true, false) {
            Ok(code) => code,
            Err(error) => {
                eprintln!("deputyctl reconcile: {error:#}");
                1
            }
        });
    }
    if args.iter().any(|a| a == "--internal-watchdog-confirm") {
        init_tracing(0);
        return Some(match watchdog::run_confirm() {
            Ok(()) => 0,
            Err(error) => {
                eprintln!("deputyctl watchdog confirm: {error:#}");
                1
            }
        });
    }
    if args.iter().any(|a| a == "--internal-update-cycle") {
        init_tracing(0);
        return Some(match update::run_automatic() {
            Ok(code) => code,
            Err(error) => {
                eprintln!("deputyctl automatic update: {error:#}");
                1
            }
        });
    }
    None
}

fn init_tracing(verbose: u8) {
    let default = match verbose {
        0 => "info",
        1 => "debug",
        _ => "trace",
    };
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new(default));
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(false)
        .without_time()
        .with_writer(std::io::stderr)
        .init();
}

fn dispatch(command: Command) -> Result<u8> {
    match command {
        Command::Version { json } => cmd_version(json),

        Command::Profile(p) => match p {
            ProfileCmd::List { json } => cmd_profile_list(json),
            ProfileCmd::Status { json } => cmd_profile_status(json),
            ProfileCmd::Switch { id, yes, dry_run } => {
                profile_switch::run(&id, profile_switch::SwitchOpts { yes, dry_run })
            }
            ProfileCmd::Validate { paths, json } => cmd_profile_validate(paths, json),
        },

        Command::Doctor { json } => cmd_doctor(json),
        Command::Repair { json, force } => reconcile::run(json, force),
        Command::Limits { json } => cmd_limits(json),

        Command::Up => cmd_lifecycle("start"),
        Command::Down => cmd_lifecycle("stop"),
        Command::Restart => cmd_lifecycle("restart"),
        Command::Status { json } => cmd_status(json),
        Command::Logs { follow } => cmd_logs(follow),

        Command::Init => cmd_init(),
        Command::Shell { dry_run } => shell::run(shell::ShellOpts { dry_run }),
        Command::Model(m) => match m {
            ModelCmd::List { json } => cmd_model_list(json),
            ModelCmd::Set {
                provider,
                key_from_stdin,
                yes,
            } => model_set::run(model_set::SetOpts {
                provider,
                key_from_stdin,
                yes,
            }),
            ModelCmd::Test { provider, json } => {
                model_test::run(model_test::TestOpts { provider, json })
            }
            ModelCmd::Register { path, id, enable } => {
                let model_id = id.unwrap_or_else(|| {
                    path.file_stem()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_else(|| "unnamed".to_string())
                });
                model_register::run(model_register::RegisterOpts {
                    path,
                    id: model_id,
                    enable,
                })
            }
        },
        Command::Update {
            check,
            apply,
            yes,
            json,
            from,
        } => cmd_update(check, apply, yes, json, from.as_deref()),
        Command::Rollback => rollback::run(),
        Command::Backup(b) => match b {
            BackupCmd::Now { dry_run, to_cloud } => {
                backup::run_now(backup::NowOpts { dry_run, to_cloud })
            }
            BackupCmd::Schedule {
                every,
                at,
                to_cloud,
                list,
                disable,
            } => backup::run_schedule(backup::ScheduleOpts {
                every,
                at,
                to_cloud,
                list,
                disable,
            }),
            BackupCmd::RecoveryKey { action } => match action {
                RecoveryKeyCmd::Init => {
                    let (info, secret) = deputyctl::recovery_key::initialize()?;
                    println!("key_id: {}", info.key_id);
                    println!("path:   {}", info.path);
                    if let Some(secret) = secret {
                        println!("recovery_key: {secret}");
                        eprintln!(
                            "Save this recovery key offline now. It will not be printed automatically again."
                        );
                    } else {
                        println!("status: already initialized");
                    }
                    Ok(0)
                }
                RecoveryKeyCmd::Export => {
                    println!("{}", deputyctl::recovery_key::load()?);
                    Ok(0)
                }
                RecoveryKeyCmd::Import { file, replace } => {
                    let secret = std::fs::read_to_string(&file)?;
                    let info = deputyctl::recovery_key::import(&secret, replace)?;
                    println!("key_id: {}", info.key_id);
                    println!("path:   {}", info.path);
                    Ok(0)
                }
            },
        },
        Command::Audit(a) => match a {
            AuditCmd::Emit { kind, payload } => audit::run_emit(audit::EmitOpts { kind, payload }),
            AuditCmd::List { last, json } => audit::run_list(audit::ListOpts { last, json }),
            AuditCmd::Flush { api_base, dry_run } => {
                audit::run_flush(audit::FlushOpts { api_base, dry_run })
            }
        },
        Command::Restore(r) => match r {
            RestoreCmd::List { json } => restore::run_list(restore::ListOpts { json }),
            RestoreCmd::Snapshot { id, yes } => {
                restore::run_snapshot(restore::SnapshotOpts { id, yes })
            }
            RestoreCmd::FromCloud { id, yes } => {
                restore::run_from_cloud(restore::FromCloudOpts { id, yes })
            }
        },
        Command::FactoryReset { yes, dry_run } => factory_reset::run(factory_reset::ResetOpts {
            yes,
            dry_run,
            confirmation_override: None,
        }),
        Command::Tunnel {
            port,
            background,
            integrated,
        } => {
            if integrated {
                tunnel::run_integrated(tunnel::TunnelOpts { port, background })
            } else {
                tunnel::run(tunnel::TunnelOpts { port, background })
            }
        }
        Command::Cost { sub, json, check } => match sub {
            None => cost::run_summary(cost::CostOpts { json, check }),
            Some(CostCmd::Set {
                daily_cap,
                monthly_cap,
                on_cap_trip,
                warn_at_pct,
            }) => cost::run_set(cost::SetOpts {
                daily_cap_usd: daily_cap,
                monthly_cap_usd: monthly_cap,
                on_cap_trip,
                warn_at_pct,
            }),
            Some(CostCmd::Reset) => cost::run_reset(),
            Some(CostCmd::Ledger { last, json: lj }) => cost::run_ledger_dump(cost::LedgerOpts {
                last,
                json: lj || json,
            }),
        },
        Command::QuietHours { sub, json } => match sub {
            None => quiet_hours::run_show(json),
            Some(QuietHoursCmd::Set {
                start,
                end,
                enable,
                disable,
                behaviour,
            }) => quiet_hours::run_set(quiet_hours::SetOpts {
                start,
                end,
                enable,
                disable,
                behaviour,
            }),
        },

        Command::Network(net) => cmd_network(net),
        Command::Mounts(mnt) => cmd_mounts(mnt),
        Command::Voice(v) => match v {
            VoiceCmd::Status { json } => {
                let status = voice::status_json()?;
                if json {
                    println!("{}", serde_json::to_string_pretty(&status)?);
                } else {
                    println!(
                        "voice service:  {}",
                        if status.service_active {
                            "active"
                        } else {
                            "inactive"
                        }
                    );
                    println!("enabled:        {}", status.enabled);
                    println!(
                        "wake word:      {}",
                        status.wake_word.as_deref().unwrap_or("not set")
                    );
                    println!(
                        "STT model:      {}",
                        status.stt_model.as_deref().unwrap_or("unknown")
                    );
                    println!(
                        "TTS voice:      {}",
                        status.tts_voice.as_deref().unwrap_or("unknown")
                    );
                    println!(
                        "audio device:   {}",
                        status.audio_device.as_deref().unwrap_or("default")
                    );
                    println!(
                        "voice-relay.sh: {}",
                        if status.voice_relay_exists {
                            "installed"
                        } else {
                            "not installed"
                        }
                    );
                }
                Ok(0)
            }
            VoiceCmd::TestSpeaker => voice::test_speaker(),
            VoiceCmd::TestMic => voice::test_mic(),
            VoiceCmd::SetWakeWord { word } => voice::set_wake_word(&word),
            VoiceCmd::Enable => voice::enable_voice(),
            VoiceCmd::Disable => voice::disable_voice(),
        },
        Command::Commands(c) => match c {
            CommandsCmd::Poll {
                api_base,
                r#loop,
                interval,
            } => {
                if r#loop {
                    commands::run_loop(api_base.as_deref(), Some(interval))
                } else {
                    commands::run_once(api_base.as_deref())
                }
            }
        },
    }
}

// ---------------------------------------------------------------------------
// mounts
// ---------------------------------------------------------------------------

fn cmd_mounts(cmd: MountsCmd) -> Result<u8> {
    use std::str::FromStr;

    match cmd {
        MountsCmd::List { json } => {
            let entries = mounts::list(None)?;
            if json {
                println!("{}", serde_json::to_string_pretty(&entries)?);
            } else if entries.is_empty() {
                println!("(no mounts configured — see `deputyctl mounts add --help`)");
            } else {
                println!(
                    "{:<14} {:<16} {:<32} {:<4} source",
                    "kind", "id", "guest-path", "mode"
                );
                for e in entries {
                    println!(
                        "{:<14} {:<16} {:<32} {:<4} {}",
                        e.kind, e.id, e.guest_path, e.mode, e.source
                    );
                }
            }
            Ok(0)
        }
        MountsCmd::Add {
            id,
            host_path,
            guest_path,
            mode,
        } => {
            let m = mounts::Mode::from_str(&mode)?;
            let policy = mounts::add_host_fs(None, &id, &host_path, &guest_path, m)?;
            println!(
                "added host-fs mount {id:?}; policy now has {} entries",
                policy.host_fs.len()
            );
            println!(
                "apply with: sudo systemctl restart deputyos-mounts.service  # (M3.5 materialiser)"
            );
            Ok(0)
        }
        MountsCmd::Remove { id } => {
            mounts::remove_by_id(None, &id)?;
            println!("removed mount {id:?}");
            Ok(0)
        }
        MountsCmd::NetworkAdd {
            id,
            kind,
            source,
            guest_path,
            mode,
            credentials_env,
        } => {
            use std::str::FromStr;
            let m = mounts::Mode::from_str(&mode)?;
            let pol = mounts::add_network_mount(
                None,
                &id,
                &kind,
                &source,
                &guest_path,
                m,
                credentials_env.as_deref(),
            )?;
            println!(
                "added network mount {id:?} ({kind}); policy now has {} entries",
                pol.network.len()
            );
            println!("apply with: deputyctl mounts apply");
            Ok(0)
        }
        MountsCmd::Apply => {
            let count = mounts::apply_mounts()?;
            println!("mounts applied: {count} configured mount(s)");
            Ok(0)
        }
        MountsCmd::Health { json } => {
            let results = mounts::health_check(None)?;
            if json {
                println!("{}", serde_json::to_string_pretty(&results)?);
            } else if results.is_empty() {
                println!("(no mounts configured)");
            } else {
                println!(
                    "{:<16} {:<14} {:<32} {:<12} detail",
                    "id", "kind", "guest-path", "status"
                );
                for r in results {
                    println!(
                        "{:<16} {:<14} {:<32} {:<12} {}",
                        r.id,
                        r.kind,
                        r.guest_path,
                        r.status,
                        r.detail.as_deref().unwrap_or("")
                    );
                }
            }
            Ok(0)
        }
    }
}

// ---------------------------------------------------------------------------
// network
// ---------------------------------------------------------------------------

fn cmd_network(cmd: NetworkCmd) -> Result<u8> {
    use std::str::FromStr;

    match cmd {
        NetworkCmd::Status { json } => {
            let payload = network::status_json(None)?;
            if json {
                println!("{}", serde_json::to_string_pretty(&payload)?);
            } else {
                let mode = payload
                    .get("mode")
                    .and_then(|v| v.as_str())
                    .unwrap_or("unknown");
                println!("network mode: {mode}");
                if let Some(allow) = payload.get("allow_hosts").and_then(|v| v.as_array()) {
                    if allow.is_empty() {
                        println!("allow_hosts: (empty)");
                    } else {
                        println!("allow_hosts:");
                        for h in allow {
                            if let Some(s) = h.as_str() {
                                println!("  - {s}");
                            }
                        }
                    }
                }
            }
            Ok(0)
        }
        NetworkCmd::Mode { mode } => {
            let m = network::Mode::from_str(&mode)?;
            let pol = network::set_mode(m, None)?;
            println!("network mode now: {}", pol.mode);
            Ok(0)
        }
        NetworkCmd::Unlock => {
            let pol = network::set_mode(network::Mode::Open, None)?;
            println!("network mode now: {}", pol.mode);
            Ok(0)
        }
        NetworkCmd::Lock => {
            let pol = network::set_mode(network::Mode::Airgap, None)?;
            println!("network mode now: {}", pol.mode);
            Ok(0)
        }
        NetworkCmd::Apply => {
            network::apply_ruleset(None)?;
            let pol = network::read(None)?;
            println!(
                "nftables ruleset applied — mode: {}, hosts: {}",
                pol.mode,
                pol.allow_hosts.len()
            );
            Ok(0)
        }
        NetworkCmd::Allow(NetworkAllowCmd::Add { host }) => {
            let pol = network::allow_mutate(None, |hosts| hosts.push(host.clone()))?;
            println!("allow_hosts: {} entries", pol.allow_hosts.len());
            println!("run 'deputyctl network apply' to reload nftables");
            Ok(0)
        }
        NetworkCmd::Allow(NetworkAllowCmd::Remove { host }) => {
            let pol = network::allow_mutate(None, |hosts| hosts.retain(|h| h != &host))?;
            println!("allow_hosts: {} entries", pol.allow_hosts.len());
            println!("run 'deputyctl network apply' to reload nftables");
            Ok(0)
        }
    }
}

// ---------------------------------------------------------------------------
// version
// ---------------------------------------------------------------------------

fn cmd_version(json: bool) -> Result<u8> {
    let bin_version = env!("CARGO_PKG_VERSION");
    let git_sha = option_env!("DEPUTYOS_GIT_SHA").unwrap_or("unknown");
    let build_date = option_env!("DEPUTYOS_BUILD_DATE").unwrap_or("unknown");
    let target = current_target_triple();
    let kernel = read_kernel_version();
    let apparmor = read_apparmor_mode();
    let profiles = profile::list().unwrap_or_default();

    if json {
        let payload = serde_json::json!({
            "binary_version": bin_version,
            "git_sha": git_sha,
            "build_date": build_date,
            "target": target,
            "kernel": kernel,
            "apparmor": apparmor,
            "profiles_dir": paths::profiles_dir().display().to_string(),
            "profiles": profiles,
        });
        println!("{}", serde_json::to_string_pretty(&payload)?);
    } else {
        println!("deputyctl version:    {bin_version}");
        println!("git_sha:             {git_sha}");
        println!("build_date:          {build_date}");
        println!("target:              {target}");
        println!("kernel:              {kernel}");
        println!("apparmor:            {apparmor}");
        println!("profiles_dir:        {}", paths::profiles_dir().display());
        if profiles.is_empty() {
            println!("profiles:            (none installed)");
        } else {
            println!("profiles:");
            for p in &profiles {
                let marker = if p.active { " *active*" } else { "" };
                println!(
                    "  - {} ({} @ {}){marker}",
                    p.id, p.display_name, p.pinned_version
                );
            }
        }
    }
    Ok(0)
}

fn current_target_triple() -> &'static str {
    // Built-in env var available to build scripts and rustc.
    // `rustc -vV` "host:" matches when no build script overrides.
    option_env!("TARGET").unwrap_or(std::env::consts::ARCH)
}

fn read_kernel_version() -> String {
    std::fs::read_to_string("/proc/sys/kernel/osrelease")
        .map(|s| s.trim().to_string())
        .unwrap_or_else(|_| "unknown".into())
}

fn read_apparmor_mode() -> String {
    let path = "/sys/kernel/security/apparmor/profiles";
    match std::fs::read_to_string(path) {
        Ok(s) if !s.is_empty() => {
            let n = s.lines().count();
            format!("loaded ({n} profiles)")
        }
        Ok(_) => "loaded (0 profiles)".into(),
        Err(_) => "unknown".into(),
    }
}

// ---------------------------------------------------------------------------
// profile list / status
// ---------------------------------------------------------------------------

fn cmd_profile_list(json: bool) -> Result<u8> {
    let profiles = profile::list()?;
    if profiles.is_empty() {
        eprintln!("no profiles installed");
        if json {
            println!("[]");
        }
        return Ok(0);
    }
    if json {
        println!("{}", serde_json::to_string_pretty(&profiles)?);
    } else {
        for p in &profiles {
            let marker = if p.active { " *" } else { "  " };
            println!(
                "{marker}{} {} {} {}",
                p.id, p.display_name, p.pinned_version, p.release_channel
            );
        }
    }
    Ok(0)
}

fn cmd_profile_status(json: bool) -> Result<u8> {
    let (id, m) = match profile::load_active() {
        Ok(x) => x,
        Err(_) => {
            eprintln!("no active profile");
            return Ok(1);
        }
    };
    let unit = m.service.unit.clone();
    let active_state = if systemd::available() {
        systemd::is_active(&unit)
            .map(|s| s.as_str().to_string())
            .unwrap_or_else(|_| "unknown".into())
    } else {
        "unavailable".into()
    };
    if json {
        let payload = serde_json::json!({
            "id": id,
            "display_name": m.profile.display_name,
            "pinned_version": m.profile.pinned_version,
            "install_root": m.paths.install_root,
            "data_dir": m.paths.data_dir,
            "unit": unit,
            "active_state": active_state,
        });
        println!("{}", serde_json::to_string_pretty(&payload)?);
    } else {
        println!("id:              {id}");
        println!("display_name:    {}", m.profile.display_name);
        println!("pinned_version:  {}", m.profile.pinned_version);
        println!("install_root:    {}", m.paths.install_root);
        println!("data_dir:        {}", m.paths.data_dir);
        println!("unit:            {unit}");
        println!("active_state:    {active_state}");
    }
    Ok(0)
}

// ---------------------------------------------------------------------------
// doctor / limits
// ---------------------------------------------------------------------------

fn cmd_doctor(json: bool) -> Result<u8> {
    let report = doctor::run_all();
    if json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        print!("{}", doctor::format_table(&report));
    }
    Ok(if report.fails == 0 { 0 } else { 1 })
}

fn cmd_limits(json: bool) -> Result<u8> {
    let l = limits::load()?;
    if json {
        println!("{}", serde_json::to_string_pretty(&l)?);
    } else {
        print!("{}", limits::format_human(&l));
    }
    Ok(0)
}

// ---------------------------------------------------------------------------
// up / down / restart / status / logs
// ---------------------------------------------------------------------------

fn cmd_lifecycle(verb: &str) -> Result<u8> {
    if !systemd::available() {
        eprintln!("systemd not available on this platform");
        return Ok(1);
    }
    let (_id, m) = match profile::load_active() {
        Ok(x) => x,
        Err(e) => {
            eprintln!("deputyctl {verb}: {e}");
            return Ok(1);
        }
    };
    let status = systemd::run(verb, &m.service.unit)?;
    Ok(if status.success() { 0 } else { 1 })
}

fn cmd_status(json: bool) -> Result<u8> {
    let (id, m) = match profile::load_active() {
        Ok(x) => x,
        Err(e) => {
            if json {
                let payload = serde_json::json!({
                    "profile_id": null,
                    "unit": null,
                    "active_state": "unknown",
                    "agent": {
                        "profile_id": null,
                        "unit": null,
                        "active_state": "unknown",
                    },
                    "tunnel": {
                        "unit": "deputyos-tunnel.service",
                        "active_state": "unknown",
                        "enabled": false,
                        "on_demand": true,
                    },
                    "uptime_seconds": 0,
                    "cost_tripped": cost::is_tripped(),
                    "error": e.to_string(),
                });
                println!("{}", serde_json::to_string_pretty(&payload)?);
            } else {
                eprintln!("deputyctl status: {e}");
            }
            return Ok(1);
        }
    };

    let systemd_available = systemd::available();
    let state = if systemd_available {
        systemd::is_active(&m.service.unit)
            .map(|s| s.as_str().to_string())
            .unwrap_or_else(|_| "unknown".into())
    } else {
        "unavailable".into()
    };
    let tunnel_state = if systemd_available {
        systemd::is_active("deputyos-tunnel.service")
            .map(|s| s.as_str().to_string())
            .unwrap_or_else(|_| "unknown".into())
    } else {
        "unavailable".into()
    };
    let tunnel_enabled = systemd_available && systemd::is_enabled("deputyos-tunnel.service");
    let fails = doctor::quick_fail_count();

    if json {
        let payload = serde_json::json!({
            "profile_id": id,
            "unit": m.service.unit,
            "active_state": state,
            "agent": {
                "profile_id": id,
                "display_name": m.profile.display_name,
                "unit": m.service.unit,
                "active_state": state,
                "journal_unit": m.health.journal_unit,
            },
            "tunnel": {
                "unit": "deputyos-tunnel.service",
                "active_state": tunnel_state,
                "enabled": tunnel_enabled,
                "on_demand": true,
                "token_path": "/etc/deputyos/tunnel-token",
            },
            "uptime_seconds": 0,
            "cost_tripped": cost::is_tripped(),
            "doctor_fails": fails,
        });
        println!("{}", serde_json::to_string_pretty(&payload)?);
        return Ok(0);
    }

    if !systemd_available {
        eprintln!("systemd not available on this platform");
        return Ok(1);
    }
    println!("profile:  {id}");
    println!("unit:     {}", m.service.unit);
    println!("state:    {state}");
    println!("tunnel:   {tunnel_state} (on-demand)");
    println!("doctor:   {fails} failing check(s)");
    println!();
    let _ = systemd::status_print(&m.service.unit);
    // Surface non-zero if the unit is not active even if doctor passes.
    Ok(if state == "active" { 0 } else { 1 })
}

fn cmd_logs(follow: bool) -> Result<u8> {
    if !systemd::available() {
        eprintln!("systemd not available on this platform");
        return Ok(1);
    }
    let (_id, m) = match profile::load_active() {
        Ok(x) => x,
        Err(e) => {
            eprintln!("deputyctl logs: {e}");
            return Ok(1);
        }
    };
    let unit = &m.health.journal_unit;
    let status = systemd::journal(unit, follow)?;
    Ok(if status.success() { 0 } else { 1 })
}

// ---------------------------------------------------------------------------
// profile validate
// ---------------------------------------------------------------------------

fn cmd_profile_validate(paths: Vec<PathBuf>, json: bool) -> Result<u8> {
    let total = paths.len();
    let mut ok_count = 0usize;
    let mut results = Vec::with_capacity(total);
    for path in &paths {
        let errs = validate::validate_profile_file(path);
        let ok = errs.is_empty();
        if ok {
            ok_count += 1;
        }
        if !json {
            for e in &errs {
                eprintln!("{}: {}: {}", path.display(), e.field, e.reason);
            }
        }
        results.push(validate::FileResult {
            path: path.clone(),
            ok,
            errors: errs,
        });
    }
    if json {
        let payload = serde_json::json!({ "results": results });
        println!("{}", serde_json::to_string_pretty(&payload)?);
    } else {
        println!("validated: {ok_count}/{total}");
    }
    Ok(if ok_count == total { 0 } else { 1 })
}

// ---------------------------------------------------------------------------
// model list
// ---------------------------------------------------------------------------

fn cmd_model_list(json: bool) -> Result<u8> {
    let statuses = model::list_status()?;
    let is_airgap = model::airgap_active();
    if json {
        let payload = serde_json::json!({
            "airgap": is_airgap,
            "providers": statuses,
        });
        println!("{}", serde_json::to_string_pretty(&payload)?);
    } else if statuses.is_empty() {
        eprintln!("no providers in catalogue (this should never happen)");
        return Ok(1);
    } else {
        if is_airgap {
            println!("airgap mode: active (cloud providers may be inaccessible)");
            println!();
        }
        println!(
            "{:<24} {:<36} {:<22} status",
            "id", "display_name", "key_env_var"
        );
        for s in &statuses {
            let status = if s.configured {
                "configured"
            } else {
                "not configured"
            };
            println!(
                "{:<24} {:<36} {:<22} {status}",
                s.id, s.display_name, s.key_env_var,
            );
        }
    }
    Ok(0)
}

// ---------------------------------------------------------------------------
// update
// ---------------------------------------------------------------------------

fn cmd_update(check: bool, apply: bool, yes: bool, json: bool, from: Option<&str>) -> Result<u8> {
    if check && apply {
        eprintln!("deputyctl update: --check and --apply are mutually exclusive");
        return Ok(EX_USAGE);
    }
    if !check && !apply {
        eprintln!("deputyctl update: pass --check to inspect, --apply to stage an update");
        return Ok(EX_USAGE);
    }
    if check {
        update::run_check(json, from)
    } else {
        update::run_apply(yes, json, from)
    }
}

// ---------------------------------------------------------------------------
// init — spawn the deputywizard binary
// ---------------------------------------------------------------------------

fn cmd_init() -> Result<u8> {
    use std::process::Command as PCommand;

    // Locate the deputywizard binary. PATH first; then dev fallbacks under
    // target/{debug,release}/deputywizard so `cargo run -- init` works
    // without an install step.
    let candidate = which_deputywizard();
    let exe = match candidate {
        Some(p) => p,
        None => {
            eprintln!(
                "deputyctl init: deputywizard binary not found on PATH or in target/{{debug,release}}.\n\
                 Run `cargo build -p deputywizard` first, or install the deputywizard package."
            );
            return Ok(1);
        }
    };

    eprintln!("deputyctl init: starting wizard via {}", exe.display());
    eprintln!("deputyctl init: open http://localhost:8088/wizard once the URL is printed below.");
    let status = PCommand::new(&exe)
        .args(["serve", "--port", "8088"])
        .status();
    match status {
        Ok(s) if s.success() => Ok(0),
        Ok(s) => Ok(s.code().unwrap_or(1) as u8),
        Err(e) => {
            eprintln!("deputyctl init: failed to spawn deputywizard: {e}");
            Ok(1)
        }
    }
}

fn which_deputywizard() -> Option<PathBuf> {
    // PATH lookup.
    if let Ok(path) = std::env::var("PATH") {
        for dir in std::env::split_paths(&path) {
            let cand = dir.join("deputywizard");
            if cand.is_file() {
                return Some(cand);
            }
        }
    }
    // Dev fallbacks relative to the current working directory.
    for rel in ["target/debug/deputywizard", "target/release/deputywizard"] {
        let p = PathBuf::from(rel);
        if p.is_file() {
            return Some(p);
        }
    }
    None
}
