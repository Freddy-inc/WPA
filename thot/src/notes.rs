//! Live course notes — text drafts from Web Speech; Groq cleans after class.
//! Privacy invariant: only text is stored/processed. Never audio bytes.

use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::db::{Db, NoteRow};
use crate::llm::{LlmClient, ModelRole};
use crate::pdf::chunk_text;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CleanedNote {
    pub title: String,
    pub cleaned_text: String,
    pub summary: String,
}

/// Reformulate raw live transcript: keep course-relevant, drop digressions.
pub async fn clean_note(db: &Db, llm: &LlmClient, note_id: i64) -> Result<NoteRow> {
    let note = db
        .get_note(note_id)
        .await?
        .ok_or_else(|| anyhow!("note introuvable"))?;
    let raw = note.raw_text.trim();
    if raw.chars().count() < 20 {
        return Err(anyhow!(
            "Transcript trop court — dictez ou collez plus de texte avant de nettoyer"
        ));
    }

    let course = db
        .get_course()
        .await?
        .ok_or_else(|| anyhow!("Aucun cours actif"))?;
    let (l1, l2, _) = db
        .effective_languages_for(&course)
        .await
        .map_err(|e| anyhow!("{e}"))?;

    let chunks = chunk_text(&course.text, 2000);
    let material = chunks
        .iter()
        .take(2)
        .cloned()
        .collect::<Vec<_>>()
        .join("\n\n");

    let cleaned = generate_clean(
        llm,
        &course.title,
        &material,
        &note.title,
        raw,
        &l1,
        &l2,
    )
    .await?;

    let title = if cleaned.title.trim().is_empty() {
        note.title.clone()
    } else {
        cleaned.title.trim().to_string()
    };
    let body = if cleaned.summary.trim().is_empty() {
        cleaned.cleaned_text
    } else {
        format!(
            "## Résumé\n{}\n\n## Notes\n{}",
            cleaned.summary.trim(),
            cleaned.cleaned_text.trim()
        )
    };

    db.update_note(note_id, Some(&title), None, Some(body.trim()))
        .await?
        .ok_or_else(|| anyhow!("note disparue après nettoyage"))
}

async fn generate_clean(
    llm: &LlmClient,
    course_title: &str,
    material: &str,
    note_title: &str,
    raw: &str,
    l1: &str,
    l2: &str,
) -> Result<CleanedNote> {
    let system = format!(
        "You clean live class notes transcribed as text (never audio). \
         Student mother tongue L1={l1}; course language L2={l2}. \
         Reformulate clearly in L1 (or bilingual L1/L2 terms when useful). \
         Keep only course-relevant content about THIS course; drop digressions, filler, \
         greetings, and off-topic chatter. Structure with short headings and bullets. \
         Use the course PDF excerpt only to decide relevance — do not invent facts absent \
         from the transcript or excerpt. \
         Return JSON only: {{\"title\":\"...\",\"summary\":\"2-4 sentences\",\"cleaned_text\":\"markdown notes\"}}."
    );
    let user = json!({
        "course_title": course_title,
        "note_title": note_title,
        "course_excerpt": material,
        "raw_transcript": raw,
        "l1": l1,
        "l2": l2,
        "privacy": "text-only; no audio was uploaded",
        "task": "clean live course notes after class"
    })
    .to_string();

    let v = llm.chat_json(ModelRole::Batch, &system, &user).await?;
    let title = v
        .get("title")
        .and_then(|s| s.as_str())
        .unwrap_or(note_title)
        .trim()
        .to_string();
    let summary = v
        .get("summary")
        .and_then(|s| s.as_str())
        .unwrap_or("")
        .trim()
        .to_string();
    let cleaned_text = v
        .get("cleaned_text")
        .and_then(|s| s.as_str())
        .unwrap_or("")
        .trim()
        .to_string();
    if cleaned_text.is_empty() {
        return Err(anyhow!("nettoyage IA vide — réessayez"));
    }
    Ok(CleanedNote {
        title: if title.is_empty() {
            note_title.to_string()
        } else {
            title
        },
        summary,
        cleaned_text,
    })
}
