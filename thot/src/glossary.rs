//! Glossary: LLM draft → web verify → reconcile → SQLite cache.

use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::db::Db;
use crate::llm::{LlmClient, ModelRole};
use crate::web_verify::{WebSource, WebVerifier};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GlossaryResult {
    pub term_l2: String,
    pub meaning_l1: String,
    pub verified: bool,
    pub badge: String,
    pub confidence: f64,
    pub sources: Vec<WebSource>,
    pub cached: bool,
    pub context_snippet: Option<String>,
}

pub async fn define_term(
    db: &Db,
    llm: &LlmClient,
    verifier: &WebVerifier,
    term: &str,
    context: &str,
    l1: &str,
    l2: &str,
) -> Result<GlossaryResult> {
    let term = term.trim();
    if term.is_empty() {
        return Err(anyhow!("terme vide"));
    }
    if term.split_whitespace().count() > 8 {
        return Err(anyhow!(
            "Sélection trop large — resserrez sur un mot ou une expression courte"
        ));
    }

    if let Some(cached) = db.get_glossary(term).await? {
        let sources: Vec<WebSource> =
            serde_json::from_str(&cached.sources_json).unwrap_or_default();
        return Ok(GlossaryResult {
            term_l2: cached.term_l2,
            meaning_l1: cached.meaning_l1,
            verified: cached.verified,
            badge: badge_for(cached.verified),
            confidence: cached.confidence,
            sources,
            cached: true,
            context_snippet: cached.context_snippet,
        });
    }

    let snippet = nearest_context(context, term, 400);
    let draft = draft_meaning(llm, term, &snippet, l1, l2).await?;
    let sources = verifier.lookup(term, l2).await;
    let reconciled = reconcile(llm, term, &draft.meaning_l1, &sources, l1, l2).await?;

    let sources_json = serde_json::to_string(&reconciled.sources)?;
    let row = db
        .put_glossary(
            &reconciled.term_l2,
            &reconciled.meaning_l1,
            snippet.as_deref(),
            &sources_json,
            reconciled.verified,
            reconciled.confidence,
        )
        .await?;

    Ok(GlossaryResult {
        term_l2: row.term_l2,
        meaning_l1: row.meaning_l1,
        verified: row.verified,
        badge: badge_for(row.verified),
        confidence: row.confidence,
        sources: reconciled.sources,
        cached: false,
        context_snippet: row.context_snippet,
    })
}

fn badge_for(verified: bool) -> String {
    if verified {
        "Vérifié sources web".into()
    } else {
        "Sens proposé — sources limitées".into()
    }
}

#[derive(Debug)]
struct Draft {
    meaning_l1: String,
}

async fn draft_meaning(
    llm: &LlmClient,
    term: &str,
    snippet: &Option<String>,
    l1: &str,
    l2: &str,
) -> Result<Draft> {
    let system = format!(
        "You are a bilingual study assistant. Explain L2 ({l2}) academic terms in L1 ({l1}). \
         Reply with a JSON object only: {{\"term_l2\":\"...\",\"meaning_l1\":\"...\",\"confidence\":0.0-1.0}}. \
         Keep meaning_l1 to 1-2 short sentences, course-aware when context is given."
    );
    let user = json!({
        "term_l2": term,
        "course_context": snippet,
        "l1": l1,
        "l2": l2,
    })
    .to_string();

    let v = llm
        .chat_json(ModelRole::Interactive, &system, &user)
        .await?;
    let meaning = v
        .get("meaning_l1")
        .and_then(|m| m.as_str())
        .ok_or_else(|| anyhow!("LLM draft missing meaning_l1"))?
        .trim()
        .to_string();
    if meaning.is_empty() {
        return Err(anyhow!("Sens non disponible"));
    }
    Ok(Draft {
        meaning_l1: meaning,
    })
}

async fn reconcile(
    llm: &LlmClient,
    term: &str,
    draft_meaning: &str,
    sources: &[WebSource],
    l1: &str,
    l2: &str,
) -> Result<GlossaryResult> {
    if sources.is_empty() {
        return Ok(GlossaryResult {
            term_l2: term.into(),
            meaning_l1: draft_meaning.into(),
            verified: false,
            badge: badge_for(false),
            confidence: 0.4,
            sources: vec![],
            cached: false,
            context_snippet: None,
        });
    }

    // Rule-based overlap first (cheap); LLM reconcile if unclear.
    let joined = sources
        .iter()
        .map(|s| s.excerpt.to_lowercase())
        .collect::<Vec<_>>()
        .join(" ");
    let draft_l = draft_meaning.to_lowercase();
    let overlap = significant_overlap(&draft_l, &joined);

    if overlap {
        return Ok(GlossaryResult {
            term_l2: term.into(),
            meaning_l1: draft_meaning.into(),
            verified: true,
            badge: badge_for(true),
            confidence: 0.85,
            sources: sources.to_vec(),
            cached: false,
            context_snippet: None,
        });
    }

    // Second LLM pass: reconcile draft with official excerpts.
    let system = format!(
        "Reconcile a draft L1 ({l1}) gloss of an L2 ({l2}) term with official web excerpts. \
         Output JSON only: {{\"term_l2\":\"...\",\"meaning_l1\":\"...\",\"verified\":true|false,\"confidence\":0.0-1.0}}. \
         Prefer official sources; if they conflict strongly with the draft, rewrite meaning_l1 from sources. \
         Set verified=true only if meaning is supported by at least one excerpt."
    );
    let user = json!({
        "term_l2": term,
        "draft_meaning_l1": draft_meaning,
        "sources": sources,
    })
    .to_string();

    match llm.chat_json(ModelRole::Interactive, &system, &user).await {
        Ok(v) => {
            let meaning = v
                .get("meaning_l1")
                .and_then(|m| m.as_str())
                .unwrap_or(draft_meaning)
                .trim()
                .to_string();
            let verified = v
                .get("verified")
                .and_then(|b| b.as_bool())
                .unwrap_or(false);
            let confidence = v
                .get("confidence")
                .and_then(|c| c.as_f64())
                .unwrap_or(0.6);
            Ok(GlossaryResult {
                term_l2: term.into(),
                meaning_l1: meaning,
                verified,
                badge: badge_for(verified),
                confidence,
                sources: sources.to_vec(),
                cached: false,
                context_snippet: None,
            })
        }
        Err(_) => Ok(GlossaryResult {
            term_l2: term.into(),
            meaning_l1: draft_meaning.into(),
            verified: false,
            badge: badge_for(false),
            confidence: 0.45,
            sources: sources.to_vec(),
            cached: false,
            context_snippet: None,
        }),
    }
}

fn significant_overlap(draft: &str, sources: &str) -> bool {
    let stop: [&str; 24] = [
        "the", "a", "an", "and", "or", "of", "to", "in", "on", "for", "is", "are", "le", "la",
        "les", "un", "une", "des", "de", "du", "et", "en", "est", "que",
    ];
    let tokens: Vec<&str> = draft
        .split(|c: char| !c.is_alphanumeric())
        .filter(|t| t.len() > 3 && !stop.contains(t))
        .collect();
    if tokens.is_empty() {
        return false;
    }
    let hits = tokens.iter().filter(|t| sources.contains(*t)).count();
    hits * 2 >= tokens.len() // ≥ 50% of content tokens appear in sources
}

fn nearest_context(text: &str, term: &str, radius: usize) -> Option<String> {
    let lower = text.to_lowercase();
    let needle = term.to_lowercase();
    let pos = lower.find(&needle)?;
    let start = pos.saturating_sub(radius);
    let end = (pos + needle.len() + radius).min(text.len());
    // Align to char boundaries roughly by byte clamp then trim.
    let slice = text.get(start..end).unwrap_or(term);
    Some(slice.trim().to_string())
}
