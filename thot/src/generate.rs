//! One generation path: auto fiches OR quiz from extracted course text.

use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::db::{Db, FicheRow};
use crate::llm::{LlmClient, ModelRole};
use crate::pdf::chunk_text;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QuizQuestion {
    pub question: String,
    pub choices: Vec<String>,
    pub answer_index: usize,
    pub notion: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "kind")]
pub enum GenerationResult {
    #[serde(rename = "fiches")]
    Fiches { fiches: Vec<FicheRow> },
    #[serde(rename = "quiz")]
    Quiz {
        questions: Vec<QuizQuestion>,
        count: usize,
    },
}

pub async fn generate(
    db: &Db,
    llm: &LlmClient,
    kind: &str,
) -> Result<GenerationResult> {
    let course = db
        .get_course()
        .await?
        .ok_or_else(|| anyhow!("Aucun cours — importez un PDF d'abord"))?;
    if course.text.trim().is_empty() {
        return Err(anyhow!("Matériel insuffisant pour générer"));
    }

    let l1 = course.l1.as_deref().unwrap_or("fr");
    let l2 = course.l2.as_deref().unwrap_or("en");
    // Keep chunks short for free-tier TPM.
    let chunks = chunk_text(&course.text, 2500);
    let material = chunks.first().cloned().unwrap_or(course.text.clone());

    match kind {
        "fiches" | "flashcards" => {
            let fiches = generate_fiches(llm, &material, l1, l2).await?;
            let rows = db.replace_fiches(&fiches).await?;
            Ok(GenerationResult::Fiches { fiches: rows })
        }
        "quiz" => {
            let questions = generate_quiz(llm, &material, l1, l2).await?;
            if questions.is_empty() {
                return Err(anyhow!("Matériel insuffisant pour générer des questions"));
            }
            let json = serde_json::to_string(&questions)?;
            db.save_quiz(&json).await?;
            let count = questions.len();
            Ok(GenerationResult::Quiz { questions, count })
        }
        other => Err(anyhow!("kind inconnu: {other} (fiches|quiz)")),
    }
}

async fn generate_fiches(
    llm: &LlmClient,
    material: &str,
    l1: &str,
    l2: &str,
) -> Result<Vec<(String, String)>> {
    let system = format!(
        "You create study flashcards from course text. \
         Student L1={l1}, course language L2={l2}. \
         Return JSON only: {{\"fiches\":[{{\"front\":\"...\",\"back\":\"...\"}}]}}. \
         Produce 5 to 8 cards. Front = key term or prompt (L2 ok); back = concise explanation in L1. \
         Stay faithful to the provided text; do not invent unrelated facts."
    );
    let user = json!({ "course_excerpt": material }).to_string();
    let v = llm.chat_json(ModelRole::Batch, &system, &user).await?;
    let arr = v
        .get("fiches")
        .and_then(|a| a.as_array())
        .ok_or_else(|| anyhow!("réponse fiches invalide"))?;

    let mut out = Vec::new();
    for item in arr {
        let front = item
            .get("front")
            .and_then(|s| s.as_str())
            .unwrap_or("")
            .trim();
        let back = item
            .get("back")
            .and_then(|s| s.as_str())
            .unwrap_or("")
            .trim();
        if !front.is_empty() && !back.is_empty() {
            out.push((front.to_string(), back.to_string()));
        }
    }
    if out.is_empty() {
        return Err(anyhow!("aucune fiche générée"));
    }
    Ok(out)
}

async fn generate_quiz(
    llm: &LlmClient,
    material: &str,
    l1: &str,
    l2: &str,
) -> Result<Vec<QuizQuestion>> {
    let system = format!(
        "You create a multiple-choice quiz from course text. \
         Student L1={l1}, course language L2={l2}. \
         Return JSON only: {{\"questions\":[{{\"question\":\"...\",\"choices\":[\"A\",\"B\",\"C\",\"D\"],\"answer_index\":0,\"notion\":\"...\"}}]}}. \
         Produce 5 to 8 questions covering the excerpt. answer_index is 0-based. \
         Questions may be in L2; keep them anchored to the excerpt."
    );
    let user = json!({ "course_excerpt": material }).to_string();
    let v = llm.chat_json(ModelRole::Batch, &system, &user).await?;
    let arr = v
        .get("questions")
        .and_then(|a| a.as_array())
        .ok_or_else(|| anyhow!("réponse quiz invalide"))?;

    let mut out = Vec::new();
    for item in arr {
        let question = item
            .get("question")
            .and_then(|s| s.as_str())
            .unwrap_or("")
            .trim()
            .to_string();
        let choices: Vec<String> = item
            .get("choices")
            .and_then(|c| c.as_array())
            .map(|a| {
                a.iter()
                    .filter_map(|x| x.as_str().map(|s| s.trim().to_string()))
                    .filter(|s| !s.is_empty())
                    .collect()
            })
            .unwrap_or_default();
        let answer_index = item
            .get("answer_index")
            .and_then(|i| i.as_u64())
            .unwrap_or(0) as usize;
        let notion = item
            .get("notion")
            .and_then(|s| s.as_str())
            .unwrap_or("")
            .trim()
            .to_string();
        if question.is_empty() || choices.len() < 2 || answer_index >= choices.len() {
            continue;
        }
        out.push(QuizQuestion {
            question,
            choices,
            answer_index,
            notion,
        });
    }
    Ok(out)
}
