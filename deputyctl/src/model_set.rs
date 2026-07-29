//! `deputyctl model set` — interactive provider/key picker.
//!
//! Persists the chosen provider id to `/etc/deputyos/active-provider` and
//! the key to `/etc/deputyos/secrets.env` (mode 0600). Both file paths are
//! overridable via env vars (see `paths.rs`) so contributors can exercise
//! the flow against a tempdir.

use std::io::{BufRead, IsTerminal, Write};

use anyhow::{anyhow, Context, Result};

use crate::model;

#[derive(Debug, Clone, Default)]
pub struct SetOpts {
    pub provider: Option<String>,
    pub key_from_stdin: bool,
    pub yes: bool,
}

pub fn run(opts: SetOpts) -> Result<u8> {
    let catalogue = model::load_providers().context("loading providers catalogue")?;
    if catalogue.providers.is_empty() {
        eprintln!("model set: no providers in catalogue (this should never happen)");
        return Ok(1);
    }

    // 1. Resolve provider id.
    let provider_id = match &opts.provider {
        Some(id) => {
            if model::find_provider(&catalogue, id).is_none() {
                eprintln!(
                    "model set: unknown provider '{id}' (run `deputyctl model list` for valid ids)"
                );
                return Ok(64);
            }
            id.clone()
        }
        None => match prompt_provider(&catalogue)? {
            Some(id) => id,
            None => {
                eprintln!("model set: aborted");
                return Ok(1);
            }
        },
    };
    let provider = model::find_provider(&catalogue, &provider_id)
        .ok_or_else(|| anyhow!("provider {provider_id} disappeared mid-flight"))?
        .clone();

    // 2. Resolve key.
    let key = if opts.key_from_stdin {
        read_stdin_line()?
    } else if std::io::stdin().is_terminal() {
        let prompt = format!(
            "enter key for {} ({}): ",
            provider.display_name, provider.key_env_var
        );
        rpassword::prompt_password(prompt).context("reading masked key from TTY")?
    } else {
        eprintln!(
            "model set: stdin is not a TTY; pass --key-from-stdin to supply the key non-interactively"
        );
        return Ok(64);
    };
    let key = key.trim().to_string();
    if key.is_empty() {
        eprintln!("model set: empty key — aborting");
        return Ok(1);
    }

    // 3. Confirm (skip with --yes).
    if !opts.yes && std::io::stdin().is_terminal() {
        eprint!(
            "set provider \"{}\" with {} key (length {})? [y/N] ",
            provider.id,
            provider.key_env_var,
            key.len()
        );
        let _ = std::io::stderr().flush();
        let mut buf = String::new();
        std::io::stdin().read_line(&mut buf)?;
        if !matches!(buf.trim(), "y" | "Y" | "yes" | "Yes") {
            eprintln!("model set: aborted");
            return Ok(1);
        }
    }

    // 4. Persist.
    model::write_secret(&provider.key_env_var, &key)
        .with_context(|| format!("writing {} to secrets.env", provider.key_env_var))?;
    model::write_active_provider(&provider.id)
        .with_context(|| format!("writing active-provider {}", provider.id))?;

    println!(
        "configured: provider={} key_env={} (run `deputyctl model test` to round-trip)",
        provider.id, provider.key_env_var
    );
    Ok(0)
}

fn prompt_provider(cat: &model::ProvidersFile) -> Result<Option<String>> {
    if !std::io::stdin().is_terminal() {
        eprintln!(
            "model set: stdin is not a TTY; pass --provider <id> to select non-interactively"
        );
        return Ok(None);
    }
    println!("available providers:");
    for (i, p) in cat.providers.iter().enumerate() {
        println!(
            "  {:>2}. {:<22} {} ({})",
            i + 1,
            p.id,
            p.display_name,
            p.key_env_var
        );
    }
    print!("pick a number (or blank to abort): ");
    std::io::stdout().flush()?;
    let mut buf = String::new();
    std::io::stdin().read_line(&mut buf)?;
    let trimmed = buf.trim();
    if trimmed.is_empty() {
        return Ok(None);
    }
    match trimmed.parse::<usize>() {
        Ok(n) if n >= 1 && n <= cat.providers.len() => Ok(Some(cat.providers[n - 1].id.clone())),
        _ => {
            eprintln!("invalid selection: {trimmed}");
            Ok(None)
        }
    }
}

fn read_stdin_line() -> Result<String> {
    let mut line = String::new();
    let stdin = std::io::stdin();
    let mut handle = stdin.lock();
    handle.read_line(&mut line)?;
    Ok(line)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::path::PathBuf;

    fn providers_path() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("etc")
            .join("providers.json")
    }

    fn stage_env(dir: &tempfile::TempDir) {
        let secrets = dir.path().join("secrets.env");
        let active = dir.path().join("active-provider");
        std::env::set_var("DEPUTYOS_SECRETS_FILE", &secrets);
        std::env::set_var("DEPUTYOS_ACTIVE_PROVIDER_FILE", &active);
        std::env::set_var("DEPUTYOS_PROVIDERS_FILE", providers_path());
    }

    fn cleanup_env() {
        std::env::remove_var("DEPUTYOS_SECRETS_FILE");
        std::env::remove_var("DEPUTYOS_ACTIVE_PROVIDER_FILE");
        std::env::remove_var("DEPUTYOS_PROVIDERS_FILE");
    }

    /// Unknown provider id → exit 64.
    #[test]
    fn unknown_provider_rejected() {
        let _g = crate::env_mutex().lock().unwrap_or_else(|e| e.into_inner());
        let dir = tempfile::tempdir().expect("tempdir");
        stage_env(&dir);
        let code = run(SetOpts {
            provider: Some("nonsense".into()),
            key_from_stdin: true,
            yes: true,
        })
        .expect("run");
        assert_eq!(code, 64);
        cleanup_env();
    }

    /// Verify existing keys are preserved when `model set` writes a new one.
    #[test]
    fn existing_keys_preserved_on_subsequent_set() {
        let _g = crate::env_mutex().lock().unwrap_or_else(|e| e.into_inner());
        let dir = tempfile::tempdir().expect("tempdir");
        stage_env(&dir);
        let secrets = dir.path().join("secrets.env");
        let mut f = std::fs::File::create(&secrets).expect("seed");
        writeln!(f, "OTHER_KEY=untouched").expect("write");
        // Use the model::write_secret path directly — same as `model set`
        // happy path; we are not exercising stdin parsing here.
        model::write_secret_to(&secrets, "OPENROUTER_API_KEY", "sk-or-x").expect("write");
        let body = std::fs::read_to_string(&secrets).expect("read");
        assert!(body.contains("OTHER_KEY=untouched"));
        assert!(body.contains("OPENROUTER_API_KEY=sk-or-x"));
        cleanup_env();
    }
}
