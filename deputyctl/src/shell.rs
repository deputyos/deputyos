//! `deputyctl shell` — drop into the active profile's CLI.
//!
//! Per `docs/02-profiles.md`, this exec's the profile binary (with a
//! per-profile sub-arg if known) so `ps` shows the profile process directly,
//! not an `deputyctl` parent. On dev hosts the binary path almost never
//! exists; we surface a clear "requires baked image" error and exit 64.

use std::path::Path;

use anyhow::Result;

use crate::profile;

/// Caller-supplied flags for `shell`.
#[derive(Debug, Clone, Copy, Default)]
pub struct ShellOpts {
    pub dry_run: bool,
}

/// Pick the per-profile sub-arg to append when none is declared in the
/// manifest. Mirrors `docs/02-profiles.md` table.
fn default_subcommand(profile_id: &str) -> Option<&'static str> {
    match profile_id {
        "openclaw" => Some("repl"),
        "hermes" => Some("shell"),
        _ => None,
    }
}

/// Plan of an exec — used for `--dry-run` printout and the real exec path.
struct Plan {
    binary: String,
    args: Vec<String>,
    profile_id: String,
}

fn build_plan() -> Result<Plan> {
    let (id, m) = profile::load_active()?;
    let binary = m.paths.binary.clone();
    let mut args: Vec<String> = Vec::new();
    if let Some(sub) = default_subcommand(&id) {
        args.push(sub.to_string());
    }
    Ok(Plan {
        binary,
        args,
        profile_id: id,
    })
}

pub fn run(opts: ShellOpts) -> Result<u8> {
    let plan = match build_plan() {
        Ok(p) => p,
        Err(e) => {
            eprintln!("deputyctl shell: {e}");
            return Ok(64);
        }
    };

    if opts.dry_run {
        let argv = std::iter::once(plan.binary.clone())
            .chain(plan.args.iter().cloned())
            .collect::<Vec<_>>();
        println!("(dry-run) profile: {}", plan.profile_id);
        println!("(dry-run) exec: {}", argv.join(" "));
        return Ok(0);
    }

    if !Path::new(&plan.binary).exists() {
        eprintln!(
            "deputyctl shell: active profile binary not present at {} — `deputyctl up` requires a baked image",
            plan.binary
        );
        return Ok(64);
    }

    // Replace the deputyctl process so `ps` shows the agent directly.
    do_exec(&plan)
}

#[cfg(unix)]
fn do_exec(plan: &Plan) -> Result<u8> {
    use std::os::unix::process::CommandExt;
    let err = std::process::Command::new(&plan.binary)
        .args(&plan.args)
        .exec();
    // exec only returns on failure.
    eprintln!("deputyctl shell: exec({}) failed: {err}", plan.binary);
    Ok(1)
}

#[cfg(not(unix))]
fn do_exec(plan: &Plan) -> Result<u8> {
    let status = std::process::Command::new(&plan.binary)
        .args(&plan.args)
        .status()?;
    Ok(status.code().unwrap_or(1) as u8)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write as _;

    fn stage_profile(dir: &tempfile::TempDir, id: &str, binary: &str) {
        let pdir = dir.path().join("profiles");
        std::fs::create_dir_all(&pdir).expect("mkdir");
        let manifest = format!(
            r#"
[profile]
id = "{id}"
display_name = "Test"
upstream_repo = "test/test"
release_channel = "stable"
min_ram_mb = 1024
pinned_version = "0.0.0"

[paths]
install_root = "/opt/test"
data_dir = "/tmp/test-data"
binary = "{binary}"

[runtime]
language = "node"
package_manager = "npm"

[service]
unit = "test.service"
entrypoint = "test"
ports = [9999]

[health]
http_check = "http://127.0.0.1:9999/healthz"
journal_unit = "test.service"
startup_grace_s = 30
"#
        );
        std::fs::write(pdir.join(format!("{id}.toml")), manifest).expect("write manifest");
        let active = dir.path().join("active-profile");
        let mut f = std::fs::File::create(&active).expect("active");
        writeln!(f, "{id}").expect("write");
        std::env::set_var("DEPUTYOS_PROFILES_DIR", &pdir);
        std::env::set_var("DEPUTYOS_ACTIVE_PROFILE_FILE", &active);
    }

    fn cleanup() {
        std::env::remove_var("DEPUTYOS_PROFILES_DIR");
        std::env::remove_var("DEPUTYOS_ACTIVE_PROFILE_FILE");
    }

    #[test]
    fn dry_run_prints_planned_exec() {
        let _g = crate::env_mutex().lock().unwrap_or_else(|e| e.into_inner());
        let dir = tempfile::tempdir().expect("tempdir");
        stage_profile(&dir, "openclaw", "/opt/test/openclaw-bin");
        let code = run(ShellOpts { dry_run: true }).expect("run");
        assert_eq!(code, 0);
        cleanup();
    }

    #[test]
    fn missing_binary_exits_64() {
        let _g = crate::env_mutex().lock().unwrap_or_else(|e| e.into_inner());
        let dir = tempfile::tempdir().expect("tempdir");
        stage_profile(&dir, "openclaw", "/nonexistent/agent-binary");
        let code = run(ShellOpts { dry_run: false }).expect("run");
        assert_eq!(code, 64);
        cleanup();
    }
}
