//! Free MS Edge Read Aloud TTS in Rust (`msedge-tts`) — replaces `scripts/podcast_tts.py`.
//! Optional `ffmpeg` binary only for multi-part MP3 concat when present; otherwise byte-concat.

use std::path::{Path, PathBuf};
use std::process::Stdio;

use anyhow::{anyhow, Context, Result};
use msedge_tts::tts::client::tokio_runtime::connect_async;
use msedge_tts::tts::SpeechConfig;
use tokio::process::Command;
use tracing::{info, warn};

/// One dialogue turn for TTS (speaker A/B + text).
pub struct TtsTurn<'a> {
    pub speaker: &'a str,
    pub text: &'a str,
}

const VOICE_PAIRS: &[(&str, &str, &str)] = &[
    ("fr", "fr-FR-DeniseNeural", "fr-FR-HenriNeural"),
    ("en", "en-US-JennyNeural", "en-US-GuyNeural"),
    ("es", "es-ES-ElviraNeural", "es-ES-AlvaroNeural"),
    ("de", "de-DE-KatjaNeural", "de-DE-ConradNeural"),
];

fn voice_pair(lang: &str) -> (&'static str, &'static str) {
    let key = lang.get(..2).unwrap_or("en").to_lowercase();
    VOICE_PAIRS
        .iter()
        .find(|(l, _, _)| *l == key)
        .map(|(_, a, b)| (*a, *b))
        .unwrap_or(("en-US-JennyNeural", "en-US-GuyNeural"))
}

fn speech_config(short_name: &str) -> SpeechConfig {
    SpeechConfig {
        voice_name: short_name.to_string(),
        audio_format: "audio-24khz-48kbitrate-mono-mp3".into(),
        pitch: 0,
        rate: 0,
        volume: 0,
    }
}

/// Synthesize a two-voice podcast MP3 from dialogue turns.
pub async fn synthesize_podcast_mp3(turns: &[TtsTurn<'_>], lang: &str, out: &Path) -> Result<()> {
    let (voice_a, voice_b) = voice_pair(lang);
    let mut parts: Vec<Vec<u8>> = Vec::new();

    let mut tts = connect_async()
        .await
        .map_err(|e| anyhow!("msedge-tts connect: {e}"))?;

    for turn in turns {
        let text = turn.text.trim();
        if text.is_empty() {
            continue;
        }
        let voice = if turn.speaker.to_uppercase().starts_with('B') {
            voice_b
        } else {
            voice_a
        };
        let config = speech_config(voice);
        let audio = tts
            .synthesize(text, &config)
            .await
            .map_err(|e| anyhow!("msedge-tts synthesize ({voice}): {e}"))?;
        if audio.audio_bytes.len() > 32 {
            parts.push(audio.audio_bytes);
        }
    }

    if parts.is_empty() {
        return Err(anyhow!("aucune piste audio synthétisée"));
    }

    write_concat_mp3(&parts, out).await?;
    let meta = std::fs::metadata(out).with_context(|| format!("stat {}", out.display()))?;
    if meta.len() < 64 {
        return Err(anyhow!("mp3 trop petit après concat ({})", out.display()));
    }
    info!(
        path = %out.display(),
        parts = parts.len(),
        bytes = meta.len(),
        "podcast MP3 written (Rust msedge-tts)"
    );
    Ok(())
}

async fn write_concat_mp3(parts: &[Vec<u8>], out: &Path) -> Result<()> {
    if parts.len() == 1 {
        std::fs::write(out, &parts[0]).with_context(|| format!("write {}", out.display()))?;
        return Ok(());
    }

    // Prefer ffmpeg re-encode when available (more reliable across part boundaries).
    if which_ffmpeg().is_some() {
        match concat_via_ffmpeg(parts, out).await {
            Ok(()) => return Ok(()),
            Err(err) => warn!(error = %err, "ffmpeg concat failed — falling back to byte concat"),
        }
    }

    let mut buf = Vec::with_capacity(parts.iter().map(|p| p.len()).sum());
    for p in parts {
        buf.extend_from_slice(p);
    }
    std::fs::write(out, &buf).with_context(|| format!("write {}", out.display()))?;
    Ok(())
}

fn which_ffmpeg() -> Option<PathBuf> {
    std::env::var_os("PATH").and_then(|paths| {
        for dir in std::env::split_paths(&paths) {
            let p = dir.join("ffmpeg");
            if p.is_file() {
                return Some(p);
            }
        }
        None
    })
}

async fn concat_via_ffmpeg(parts: &[Vec<u8>], out: &Path) -> Result<()> {
    let ffmpeg = which_ffmpeg().ok_or_else(|| anyhow!("ffmpeg introuvable"))?;
    let tmp = tempfile_dir()?;
    let mut list_lines = Vec::new();
    let mut part_paths = Vec::new();
    for (i, bytes) in parts.iter().enumerate() {
        let path = tmp.join(format!("part-{i:03}.mp3"));
        std::fs::write(&path, bytes)?;
        list_lines.push(format!("file '{}'", path.display()));
        part_paths.push(path);
    }
    let list_path = tmp.join("list.txt");
    std::fs::write(&list_path, list_lines.join("\n"))?;

    let output = Command::new(&ffmpeg)
        .args([
            "-y",
            "-f",
            "concat",
            "-safe",
            "0",
            "-i",
        ])
        .arg(&list_path)
        .args(["-codec:a", "libmp3lame", "-q:a", "5"])
        .arg(out)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .await
        .context("spawn ffmpeg")?;

    let _ = std::fs::remove_dir_all(&tmp);

    if !output.status.success() {
        let err = String::from_utf8_lossy(&output.stderr);
        return Err(anyhow!("ffmpeg failed: {}", err.trim()));
    }
    Ok(())
}

fn tempfile_dir() -> Result<PathBuf> {
    let base = std::env::temp_dir().join(format!(
        "thot-tts-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis())
            .unwrap_or(0)
    ));
    std::fs::create_dir_all(&base)?;
    Ok(base)
}
