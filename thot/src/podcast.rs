//! Course podcast overview: Groq script (two voices) + free edge-tts MP3 (Web Speech fallback).
//! Misses remédiation podcast: short chapters tied to quiz misses + course PDF pages.

use std::path::{Path, PathBuf};
use std::process::Stdio;

use anyhow::{anyhow, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::json;
use tokio::process::Command;
use tracing::{info, warn};

use crate::db::{Db, PodcastRow};
use crate::generate::QuizMiss;
use crate::llm::{LlmClient, ModelRole};
use crate::pdf::chunk_text;
use crate::source::{self, SourceHit};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PodcastTurn {
    pub speaker: String,
    pub text: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct PodcastChapter {
    pub notion: String,
    #[serde(default)]
    pub page: Option<i64>,
    #[serde(default)]
    pub doc_id: Option<i64>,
    #[serde(default)]
    pub filename: Option<String>,
    #[serde(default)]
    pub turns: Vec<PodcastTurn>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PodcastScript {
    pub title: String,
    #[serde(default)]
    pub kind: String,
    #[serde(default)]
    pub chapters: Vec<PodcastChapter>,
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

    let mut script = generate_script(llm, &course.title, &material, &l1, &l2).await?;
    script.kind = "overview".into();
    if script.turns.len() < 4 {
        return Err(anyhow!("Script podcast trop court — réessayez"));
    }
    persist_podcast(db, data_dir, &script, &l1).await
}

/// Short remédiation podcast: one mini-chapter per missed notion, grounded in course PDF.
pub async fn generate_misses_podcast(
    db: &Db,
    llm: &LlmClient,
    data_dir: &Path,
    misses: &[QuizMiss],
    notions: &[String],
) -> Result<(PodcastRow, Vec<SourceHit>)> {
    let course = db
        .get_course()
        .await?
        .ok_or_else(|| anyhow!("Aucun cours — importez un PDF d'abord"))?;
    if course.text.trim().is_empty() {
        return Err(anyhow!("Matériel insuffisant pour générer un podcast"));
    }

    let mut notion_list: Vec<String> = notions
        .iter()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();
    for m in misses {
        let n = m.notion.trim();
        if !n.is_empty() {
            notion_list.push(n.to_string());
        } else if !m.question.trim().is_empty() {
            notion_list.push(m.question.trim().chars().take(48).collect());
        }
    }
    notion_list.sort();
    notion_list.dedup();
    if notion_list.is_empty() {
        return Err(anyhow!("Aucune notion manquée pour le podcast"));
    }
    notion_list.truncate(6);

    let sources = if !misses.is_empty() {
        source::locate_misses(db, data_dir, misses)
            .await
            .unwrap_or_default()
    } else {
        let mut hits = Vec::new();
        for n in &notion_list {
            if let Ok(Some(h)) = source::locate_query_str(db, data_dir, n, n).await {
                hits.push(h);
            }
        }
        hits
    };

    let (l1, l2, _) = db
        .effective_languages_for(&course)
        .await
        .map_err(|e| anyhow!("{e}"))?;

    let mut material_parts: Vec<String> = sources
        .iter()
        .map(|s| {
            format!(
                "[p.{} · {} · {}] {}",
                s.page, s.filename, s.notion, s.snippet
            )
        })
        .collect();
    let chunks = chunk_text(&course.text, 1800);
    material_parts.extend(chunks.into_iter().take(2));
    let material = material_parts.join("\n\n");

    let mut script =
        generate_misses_script(llm, &course.title, &material, &notion_list, &sources, &l1, &l2)
            .await?;
    script.kind = "misses".into();
    for (i, ch) in script.chapters.iter_mut().enumerate() {
        let hit = sources
            .iter()
            .find(|h| {
                h.notion.eq_ignore_ascii_case(&ch.notion)
                    || ch.notion.to_lowercase().contains(&h.notion.to_lowercase())
                    || h.notion.to_lowercase().contains(&ch.notion.to_lowercase())
            })
            .or_else(|| {
                // Same page as a locate hit
                ch.page.and_then(|p| sources.iter().find(|h| h.page == p))
            })
            .or_else(|| sources.get(i));
        if let Some(hit) = hit {
            if ch.page.is_none() {
                ch.page = Some(hit.page);
            }
            if ch.doc_id.is_none() {
                ch.doc_id = Some(hit.doc_id);
            }
            if ch.filename.is_none() {
                ch.filename = Some(hit.filename.clone());
            }
        }
    }
    if script.turns.len() < 4 {
        return Err(anyhow!("Script remédiation trop court — réessayez"));
    }
    let row = persist_podcast(db, data_dir, &script, &l1).await?;
    Ok((row, sources))
}

async fn persist_podcast(
    db: &Db,
    data_dir: &Path,
    script: &PodcastScript,
    l1: &str,
) -> Result<PodcastRow> {
    let script_json = serde_json::to_string(script)?;
    let podcasts_dir = data_dir.join("podcasts");
    let _ = std::fs::create_dir_all(&podcasts_dir);

    let draft = db
        .insert_podcast(&script.title, &script_json, None, "webspeech")
        .await?;

    let audio_path = podcasts_dir.join(format!("{}.mp3", draft.id));
    let playback = match synthesize_mp3(script, l1, &audio_path).await {
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
        kind = %script.kind,
        turns = script.turns.len(),
        chapters = script.chapters.len(),
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
    let turns = parse_turns(v.get("turns"))?;
    if turns.is_empty() {
        return Err(anyhow!("aucune réplique podcast générée"));
    }
    Ok(PodcastScript {
        title: if title.is_empty() {
            format!("Aperçu — {course_title}")
        } else {
            title
        },
        kind: "overview".into(),
        chapters: vec![],
        turns,
    })
}

async fn generate_misses_script(
    llm: &LlmClient,
    course_title: &str,
    material: &str,
    notions: &[String],
    sources: &[SourceHit],
    l1: &str,
    l2: &str,
) -> Result<PodcastScript> {
    let system = format!(
        "You write a SHORT two-host remédiation podcast about MISSED quiz notions from THIS course PDF. \
         Student mother tongue L1={l1}; course language L2={l2}. Speak in L1. \
         Host A explains; Host B asks brief clarifying questions. \
         Create one mini-chapter per missed notion (2–4 turns each). \
         Ground every claim in the course excerpt / page snippets — no inventing unrelated facts. \
         Return JSON only: {{\"title\":\"...\",\"kind\":\"misses\",\"chapters\":[{{\"notion\":\"...\",\"page\":1,\"turns\":[{{\"speaker\":\"A\",\"text\":\"...\"}},{{\"speaker\":\"B\",\"text\":\"...\"}}]}}],\"turns\":[]}}. \
         Also flatten all chapter turns into top-level turns in order (for TTS). \
         Total flattened turns 6 to 16."
    );
    let source_map: Vec<_> = sources
        .iter()
        .map(|s| {
            json!({
                "notion": s.notion,
                "page": s.page,
                "doc_id": s.doc_id,
                "filename": s.filename,
                "snippet": s.snippet,
            })
        })
        .collect();
    let user = json!({
        "course_title": course_title,
        "missed_notions": notions,
        "pdf_sources": source_map,
        "course_excerpt": material,
        "l1": l1,
        "l2": l2,
        "style": "remédiation audio chapters tied to course PDF pages — not a random document"
    })
    .to_string();

    let v = llm.chat_json(ModelRole::Batch, &system, &user).await?;
    let title = v
        .get("title")
        .and_then(|s| s.as_str())
        .unwrap_or("Remédiation — notions manquées")
        .trim()
        .to_string();

    let mut chapters = Vec::new();
    if let Some(arr) = v.get("chapters").and_then(|a| a.as_array()) {
        for (i, item) in arr.iter().enumerate() {
            let notion = item
                .get("notion")
                .and_then(|s| s.as_str())
                .unwrap_or_else(|| notions.get(i).map(|s| s.as_str()).unwrap_or("notion"))
                .trim()
                .to_string();
            let page = item.get("page").and_then(|p| p.as_i64());
            let doc_id = item.get("doc_id").and_then(|p| p.as_i64());
            let filename = item
                .get("filename")
                .and_then(|s| s.as_str())
                .map(|s| s.to_string());
            let turns = parse_turns(item.get("turns")).unwrap_or_default();
            if turns.is_empty() {
                continue;
            }
            chapters.push(PodcastChapter {
                notion,
                page,
                doc_id,
                filename,
                turns,
            });
        }
    }

    let mut turns = parse_turns(v.get("turns")).unwrap_or_default();
    if turns.is_empty() {
        for ch in &chapters {
            turns.extend(ch.turns.clone());
        }
    }
    if chapters.is_empty() && !turns.is_empty() {
        // Build chapters from notions by splitting turns.
        let per = (turns.len() / notions.len().max(1)).max(2);
        for (i, n) in notions.iter().enumerate() {
            let start = i * per;
            if start >= turns.len() {
                break;
            }
            let end = ((i + 1) * per).min(turns.len());
            let hit = sources.iter().find(|h| h.notion.eq_ignore_ascii_case(n));
            chapters.push(PodcastChapter {
                notion: n.clone(),
                page: hit.map(|h| h.page),
                doc_id: hit.map(|h| h.doc_id),
                filename: hit.map(|h| h.filename.clone()),
                turns: turns[start..end].to_vec(),
            });
        }
    }
    if turns.is_empty() {
        return Err(anyhow!("aucune réplique remédiation générée"));
    }

    Ok(PodcastScript {
        title: if title.is_empty() {
            format!("Remédiation — {course_title}")
        } else {
            title
        },
        kind: "misses".into(),
        chapters,
        turns,
    })
}

fn parse_turns(v: Option<&serde_json::Value>) -> Result<Vec<PodcastTurn>> {
    let arr = match v.and_then(|a| a.as_array()) {
        Some(a) => a,
        None => return Ok(vec![]),
    };
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
    Ok(turns)
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
