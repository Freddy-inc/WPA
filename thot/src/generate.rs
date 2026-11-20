//! One generation path: auto fiches OR quiz from extracted course text.
//! Post-quiz remédiation appends targeted fiches for missed notions.

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

/// One missed quiz item used to build remédiation fiches.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QuizMiss {
    pub question: String,
    #[serde(default)]
    pub notion: String,
    #[serde(default)]
    pub correct_answer: String,
    #[serde(default)]
    pub chosen_answer: String,
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

/// Minimum extractable course text for quiz generation (covers "matériel insuffisant").
pub const MIN_QUIZ_MATERIAL_CHARS: usize = 120;
pub const MIN_QUIZ_QUESTIONS: usize = 4;

pub fn material_sufficient_for_quiz(text: &str) -> bool {
    text.chars().filter(|c| !c.is_whitespace()).count() >= MIN_QUIZ_MATERIAL_CHARS
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

    let (l1, l2, _) = db.effective_languages_for(&course).await.map_err(|e| {
        anyhow!("{e}")
    })?;
    let l1 = l1.as_str();
    let l2 = l2.as_str();
    // Keep chunks short for free-tier TPM; use more text for quiz / fiche coverage.
    let chunks = chunk_text(&course.text, 2500);
    let material = chunks
        .iter()
        .take(if kind == "quiz" { 3 } else { 2 })
        .cloned()
        .collect::<Vec<_>>()
        .join("\n\n");
    let material = if material.is_empty() {
        course.text.clone()
    } else {
        material
    };

    match kind {
        "fiches" | "flashcards" => {
            let fiches = generate_fiches(llm, &course.title, &material, l1, l2).await?;
            let rows = db.replace_fiches(&fiches).await?;
            Ok(GenerationResult::Fiches { fiches: rows })
        }
        "quiz" => {
            if !material_sufficient_for_quiz(&course.text) {
                return Err(anyhow!(
                    "Quiz indisponible : matériel insuffisant pour générer des questions"
                ));
            }
            let questions = generate_quiz(llm, &course.title, &material, l1, l2).await?;
            if questions.len() < MIN_QUIZ_QUESTIONS {
                return Err(anyhow!(
                    "Quiz indisponible : matériel insuffisant pour générer des questions"
                ));
            }
            let json = serde_json::to_string(&questions)?;
            db.save_quiz(&json).await?;
            let count = questions.len();
            Ok(GenerationResult::Quiz { questions, count })
        }
        other => Err(anyhow!("kind inconnu: {other} (fiches|quiz)")),
    }
}

/// After a quiz: for each miss, generate a remédiation fiche that re-explains
/// that part of the course (appended to Mes fiches — does not wipe existing cards).
pub async fn remediate_misses(
    db: &Db,
    llm: &LlmClient,
    misses: &[QuizMiss],
) -> Result<Vec<FicheRow>> {
    if misses.is_empty() {
        return Ok(vec![]);
    }
    let course = db
        .get_course()
        .await?
        .ok_or_else(|| anyhow!("Aucun cours — importez un PDF d'abord"))?;
    if course.text.trim().is_empty() {
        return Err(anyhow!("Matériel insuffisant pour la remédiation"));
    }
    let (l1, l2, _) = db.effective_languages_for(&course).await.map_err(|e| anyhow!("{e}"))?;
    let chunks = chunk_text(&course.text, 2500);
    let material = chunks
        .iter()
        .take(3)
        .cloned()
        .collect::<Vec<_>>()
        .join("\n\n");
    let material = if material.is_empty() {
        course.text.clone()
    } else {
        material
    };

    let pairs = generate_remediation_fiches(llm, &course.title, &material, misses, &l1, &l2).await?;
    db.append_fiches(&pairs).await
}

async fn generate_fiches(
    llm: &LlmClient,
    course_title: &str,
    material: &str,
    l1: &str,
    l2: &str,
) -> Result<Vec<(String, String)>> {
    let system = format!(
        "You are an AI study coach. Create flashcards from THIS course PDF excerpt only. \
         Student mother tongue L1={l1}; course language L2={l2}. \
         Return JSON only: {{\"fiches\":[{{\"front\":\"...\",\"back\":\"...\"}}]}}. \
         Produce 5 to 8 cards covering the main notions. \
         Front = key term or short prompt (L2 ok); back = clear explanation in L1 (2–4 sentences). \
         Stay faithful to the excerpt; no unrelated trivia."
    );
    let user = json!({
        "course_title": course_title,
        "course_excerpt": material,
        "l1": l1,
        "l2": l2,
    })
    .to_string();
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
    course_title: &str,
    material: &str,
    l1: &str,
    l2: &str,
) -> Result<Vec<QuizQuestion>> {
    let system = format!(
        "You are an AI study coach. Create a multiple-choice quiz from THIS course PDF excerpt only. \
         Student L1={l1}, course language L2={l2}. \
         Return JSON only: {{\"questions\":[{{\"question\":\"...\",\"choices\":[\"A\",\"B\",\"C\",\"D\"],\"answer_index\":0,\"notion\":\"...\"}}]}}. \
         Produce 6 to 10 questions covering as much of the excerpt as possible. \
         answer_index is 0-based. Questions may be in L2; notion = short topic label. \
         Stay anchored to the excerpt; no generic trivia."
    );
    let user = json!({
        "course_title": course_title,
        "course_excerpt": material,
        "l1": l1,
        "l2": l2,
    })
    .to_string();
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

async fn generate_remediation_fiches(
    llm: &LlmClient,
    course_title: &str,
    material: &str,
    misses: &[QuizMiss],
    l1: &str,
    l2: &str,
) -> Result<Vec<(String, String)>> {
    let system = format!(
        "You are an AI remédiation coach. The student missed quiz items. \
         For EACH miss, create ONE flashcard that re-explains that part of the course \
         so they understand why the correct answer is right. \
         Student L1={l1}; course language L2={l2}. \
         Return JSON only: {{\"fiches\":[{{\"front\":\"...\",\"back\":\"...\",\"notion\":\"...\"}}]}}. \
         Front = short L1 or L2 prompt about the missed notion (prefix is added by the app). \
         Back = clear remédiation in L1 (3–5 sentences), grounded in course_excerpt. \
         One fiche per miss; do not invent facts outside the excerpt."
    );
    let user = json!({
        "course_title": course_title,
        "course_excerpt": material,
        "misses": misses,
        "l1": l1,
        "l2": l2,
    })
    .to_string();
    let v = llm.chat_json(ModelRole::Batch, &system, &user).await?;
    let arr = v
        .get("fiches")
        .and_then(|a| a.as_array())
        .ok_or_else(|| anyhow!("réponse remédiation invalide"))?;

    let mut out = Vec::new();
    for (i, item) in arr.iter().enumerate() {
        let mut front = item
            .get("front")
            .and_then(|s| s.as_str())
            .unwrap_or("")
            .trim()
            .to_string();
        let back = item
            .get("back")
            .and_then(|s| s.as_str())
            .unwrap_or("")
            .trim()
            .to_string();
        if front.is_empty() {
            let notion = item
                .get("notion")
                .and_then(|s| s.as_str())
                .or_else(|| misses.get(i).map(|m| m.notion.as_str()))
                .unwrap_or("notion");
            front = format!("Revoir : {notion}");
        }
        if back.is_empty() {
            continue;
        }
        if !front.starts_with("[Remédiation]") {
            front = format!("[Remédiation] {front}");
        }
        out.push((front, back));
    }

    // Fallback: one card per miss if the model under-delivered.
    if out.is_empty() {
        for m in misses {
            let notion = if m.notion.trim().is_empty() {
                "point du cours"
            } else {
                m.notion.trim()
            };
            out.push((
                format!("[Remédiation] {notion}"),
                format!(
                    "Vous avez manqué : {}. La bonne réponse était « {} » (vous aviez choisi « {} »). \
                     Relisez ce passage du cours et reformulez l’idée dans votre L1.",
                    m.question, m.correct_answer, m.chosen_answer
                ),
            ));
        }
    }
    Ok(out)
}
