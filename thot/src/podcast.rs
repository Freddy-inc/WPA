//! Course podcast overview: Groq script (two voices) + free edge-tts MP3 (Web Speech fallback).

use std::path::{Path, PathBuf};
use std::process::Stdio;

use anyhow::{anyhow, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::json;
use tokio::process::Command;
use tracing::{info, warn};

use crate::db::{Db, PodcastRow};
use crate::llm::{LlmClient, ModelRole};
use crate::pdf::chunk_text;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PodcastTurn {
    pub speaker: String,
    pub text: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PodcastScript {
    pub title: String,
    pub turns: Vec<PodcastTurn>,
}

/// Generate script from course corpus, synthesize MP3 when possible, store as course artifact.
pub async fn generate_podcast(db: &Db, llm: &LlmClient, data_dir: &Path) -> Result<PodcastRow> {
    let course = db
        .get_course()
        .await?
        .ok_or_else(|| anyhow!("Aucun cours — importez un PDF d'abord"))?;
    if course.text.trim().is_empty() {
        return Err(anyhow!("Matériel insuffisant pour générer un podcast"));
    }

    let (l1, l2, _) = db
        .effective_languages_for(&course)
        .await
        .map_err(|e| anyhow!("{e}"))?;
    let chunks = chunk_text(&course.text, 2500);
    let material = chunks
        .iter()
        .take(2)
        .cloned()
        .collect::<Vec<_>>()
        .join("\n\n");
    let material = if material.is_empty() {
        course.text.clone()
    } else {
        material
    };

    let script = generate_script(llm, &course.title, &material, &l1, &l2).await?;
    if script.turns.len() < 4 {
        return Err(anyhow!("Script podcast trop court — réessayez"));
    }

    let script_json = serde_json::to_string(&script)?;
    let podcasts_dir = data_dir.join("podcasts");
    let _ = std::fs::create_dir_all(&podcasts_dir);

    // Insert row first to get id for audio filename.
    let draft = db
        .insert_podcast(
            &script.title,
            &script_json,
            None,
            "webspeech",
        )
        .await?;

    let audio_path = podcasts_dir.join(format!("{}.mp3", draft.id));
    let playback = match synthesize_mp3(&script, &l1, &audio_path).await {
        Ok(()) => {
            let rel = format!("podcasts/{}.mp3", draft.id);
            db.update_podcast_audio(draft.id, Some(&rel), "mp3")
                .await?
                .ok_or_else(|| anyhow!("podcast missing after audio update"))?
        }
        Err(err) => {
            warn!(error = %err, "edge-tts failed; keeping webspeech fallback");
            draft
        }
    };

    info!(
        id = playback.id,
        mode = %playback.playback_mode,
        turns = script.turns.len(),
        "podcast generated"
    );
    Ok(playback)
}

async fn generate_script(
    llm: &LlmClient,
    course_title: &str,
    material: &str,
    l1: &str,
    l2: &str,
) -> Result<PodcastScript> {
    let system = format!(
        "You write a short two-host study podcast overview of a university course PDF. \
         Student mother tongue L1={l1}; course language L2={l2}. \
         Speak the dialogue in L1 so the student can follow. \
         Host A = clear explainer; Host B = curious peer who asks short clarifying questions. \
         Cover the main notions from the excerpt only — no inventing unrelated facts. \
         Keep turns short (1–3 sentences each). Total 8 to 14 turns. \
         Return JSON only: {{\"title\":\"...\",\"turns\":[{{\"speaker\":\"A\",\"text\":\"...\"}},{{\"speaker\":\"B\",\"text\":\"...\"}}]}}."
    );
    let user = json!({
        "course_title": course_title,
        "course_excerpt": material,
        "l1": l1,
        "l2": l2,
        "style": "notebooklm-like audio overview for international students"
    })
    .to_string();

    let v = llm.chat_json(ModelRole::Batch, &system, &user).await?;
    let title = v
        .get("title")
        .and_then(|s| s.as_str())
        .unwrap_or(course_title)
        .trim()
        .to_string();
    let arr = v
        .get("turns")
        .and_then(|a| a.as_array())
        .ok_or_else(|| anyhow!("réponse podcast invalide (turns)"))?;

    let mut turns = Vec::new();
    for item in arr {
        let speaker = item
            .get("speaker")
            .and_then(|s| s.as_str())
            .unwrap_or("A")
            .trim()
            .to_uppercase();
        let speaker = if speaker.starts_with('B') {
            "B".into()
        } else {
            "A".into()
        };
        let text = item
            .get("text")
            .and_then(|s| s.as_str())
            .unwrap_or("")
            .trim()
            .to_string();
        if !text.is_empty() {
            turns.push(PodcastTurn { speaker, text });
        }
    }
    if turns.is_empty() {
        return Err(anyhow!("aucune réplique podcast générée"));
    }
    Ok(PodcastScript {
        title: if title.is_empty() {
            format!("Aperçu — {course_title}")
        } else {
            title
        },
        turns,
    })
}

async fn synthesize_mp3(script: &PodcastScript, l1: &str, out: &Path) -> Result<()> {
    let script_path = resolve_tts_script()?;
    let tmp_json = out.with_extension("json");
    let payload = json!({ "turns": script.turns });
    std::fs::write(&tmp_json, serde_json::to_vec_pretty(&payload)?)
        .with_context(|| format!("write {}", tmp_json.display()))?;

    let output = Command::new("python3")
        .arg(&script_path)
        .arg("--script")
        .arg(&tmp_json)
        .arg("--out")
        .arg(out)
        .arg("--lang")
        .arg(l1)
        .env(
            "PYTHONPATH",
            format!(
                "{}:{}",
                std::env::var("PYTHONPATH").unwrap_or_default(),
                dirs_user_site().unwrap_or_default()
            ),
        )
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .await
        .context("spawn python3 podcast_tts")?;

    let _ = std::fs::remove_file(&tmp_json);

    let stderr = String::from_utf8_lossy(&output.stderr);
    let stdout = String::from_utf8_lossy(&output.stdout);
    // Accept a usable MP3 even if the helper exits oddly (edge-tts/ffmpeg teardown).
    let ok_file = out.is_file()
        && std::fs::metadata(out)
            .map(|m| m.len() > 64)
            .unwrap_or(false);
    if !output.status.success() && !ok_file {
        return Err(anyhow!(
            "podcast_tts exited with {}: {}",
            output.status,
            stderr.trim()
        ));
    }
    if !ok_file {
        return Err(anyhow!(
            "mp3 not created at {} (stdout={} stderr={})",
            out.display(),
            stdout.trim(),
            stderr.trim()
        ));
    }
    if !output.status.success() {
        warn!(
            status = %output.status,
            stderr = %stderr.trim(),
            "podcast_tts non-zero exit but MP3 ok — accepting"
        );
    }
    Ok(())
}

fn dirs_user_site() -> Option<String> {
    // Best-effort: ~/.local/lib/python3.*/site-packages
    let home = std::env::var_os("HOME")?;
    let base = PathBuf::from(home).join(".local/lib");
    let mut found = Vec::new();
    if let Ok(rd) = std::fs::read_dir(&base) {
        for ent in rd.flatten() {
            let p = ent.path().join("site-packages");
            if p.is_dir() {
                found.push(p.display().to_string());
            }
        }
    }
    if found.is_empty() {
        None
    } else {
        Some(found.join(":"))
    }
}

fn resolve_tts_script() -> Result<PathBuf> {
    let candidates = [
        PathBuf::from("scripts/podcast_tts.py"),
        PathBuf::from("thot/scripts/podcast_tts.py"),
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("scripts/podcast_tts.py"),
    ];
    for c in candidates {
        if c.is_file() {
            return Ok(c);
        }
    }
    Err(anyhow!(
        "scripts/podcast_tts.py introuvable (installez edge-tts: pip install edge-tts)"
    ))
}
