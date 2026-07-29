//! `deputyctl voice` — voice interface management (M6).
//!
//! Subcommands:
//! - `status` — show voice-relay.service state, installed models, audio device
//! - `test-speaker` — play test phrase via piper + aplay
//! - `test-mic` — record 3s, transcribe via whisper-cli
//! - `set-wake-word <word>` — update /etc/deputyos/voice.toml
//! - `enable/disable` — enable/disable the systemd unit

use std::path::Path;

use anyhow::{bail, Context, Result};
use serde::Serialize;

const VOICE_TOML: &str = "/etc/deputyos/voice.toml";
const VOICE_SERVICE: &str = "deputyos-voice-relay.service";

#[derive(Debug, Serialize)]
pub struct VoiceStatus {
    pub enabled: bool,
    pub service_active: bool,
    pub wake_word: Option<String>,
    pub stt_model: Option<String>,
    pub tts_voice: Option<String>,
    pub audio_device: Option<String>,
    pub voice_relay_exists: bool,
}

pub fn status_json() -> Result<VoiceStatus> {
    let config = read_voice_config().unwrap_or_default();

    let service_active = std::process::Command::new("systemctl")
        .args(["is-active", VOICE_SERVICE])
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim() == "active")
        .unwrap_or(false);

    let voice_relay_exists = Path::new("/opt/deputyos/voice/voice-relay.sh").is_file();

    Ok(VoiceStatus {
        enabled: config.enabled,
        service_active,
        wake_word: config.wake_word,
        stt_model: config.stt_model,
        tts_voice: config.tts_voice,
        audio_device: config.audio_device,
        voice_relay_exists,
    })
}

#[derive(Debug, Clone, Default)]
struct VoiceConfig {
    enabled: bool,
    wake_word: Option<String>,
    stt_model: Option<String>,
    tts_voice: Option<String>,
    audio_device: Option<String>,
}

fn read_voice_config() -> Result<VoiceConfig> {
    let path = Path::new(VOICE_TOML);
    if !path.is_file() {
        return Ok(VoiceConfig::default());
    }
    let raw =
        std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    let mut cfg = VoiceConfig::default();
    for line in raw.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') || line.starts_with('[') {
            continue;
        }
        if let Some((k, v)) = line.split_once('=') {
            let v = v.trim().trim_matches('"').to_string();
            match k.trim() {
                "enabled" => cfg.enabled = v == "true",
                "wake_word" => cfg.wake_word = Some(v),
                "stt_model" => cfg.stt_model = Some(v),
                "tts_voice" => cfg.tts_voice = Some(v),
                "audio_device" => cfg.audio_device = Some(v),
                _ => {}
            }
        }
    }
    Ok(cfg)
}

pub fn test_speaker() -> Result<u8> {
    let piper = Path::new("/opt/deputyos/voice/piper/piper").to_path_buf();
    if !piper.is_file() {
        bail!("piper not found — voice assets not baked into this image");
    }
    let voice_id = "en_US-amy-medium";
    let voice_path = format!("/opt/deputyos/voice/{voice_id}.onnx");
    let mut output = std::process::Command::new("echo")
        .arg("Hello from deputyOS voice system. All systems operational.")
        .stdout(std::process::Stdio::piped())
        .spawn()
        .context("echo")?;

    let output_stdout = output.stdout.take().context("echo stdout unavailable")?;
    let mut status = std::process::Command::new(piper)
        .args(["--model", &voice_path, "--output-raw"])
        .stdin(output_stdout)
        .stdout(std::process::Stdio::piped())
        .spawn()
        .context("piper")?;

    let status_stdout = status.stdout.take().context("piper stdout unavailable")?;
    let aplay = std::process::Command::new("aplay")
        .arg("-q")
        .stdin(status_stdout)
        .status()
        .context("aplay")?;

    if aplay.success() {
        println!("speaker test: ok (played via piper → aplay)");
        Ok(0)
    } else {
        bail!("aplay exited non-zero")
    }
}

pub fn test_mic() -> Result<u8> {
    let arecord = Path::new("/usr/bin/arecord").exists();
    if !arecord {
        bail!("arecord not found — install alsa-utils");
    }
    let whisper = Path::new("/opt/deputyos/voice/whisper-cli");
    if !whisper.is_file() {
        bail!("whisper-cli not found — voice assets not baked into this image");
    }
    let model = "/opt/deputyos/voice/whisper-tiny.en.bin";

    println!("recording 3 seconds of audio...");
    let mut record = std::process::Command::new("arecord")
        .args(["-d", "3", "-f", "S16_LE", "-r", "16000", "-c", "1"])
        .stdout(std::process::Stdio::piped())
        .spawn()
        .context("arecord")?;

    let record_stdout = record.stdout.take().context("arecord stdout unavailable")?;
    let output = std::process::Command::new(whisper)
        .args([
            "--model",
            model,
            "--file",
            "/dev/stdin",
            "--no-prints",
            "--output-txt",
        ])
        .stdin(record_stdout)
        .output()
        .context("whisper-cli")?;

    if output.status.success() {
        let text = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if text.is_empty() {
            println!("mic test: no speech detected (silence or low volume)");
        } else {
            println!("mic test: transcribed: \"{text}\"");
        }
        Ok(0)
    } else {
        bail!("whisper-cli exited non-zero")
    }
}

pub fn set_wake_word(word: &str) -> Result<u8> {
    if word.is_empty() || word.len() > 32 {
        bail!("wake word must be 1–32 characters");
    }
    let path = Path::new(VOICE_TOML);
    let mut cfg = read_voice_config().unwrap_or_default();
    cfg.wake_word = Some(word.to_string());

    let mut body = String::new();
    body.push_str("# deputyOS voice configuration\n");
    body.push_str(&format!("enabled = {}\n", cfg.enabled));
    if let Some(w) = &cfg.wake_word {
        body.push_str(&format!("wake_word = \"{w}\"\n"));
    }
    if let Some(m) = &cfg.stt_model {
        body.push_str(&format!("stt_model = \"{m}\"\n"));
    }
    if let Some(v) = &cfg.tts_voice {
        body.push_str(&format!("tts_voice = \"{v}\"\n"));
    }
    if let Some(d) = &cfg.audio_device {
        body.push_str(&format!("audio_device = \"{d}\"\n"));
    }

    std::fs::write(path, body).context("writing voice.toml")?;
    println!("wake word set to: {word}");
    Ok(0)
}

pub fn enable_voice() -> Result<u8> {
    let status = std::process::Command::new("systemctl")
        .args(["enable", "--now", VOICE_SERVICE])
        .status()
        .context("systemctl enable voice-relay")?;
    if status.success() {
        println!("voice enabled — deputyos-voice-relay.service started");
        Ok(0)
    } else {
        bail!("systemctl enable failed")
    }
}

pub fn disable_voice() -> Result<u8> {
    let status = std::process::Command::new("systemctl")
        .args(["disable", "--now", VOICE_SERVICE])
        .status()
        .context("systemctl disable voice-relay")?;
    if status.success() {
        println!("voice disabled — deputyos-voice-relay.service stopped");
        Ok(0)
    } else {
        bail!("systemctl disable failed")
    }
}
