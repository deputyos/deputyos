//! Integration tests for `deputyctl model list` data plumbing.
//!
//! Network and key-validation round-trips land in M2 with `model set` and
//! `model test`. Here we just guarantee:
//! - the baked `providers.json` parses and is non-trivial,
//! - the secrets parser handles a missing file gracefully,
//! - the parser correctly extracts plain, quoted, and `export`-prefixed lines.

use std::path::PathBuf;

use deputyctl::model::{load_providers_from, load_secrets_from};

fn providers_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("etc")
        .join("providers.json")
}

#[test]
fn providers_json_parses() {
    let p = load_providers_from(&providers_path()).expect("providers.json must parse");
    assert!(
        p.providers.len() >= 10,
        "expected at least 10 providers, got {}",
        p.providers.len(),
    );
    // Every provider must have non-empty id, display_name, key_env_var.
    for prov in &p.providers {
        assert!(!prov.id.is_empty(), "empty id");
        assert!(
            !prov.display_name.is_empty(),
            "empty display_name in {}",
            prov.id
        );
        assert!(
            !prov.key_env_var.is_empty(),
            "empty key_env_var in {}",
            prov.id
        );
    }
    // OpenRouter is the documented recommended path; sanity-check it's present.
    assert!(p.providers.iter().any(|x| x.id == "openrouter"));
}

#[test]
fn model_list_handles_missing_secrets() {
    let nonexistent = PathBuf::from("/tmp/deputyos-this-file-does-not-exist-xyz");
    let secrets = load_secrets_from(&nonexistent);
    assert!(secrets.is_empty());
}

#[test]
fn secrets_parser_handles_quoted_and_export_lines() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("secrets.env");
    let body = r#"# a comment
PLAIN=plainval
QUOTED="quoted val"
SINGLE='singleq'
export EXPORTED=exp1
EMPTY=
"#;
    std::fs::write(&path, body).expect("write secrets");
    let parsed = load_secrets_from(&path);
    assert_eq!(parsed.get("PLAIN").map(String::as_str), Some("plainval"));
    assert_eq!(parsed.get("QUOTED").map(String::as_str), Some("quoted val"));
    assert_eq!(parsed.get("SINGLE").map(String::as_str), Some("singleq"));
    assert_eq!(parsed.get("EXPORTED").map(String::as_str), Some("exp1"));
    assert_eq!(parsed.get("EMPTY").map(String::as_str), Some(""));
}

// ---------------------------------------------------------------------------
// M4.5 Lane A test gap: the airgap model functions (`airgap_active`,
// `load_airgap_models`, `register_gguf`) had no coverage because every path
// they touched was hardcoded under /opt + /etc. The paths.rs refactor added
// `DEPUTYOS_AIRGAP_*` env overrides, so these are now hermetic. We never enable
// a systemd unit here (enable=false), so no systemctl shell-out.

fn env_lock() -> &'static std::sync::Mutex<()> {
    static M: std::sync::OnceLock<std::sync::Mutex<()>> = std::sync::OnceLock::new();
    M.get_or_init(|| std::sync::Mutex::new(()))
}

// Caller MUST hold the env_lock() guard for the whole test body — env vars are
// process-global, so without a held guard parallel tests clobber each other's
// DEPUTYOS_AIRGAP_*. These helpers just set/unset; they do not lock.
fn airgap_env(
    dir: &tempfile::TempDir,
) -> (
    std::path::PathBuf, // flag file
    std::path::PathBuf, // catalog file
    std::path::PathBuf, // models dir
) {
    let flag = dir.path().join("airgap.flag");
    let catalog = dir.path().join("models").join("catalog.json");
    let models_dir = dir.path().join("gguf");
    std::env::set_var("DEPUTYOS_AIRGAP_FLAG", &flag);
    std::env::set_var("DEPUTYOS_AIRGAP_CATALOG", &catalog);
    std::env::set_var("DEPUTYOS_AIRGAP_MODELS_DIR", &models_dir);
    (flag, catalog, models_dir)
}

fn airgap_cleanup() {
    std::env::remove_var("DEPUTYOS_AIRGAP_FLAG");
    std::env::remove_var("DEPUTYOS_AIRGAP_CATALOG");
    std::env::remove_var("DEPUTYOS_AIRGAP_MODELS_DIR");
}

#[test]
fn airgap_active_reflects_flag_file() {
    let _g = env_lock().lock().unwrap_or_else(|e| e.into_inner());
    let dir = tempfile::tempdir().expect("tempdir");
    let (flag, _catalog, _models) = airgap_env(&dir);
    // Absent flag → not active.
    assert!(!deputyctl::model::airgap_active());
    // Create the flag → active.
    std::fs::write(&flag, b"1\n").expect("write flag");
    assert!(deputyctl::model::airgap_active());
    airgap_cleanup();
}

#[test]
fn load_airgap_models_empty_when_catalog_absent() {
    let _g = env_lock().lock().unwrap_or_else(|e| e.into_inner());
    let dir = tempfile::tempdir().expect("tempdir");
    airgap_env(&dir);
    let models = deputyctl::model::load_airgap_models().expect("load");
    assert!(models.is_empty(), "no catalog → no airgap providers");
    airgap_cleanup();
}

#[test]
fn load_airgap_models_parses_baked_catalog_and_synthesises_providers() {
    let _g = env_lock().lock().unwrap_or_else(|e| e.into_inner());
    // Mirrors the exact JSON `tasks/llm-airgap.yml` drops at bake time — note
    // the extra `$schema`, `tier`, `sha256` and `url` keys that the
    // AirgapCatalogEntry struct must silently ignore (no deny_unknown_fields).
    let dir = tempfile::tempdir().expect("tempdir");
    let (_flag, catalog, _models) = airgap_env(&dir);
    std::fs::create_dir_all(catalog.parent().expect("parent")).expect("mkdir");
    let baked = r#"{
  "$schema": "https://www.deputyos.com/schemas/airgap-models-v1.json",
  "tier": "rich",
  "models": [
    {"id": "LFM2-2.6B", "filename": "LFM2-2.6B-Q4_K_M.gguf", "sha256": "deadbeef", "size_mb": 1600, "url": "https://example.invalid/a.gguf", "port": 8090, "default": true},
    {"id": "Qwen2.5-Coder-1.5B", "filename": "Qwen2.5-Coder-1.5B-Instruct-Q4_K_M.gguf", "sha256": "feedface", "size_mb": 1000, "url": "https://example.invalid/b.gguf", "port": 8091, "default": false}
  ]
}"#;
    std::fs::write(&catalog, baked).expect("write catalog");

    let models = deputyctl::model::load_airgap_models().expect("load");
    assert_eq!(models.len(), 2);
    let qwen = models
        .iter()
        .find(|p| p.id == "airgap-qwen2.5-coder-1.5b")
        .expect("qwen synthesised");
    assert_eq!(qwen.kind, "local-llamacpp");
    assert!(qwen.configured, "baked models are always configured");
    assert!(qwen.key_env_var.is_empty(), "airgap needs no API key");
    assert_eq!(qwen.default_model, "Qwen2.5-Coder-1.5B");

    // load_airgap_choices preserves the default flag the wizard pre-selects.
    let choices = deputyctl::model::load_airgap_choices().expect("choices");
    let lfm = choices
        .iter()
        .find(|c| c.model_id == "LFM2-2.6B")
        .expect("lfm choice");
    assert!(lfm.default, "LFM2-2.6B is the catalog default");
    assert_eq!(lfm.id, "airgap-lfm2-2.6b");
    let qwen_c = choices
        .iter()
        .find(|c| c.model_id == "Qwen2.5-Coder-1.5B")
        .expect("qwen choice");
    assert!(!qwen_c.default, "Qwen is not the default");
    airgap_cleanup();
}

#[test]
fn register_gguf_copies_appends_writes_env_and_assigns_distinct_port() {
    let _g = env_lock().lock().unwrap_or_else(|e| e.into_inner());
    let dir = tempfile::tempdir().expect("tempdir");
    let (_flag, catalog, models_dir) = airgap_env(&dir);
    std::fs::create_dir_all(catalog.parent().expect("parent")).expect("mkdir catalog dir");

    // Seed the baked default so the first registered model gets 8091, not 8090.
    let baked = r#"{"models":[{"id":"LFM2-1.2B","filename":"LFM2-1.2B-Q4_K_M.gguf","size_mb":750,"port":8090,"default":true}]}"#;
    std::fs::write(&catalog, baked).expect("seed baked catalog");

    // Stage a source GGUF somewhere outside the models dir.
    let src = dir.path().join("incoming").join("TinyLlama.Q4_K_M.gguf");
    std::fs::create_dir_all(src.parent().expect("parent")).expect("mkdir");
    std::fs::write(&src, b"not really a gguf").expect("stage source");

    // Register without enabling — must NOT shell out to systemctl.
    let msg = deputyctl::model::register_gguf(&src, "TinyLlama", false).expect("register");
    assert!(
        msg.contains("registered in catalog"),
        "enable=false returns a catalog message, not a unit name: {msg}"
    );

    // The GGUF was copied into the models dir under its original filename.
    assert!(models_dir.join("TinyLlama.Q4_K_M.gguf").is_file());

    // The catalog now has two entries and the new one is non-default with port 8091.
    let parsed: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&catalog).expect("read")).expect("parse");
    let models = parsed["models"].as_array().expect("models array");
    assert_eq!(models.len(), 2);
    let tiny = models
        .iter()
        .find(|m| m["id"] == "TinyLlama")
        .expect("tiny in catalog");
    assert_eq!(tiny["port"].as_u64(), Some(8091));
    assert_eq!(tiny["default"].as_bool(), Some(false));

    // The per-instance env file the template reads got both vars.
    let env_path = catalog.parent().expect("parent").join("TinyLlama.env");
    let env_body = std::fs::read_to_string(&env_path).expect("env file written");
    assert!(
        env_body.contains("LLAMACPP_PORT=8091"),
        "env has port, got: {env_body}"
    );
    assert!(
        env_body.contains("LLAMACPP_MODEL_FILE=TinyLlama.Q4_K_M.gguf"),
        "env has model filename, got: {env_body}"
    );

    airgap_cleanup();
}

#[test]
fn register_gguf_is_idempotent() {
    let _g = env_lock().lock().unwrap_or_else(|e| e.into_inner());
    let dir = tempfile::tempdir().expect("tempdir");
    let (_flag, catalog, models_dir) = airgap_env(&dir);
    std::fs::create_dir_all(catalog.parent().expect("parent")).expect("mkdir");
    let src = dir.path().join("a.gguf");
    std::fs::write(&src, b"x").expect("stage");

    deputyctl::model::register_gguf(&src, "Dup", false).expect("first register");
    // A second register of the same id must not duplicate the catalog entry
    // or bump the port.
    deputyctl::model::register_gguf(&src, "Dup", false).expect("second register");
    let parsed: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&catalog).expect("read")).expect("parse");
    let models = parsed["models"].as_array().expect("array");
    assert_eq!(models.len(), 1, "idempotent — no duplicate entry");
    // models dir has exactly one gguf file.
    let ggufs: Vec<_> = std::fs::read_dir(&models_dir)
        .expect("read dir")
        .filter_map(Result::ok)
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n.ends_with(".gguf"))
        .collect();
    assert_eq!(ggufs.len(), 1, "no duplicate gguf copy");
    airgap_cleanup();
}
