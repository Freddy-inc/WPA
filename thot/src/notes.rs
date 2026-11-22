//! Live course notes — text drafts from Web Speech; Groq cleans after class.
//! Privacy invariant: only text is stored/processed. Never audio bytes.
//! Cleaned output must keep the professor’s pedagogical voice — not chatbot tone.

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
}

/// Reformulate raw live transcript in the lecture’s own pedagogical voice.
/// Drop digressions only — never chatbot framing.
pub async fn clean_note(db: &Db, llm: &LlmClient, note_id: i64) -> Result<NoteRow> {
    let note = db
        .get_note(note_id)
        .await?
        .ok_or_else(|| anyhow!("note introuvable"))?;
    let raw = note.raw_text.trim();
    if raw.chars().count() < 20 {
        return Err(anyhow!(
            "Transcript trop court — dictez ou collez plus de texte avant de clarifier"
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
    let body = strip_ai_framing(cleaned.cleaned_text.trim());
    if body.is_empty() {
        return Err(anyhow!("clarification vide — réessayez"));
    }

    db.update_note(note_id, Some(&title), None, Some(&body))
        .await?
        .ok_or_else(|| anyhow!("note disparue après clarification"))
}

fn strip_ai_framing(text: &str) -> String {
    let banned_starts = [
        "voici un résumé",
        "voici le résumé",
        "en résumé",
        "résumé :",
        "résumé:",
        "points clés",
        "à retenir",
        "digressions retirées",
        "here is a summary",
        "here's a summary",
        "in summary",
        "key takeaways",
    ];
    let mut lines: Vec<&str> = text.lines().collect();
    while let Some(first) = lines.first() {
        let t = first.trim().to_lowercase();
        if t.is_empty()
            || t == "## résumé"
            || t == "## notes"
            || t == "## summary"
            || banned_starts.iter().any(|b| t.starts_with(b))
        {
            lines.remove(0);
            continue;
        }
        break;
    }
    lines.join("\n").trim().to_string()
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
        "You edit a live lecture transcript (text only — never audio) into clean course notes. \
         Student mother tongue L1={l1}; course language L2={l2}. \
         LANGUAGE: write the cleaned notes in the SAME language as the spoken lecture (raw_transcript). \
         If the professor spoke French, output French — do not switch to English because the PDF excerpt is English. \
         CRITICAL VOICE: write as if the professor explained the material again — same pedagogical style, \
         pacing, emphasis, and wording habits as that lecture. Soften fillers (euh, bon…) and remove digressions only. \
         Do NOT sound like a chatbot or study-assistant. Forbidden phrases and patterns: \
         « Voici un résumé… », « En résumé », « Points clés », « À retenir », « Digressions retirées », \
         « Key takeaways », « Good morning everyone » as a generic AI opener, meta talk about cleaning/AI, \
         flashcard-style bullet dumps. Prefer continuous explanatory paragraphs \
         (short subheadings OK only if the professor would section the board). \
         Keep technical terms as the professor used them in class. \
         Use the course PDF excerpt only to judge what is on-topic — invent nothing absent from transcript or excerpt. \
         Return JSON only: {{\"title\":\"short course-note title\",\"cleaned_text\":\"professor-voice notes in markdown\"}}."
    );
    let user = json!({
        "course_title": course_title,
        "note_title": note_title,
        "course_excerpt": material,
        "raw_transcript": raw,
        "l1": l1,
        "l2": l2,
        "privacy": "text-only; no audio was uploaded",
        "task": "rewrite live lecture notes in the professor pedagogical voice; drop digressions only",
        "voice_rule": "must read like the lecturer, not like an AI summary"
    })
    .to_string();

    let v = llm.chat_json(ModelRole::Batch, &system, &user).await?;
    let title = v
        .get("title")
        .and_then(|s| s.as_str())
        .unwrap_or(note_title)
        .trim()
        .to_string();
    let cleaned_text = v
        .get("cleaned_text")
        .and_then(|s| s.as_str())
        .unwrap_or("")
        .trim()
        .to_string();
    if cleaned_text.is_empty() {
        return Err(anyhow!("clarification vide — réessayez"));
    }
    Ok(CleanedNote {
        title: if title.is_empty() {
            note_title.to_string()
        } else {
            title
        },
        cleaned_text,
    })
}
