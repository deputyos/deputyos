//! `deputyos-desktop` binary entry point.
//!
//! Surface (per `docs/11-roadmap.md` § M2.5):
//!
//! ```text
//! deputyos-desktop install                 # prereq + download + cache + import
//! deputyos-desktop start                   # boot VM + open browser. Default action.
//! deputyos-desktop stop                    # graceful shutdown
//! deputyos-desktop status                  # running/stopped + URL
//! deputyos-desktop update                  # check manifest + download/swap the image if newer
//! deputyos-desktop self-update             # download + verify + replace THIS launcher binary
//! deputyos-desktop uninstall [--data]      # remove cache; --data also wipes data dir
//! ```
//!
//! Invoking with **no subcommand** mimics double-click: install if needed,
//! start, then open the wizard URL in the default browser. This is the
//! "non-technical user double-clicks the icon" path.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};

use deputyos_desktop::config;
use deputyos_desktop::driver::{current_driver, Driver, VmStatus};
use deputyos_desktop::runtime::InstanceOps;
use deputyos_desktop::ResourceSpec;
use deputyos_desktop::{browser, download, manifest, selfupdate};

#[derive(Parser, Debug)]
#[command(
    name = "deputyos-desktop",
    version,
    about = "One-click desktop launcher for deputyOS. Mandates the host's native virtualization (qemu+KVM on Linux, WSL2 on Windows, UTM on macOS) — no bundled hypervisor.",
    long_about = "When invoked without a subcommand (e.g. by double-clicking), \
                  installs (if needed), starts the VM, and opens the wizard \
                  in the default browser."
)]
struct Cli {
    #[command(subcommand)]
    cmd: Option<Cmd>,
}

#[derive(Subcommand, Debug)]
enum Cmd {
    /// Verify prerequisites, fetch the latest signed manifest, download +
    /// verify the image for this host, and stage it in the cache. When the
    /// manifest offers more than one image for this host's target, `--profile`
    /// selects which; omitting it lists the options and exits.
    Install {
        /// Which agent profile to install (e.g. `hermes`, `openclaw`).
        /// Required when the manifest lists more than one image for this
        /// host's target.
        #[arg(long)]
        profile: Option<String>,
    },
    /// Boot the VM and open the wizard in the default browser. Idempotent.
    Start,
    /// Cooperatively quiesce workloads and pause the default instance.
    Pause,
    /// Resume the default instance and thaw its workloads.
    Resume,
    /// Change the default instance's live memory balloon target.
    Memory {
        /// Guest-visible memory target in MiB.
        target_mib: u64,
    },
    /// Send SIGTERM (or platform equivalent) to the running VM.
    Stop,
    /// Print "running" or "stopped" and the wizard URL.
    Status,
    /// Print the current platform backend's lifecycle/resource capabilities.
    Capabilities,
    /// Manage named deputy instances.
    Instance {
        #[command(subcommand)]
        command: InstanceCommand,
    },
    /// Check the manifest and, if newer, download + verify + swap the VM
    /// image. Prints a hint if a newer launcher binary is also available
    /// (use `self-update` to replace the launcher itself).
    Update,
    /// Download + verify + atomically replace THIS launcher binary from the
    /// manifest's `desktop_launchers[<host-triple>]` entry. The VM image is
    /// untouched. The new launcher takes effect on the next launch.
    SelfUpdate,
    /// Remove the cached image.
    Uninstall {
        /// Also delete the persistent data directory (PID file, etc.).
        #[arg(long)]
        data: bool,
    },
}

#[derive(Subcommand, Debug)]
enum InstanceCommand {
    List,
    Create {
        name: String,
        #[arg(long)]
        profile: Option<String>,
        #[arg(long)]
        vcpus: Option<u16>,
        #[arg(long)]
        memory_min_mib: Option<u64>,
        #[arg(long)]
        memory_max_mib: Option<u64>,
        #[arg(long, default_value_t = true, action = clap::ArgAction::Set)]
        auto_balloon: bool,
    },
    Install {
        instance: String,
    },
    Start {
        instance: String,
    },
    Pause {
        instance: String,
    },
    Resume {
        instance: String,
    },
    Stop {
        instance: String,
    },
    Status {
        instance: String,
    },
    Health {
        instance: String,
    },
    Memory {
        instance: String,
        target_mib: u64,
    },
    SetResources {
        instance: String,
        #[arg(long)]
        vcpus: Option<u16>,
        #[arg(long)]
        memory_min_mib: Option<u64>,
        #[arg(long)]
        memory_max_mib: Option<u64>,
        #[arg(long, action = clap::ArgAction::Set)]
        auto_balloon: Option<bool>,
    },
    Delete {
        instance: String,
    },
}

fn main() -> Result<()> {
    // Tracing init — opt-in via RUST_LOG; default off so the launcher prints
    // its own clear stderr lines without a logger prefix in normal use.
    let _ = tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .with_writer(std::io::stderr)
        .try_init();

    let cli = Cli::parse();
    let driver = current_driver();

    match cli.cmd {
        Some(Cmd::Install { profile }) => cmd_install(driver.as_ref(), profile.as_deref()),
        Some(Cmd::Start) => cmd_start(driver.as_ref()),
        Some(Cmd::Pause) => driver.pause().context("pausing VM"),
        Some(Cmd::Resume) => {
            driver.resume().context("resuming VM")?;
            Ok(())
        }
        Some(Cmd::Memory { target_mib }) => driver
            .set_memory(target_mib)
            .context("setting live memory target"),
        Some(Cmd::Stop) => cmd_stop(driver.as_ref()),
        Some(Cmd::Status) => cmd_status(driver.as_ref()),
        Some(Cmd::Capabilities) => {
            println!("{}", serde_json::to_string_pretty(&driver.capabilities())?);
            Ok(())
        }
        Some(Cmd::Instance { command }) => cmd_instance(command),
        Some(Cmd::Update) => cmd_update(),
        Some(Cmd::SelfUpdate) => cmd_selfupdate(),
        Some(Cmd::Uninstall { data }) => cmd_uninstall(data),
        None => cmd_default(driver.as_ref()),
    }
}

fn cmd_instance(command: InstanceCommand) -> Result<()> {
    let ops = InstanceOps::new();
    match command {
        InstanceCommand::List => {
            println!("{}", serde_json::to_string_pretty(&ops.list()?)?);
        }
        InstanceCommand::Create {
            name,
            profile,
            vcpus,
            memory_min_mib,
            memory_max_mib,
            auto_balloon,
        } => {
            let created = ops.create(&name, profile, None, None)?;
            let mut resources = created.resources;
            resources.vcpus = vcpus.unwrap_or(resources.vcpus);
            resources.memory_min_mib = memory_min_mib.unwrap_or(resources.memory_min_mib);
            resources.memory_max_mib = memory_max_mib.unwrap_or(resources.memory_max_mib);
            resources.auto_balloon = auto_balloon;
            let created = if resources != created.resources {
                ops.configure_resources(&created.id, resources)?
            } else {
                created
            };
            println!("{}", serde_json::to_string_pretty(&created)?);
        }
        InstanceCommand::Install { instance } => ops.install(&instance)?,
        InstanceCommand::Start { instance } => {
            let url = ops.start(&instance)?;
            println!("{url}");
        }
        InstanceCommand::Pause { instance } => ops.pause(&instance)?,
        InstanceCommand::Resume { instance } => {
            let url = ops.resume(&instance)?;
            println!("{url}");
        }
        InstanceCommand::Stop { instance } => ops.stop(&instance)?,
        InstanceCommand::Status { instance } => {
            println!("{}", serde_json::to_string_pretty(&ops.status(&instance)?)?);
        }
        InstanceCommand::Health { instance } => {
            println!(
                "{}",
                serde_json::to_string_pretty(&ops.agent_health(&instance)?)?
            );
        }
        InstanceCommand::Memory {
            instance,
            target_mib,
        } => ops.set_memory(&instance, target_mib)?,
        InstanceCommand::SetResources {
            instance,
            vcpus,
            memory_min_mib,
            memory_max_mib,
            auto_balloon,
        } => {
            let current = find_instance(&ops, &instance)?;
            let resources = ResourceSpec {
                vcpus: vcpus.unwrap_or(current.resources.vcpus),
                memory_min_mib: memory_min_mib.unwrap_or(current.resources.memory_min_mib),
                memory_max_mib: memory_max_mib.unwrap_or(current.resources.memory_max_mib),
                auto_balloon: auto_balloon.unwrap_or(current.resources.auto_balloon),
            };
            let updated = ops.configure_resources(&instance, resources)?;
            println!("{}", serde_json::to_string_pretty(&updated)?);
        }
        InstanceCommand::Delete { instance } => ops.delete(&instance)?,
    }
    Ok(())
}

fn find_instance(ops: &InstanceOps, selector: &str) -> Result<deputyos_desktop::Instance> {
    ops.list()?
        .into_iter()
        .find(|instance| instance.id == selector || instance.name == selector)
        .ok_or_else(|| anyhow::anyhow!("no instance matching {selector:?}"))
}

fn cmd_install(driver: &dyn Driver, profile: Option<&str>) -> Result<()> {
    eprintln!("==> deputyos-desktop install");

    // 1. Prereq must pass before we touch the network.
    driver
        .check_prereq()
        .context("platform prerequisite missing")?;

    // 2. Fetch + sig-verify manifest.
    let manifest_url = config::manifest_url();
    eprintln!("==> fetching manifest: {manifest_url}");
    let pubkey = config::pubkey_path();
    let src = manifest::fetch_and_verify(&manifest_url, &pubkey)?;

    // 3. Pick artefact for this host. When the manifest offers more than one
    //    image for this target and the caller didn't pass --profile, list the
    //    options and bail rather than silently picking the first one.
    let target = driver.target_for_host();
    let candidates = manifest::artefacts_for_target(&src, target);
    let artefact = if profile.is_none() && candidates.len() > 1 {
        eprintln!(
            "Multiple images available for {target} (release {}, channel {}):",
            src.manifest.release_version, src.manifest.channel
        );
        for (i, a) in candidates.iter().enumerate() {
            eprintln!(
                "  {}) {:<10} {:.2} GB",
                i + 1,
                a.profile,
                a.size_bytes as f64 / 1e9
            );
        }
        eprintln!("Select with: deputyos-desktop install --profile <id>");
        anyhow::bail!("multiple images available for target '{target}'; specify --profile <id>");
    } else {
        manifest::pick_artefact(&src, target, profile)?
    };
    eprintln!(
        "==> selected artefact: {} ({} bytes)",
        artefact.filename, artefact.size_bytes
    );

    // 4. Download + sha + sig.
    let (img_url, sig_url) = manifest::artefact_urls(&src, artefact)?;
    let cache = config::cache_dir();
    std::fs::create_dir_all(&cache)
        .with_context(|| format!("creating cache dir {}", cache.display()))?;
    let img_dest = cached_artefact_path(target, &artefact.filename);
    let sig_dest = img_dest.with_extension(format!(
        "{}.minisig",
        img_dest
            .extension()
            .and_then(|s| s.to_str())
            .unwrap_or("artefact")
    ));
    download::download_and_verify(
        &img_url,
        &sig_url,
        &img_dest,
        &sig_dest,
        &artefact.sha256,
        &pubkey,
    )?;

    // 5. Hand to driver for any platform-specific import (no-op on Linux).
    driver.install_image(&img_dest)?;
    write_installed_record(&src, artefact)?;

    eprintln!("==> install complete");
    Ok(())
}

fn cmd_start(driver: &dyn Driver) -> Result<()> {
    eprintln!("==> deputyos-desktop start");
    driver.check_prereq().context("prereq check failed")?;
    let _handle = driver.start().context("starting VM")?;
    let url = config::wizard_url();
    browser::open_url(&url).ok();
    eprintln!("==> wizard at {url}");
    Ok(())
}

fn cmd_stop(driver: &dyn Driver) -> Result<()> {
    eprintln!("==> deputyos-desktop stop");
    driver.stop().context("stopping VM")
}

fn cmd_status(driver: &dyn Driver) -> Result<()> {
    match driver.status()? {
        VmStatus::Running { handle, urls } => {
            println!("running (id={})", handle.id);
            for u in urls {
                println!("  {u}");
            }
        }
        VmStatus::Stopped => {
            println!("stopped");
        }
        VmStatus::Paused { handle } => {
            println!("paused (id {})", handle.id);
        }
    }
    Ok(())
}

fn cmd_update() -> Result<()> {
    let manifest_url = config::manifest_url();

    let pubkey = config::pubkey_path();
    let embedded = config::embedded_pubkey();

    // If embedded pubkey is available, write it to a temp file so the
    // existing fetch_and_verify(path) API works.
    let _embedded_file;
    let pubkey_ref: &Path = if let Some(key_path) = embedded {
        _embedded_file = std::env::temp_dir().join("deputyos-desktop-embedded.pub");
        std::fs::write(&_embedded_file, key_path)
            .context("writing embedded pubkey to temp file")?;
        &_embedded_file
    } else {
        pubkey.as_path()
    };

    let src = crate::manifest::fetch_and_verify(&manifest_url, pubkey_ref)?;

    println!(
        "latest: {} (channel={})",
        src.manifest.release_version, src.manifest.channel
    );

    // Check if we already have this version installed.
    let last_path = config::last_manifest_path();
    if last_path.is_file() {
        let last_raw = std::fs::read_to_string(&last_path).unwrap_or_default();
        if let Ok(last) = serde_json::from_str::<serde_json::Value>(&last_raw) {
            if let Some(ver) = last.get("release_version").and_then(|v| v.as_str()) {
                if ver == src.manifest.release_version {
                    println!("up to date (version {ver})");
                    print_launcher_self_update_hint(&src);
                    return Ok(());
                }
            }
        }
    }

    // Pick the right artefact for this host. Update targets the profile the
    // user already installed (read from the last-manifest record) so a
    // hermes install updates hermes, not whichever profile sorts first.
    let driver = crate::current_driver();
    let target = driver.target_for_host();
    let installed_profile = read_installed_profile();
    let artefact = crate::manifest::pick_artefact(&src, target, installed_profile.as_deref())?;

    let (img_url, sig_url) = crate::manifest::artefact_urls(&src, artefact)?;
    let staging = staging_artefact_path(target, &artefact.filename);
    let sig_staging = staging.with_extension(format!(
        "{}.minisig",
        staging
            .extension()
            .and_then(|s| s.to_str())
            .unwrap_or("artefact")
    ));
    let cached = cached_artefact_path(target, &artefact.filename);

    println!("downloading {} ...", artefact.filename);
    crate::download::download_and_verify(
        &img_url,
        &sig_url,
        &staging,
        &sig_staging,
        &artefact.sha256,
        pubkey_ref,
    )?;

    // Atomically swap cached image.
    if cached.exists() {
        std::fs::remove_file(&cached)
            .with_context(|| format!("removing old {}", cached.display()))?;
    }
    std::fs::rename(&staging, &cached)
        .with_context(|| format!("renaming {} -> {}", staging.display(), cached.display()))?;

    // Record the new version.
    write_installed_record(&src, artefact)?;

    println!("update ready — run `deputyos-desktop start` to boot the new image");
    print_launcher_self_update_hint(&src);
    Ok(())
}

/// Print a one-line hint if the manifest advertises a newer launcher binary
/// than the one running. Best-effort: never fails the caller (an empty or
/// triple-less `desktop_launchers` map, or an unresolvable host triple, is
/// silently ignored — the image update must not fail because of a launcher
/// side-note). The actual self-replace lives in `cmd_selfupdate`.
fn print_launcher_self_update_hint(src: &deputyctl::release::ManifestSource) {
    let Some(triple) = config::host_triple() else {
        return;
    };
    if let Ok(Some(_)) = crate::selfupdate::check(src, triple) {
        println!(
            "hint: a newer launcher binary is available; run `deputyos-desktop self-update` to replace it."
        );
    }
}

fn cmd_selfupdate() -> Result<()> {
    eprintln!("==> deputyos-desktop self-update");

    let manifest_url = config::manifest_url();

    let triple = config::host_triple().ok_or_else(|| {
        anyhow::anyhow!(
            "this host ({} {}) has no known launcher target triple; self-update is unavailable",
            std::env::consts::OS,
            std::env::consts::ARCH
        )
    })?;

    let pubkey = config::pubkey_path();
    let embedded = config::embedded_pubkey();

    // Reuse the same embedded-pubkey-to-tempfile dance cmd_update uses so
    // fetch_and_verify(path) works whether or not the pubkey is embedded.
    let _embedded_file;
    let pubkey_ref: &Path = if let Some(key_path) = embedded {
        _embedded_file = std::env::temp_dir().join("deputyos-desktop-embedded.pub");
        std::fs::write(&_embedded_file, key_path)
            .context("writing embedded pubkey to temp file")?;
        &_embedded_file
    } else {
        pubkey.as_path()
    };

    let src = manifest::fetch_and_verify(&manifest_url, pubkey_ref)?;

    let launcher = match crate::selfupdate::check(&src, triple)? {
        None => {
            println!("launcher up to date");
            return Ok(());
        }
        Some(l) => l,
    };

    crate::selfupdate::apply(launcher, &src, pubkey_ref)?;
    Ok(())
}

fn write_installed_record(
    src: &deputyctl::release::ManifestSource,
    artefact: &deputyctl::release::Artefact,
) -> Result<()> {
    use std::time::SystemTime;
    let ts = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);

    let record = serde_json::json!({
        "release_version": src.manifest.release_version,
        "channel": src.manifest.channel,
        "filename": artefact.filename,
        "target": artefact.target,
        "profile": artefact.profile,
        "format": artefact.format,
        "sha256": artefact.sha256,
        "updated_at_unix": ts,
    });
    let last_path = config::last_manifest_path();
    if let Some(parent) = last_path.parent() {
        std::fs::create_dir_all(parent).ok();
    }
    std::fs::write(&last_path, serde_json::to_string_pretty(&record)?)
        .context("writing last-manifest.json")?;

    Ok(())
}

/// Read the profile id of the previously-installed image from the
/// last-manifest record, if any. Used by `update` to target the same profile
/// the user already chose (so a hermes install updates hermes, not whichever
/// profile happens to sort first in a multi-image manifest). Returns `None`
/// when there's no record or it predates the `profile` field.
fn read_installed_profile() -> Option<String> {
    let last_path = config::last_manifest_path();
    let raw = std::fs::read_to_string(&last_path).ok()?;
    let v: serde_json::Value = serde_json::from_str(&raw).ok()?;
    v.get("profile")?.as_str().map(str::to_string)
}

fn cached_artefact_path(target: &str, filename: &str) -> PathBuf {
    config::cache_dir().join(format!("deputyos-{target}.{}", artefact_suffix(filename)))
}

fn staging_artefact_path(target: &str, filename: &str) -> PathBuf {
    config::cache_dir().join(format!(
        "deputyos-{target}-staging.{}",
        artefact_suffix(filename)
    ))
}

fn artefact_suffix(filename: &str) -> String {
    let lower = filename.to_ascii_lowercase();
    if lower.ends_with(".tar.gz") {
        return "tar.gz".to_string();
    }
    Path::new(filename)
        .extension()
        .and_then(|s| s.to_str())
        .filter(|s| !s.is_empty())
        .unwrap_or("img")
        .to_string()
}

fn cmd_uninstall(also_data: bool) -> Result<()> {
    eprintln!("==> deputyos-desktop uninstall");
    let cache = config::cache_dir();
    if cache.exists() {
        std::fs::remove_dir_all(&cache)
            .with_context(|| format!("removing cache {}", cache.display()))?;
        eprintln!("==> removed cache: {}", cache.display());
    } else {
        eprintln!("==> no cache to remove at {}", cache.display());
    }
    if also_data {
        let data = config::data_dir();
        if data.exists() {
            std::fs::remove_dir_all(&data)
                .with_context(|| format!("removing data {}", data.display()))?;
            eprintln!("==> removed data: {}", data.display());
        }
        let runtime = config::runtime_dir();
        if runtime.exists() && runtime != data {
            // Best-effort; runtime dir is often /run/user/<uid> which we
            // don't want to delete recursively. Just remove the
            // deputyos-desktop subdir.
            let _ = std::fs::remove_dir_all(&runtime);
        }
    }
    Ok(())
}

/// Default action when invoked with no subcommand.
fn cmd_default(driver: &dyn Driver) -> Result<()> {
    eprintln!("==> deputyos-desktop (no subcommand) — install-if-needed + start + open browser");
    let install_record = config::last_manifest_path();
    if !install_record.exists() {
        cmd_install(driver, None)?;
    } else {
        eprintln!(
            "==> install already complete at {}",
            install_record.display()
        );
    }
    cmd_start(driver)
}

// Silence unused-import warning in tools that only call the library API.
#[allow(dead_code)]
fn _silence_pathbuf() -> PathBuf {
    PathBuf::new()
}
