//! Glossary: LLM draft → web verify → reconcile → SQLite cache.
//! Single-word terms keep the classic path; multi-word selections also get
//! a whole-phrase L1 sense (course-aware) plus per-word help.

use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::db::Db;
use crate::llm::{LlmClient, ModelRole};
use crate::web_verify::{WebSource, WebVerifier};

/// Soft cap for sentence / phrase selections (product: whole-phrase sense).
const MAX_PHRASE_WORDS: usize = 40;
/// Above this word count we still treat as phrase (not a single dictionary term).
const PHRASE_MIN_WORDS: usize = 2;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WordHelp {
    pub term_l2: String,
    pub meaning_l1: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GlossaryResult {
    pub term_l2: String,
    /// Primary L1 gloss: for phrases this is the whole-phrase sense.
    pub meaning_l1: String,
    /// Explicit whole-phrase L1 sense when `kind == "phrase"` (same as meaning_l1).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub phrase_sense_l1: Option<String>,
    /// Word-level help (empty for single-word term path, or one entry).
    #[serde(default)]
    pub words: Vec<WordHelp>,
    /// `"term"` or `"phrase"`.
    pub kind: String,
    pub verified: bool,
    pub badge: String,
    pub confidence: f64,
    pub sources: Vec<WebSource>,
    pub cached: bool,
    pub context_snippet: Option<String>,
}

pub fn is_phrase_selection(selection: &str) -> bool {
    selection.split_whitespace().count() >= PHRASE_MIN_WORDS
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
    let word_count = term.split_whitespace().count();
    if word_count > MAX_PHRASE_WORDS {
        return Err(anyhow!(
            "Sélection trop large — resserrez sur une phrase ou une expression (max {MAX_PHRASE_WORDS} mots)"
        ));
    }

    if is_phrase_selection(term) {
        return define_phrase(db, llm, term, context, l1, l2).await;
    }

    define_single_term(db, llm, verifier, term, context, l1, l2).await
}

async fn define_single_term(
    db: &Db,
    llm: &LlmClient,
    verifier: &WebVerifier,
    term: &str,
    context: &str,
    l1: &str,
    l2: &str,
) -> Result<GlossaryResult> {
    if let Some(cached) = db.get_glossary(term).await? {
        let (sources, words, _) = parse_cached_payload(&cached.sources_json);
        return Ok(GlossaryResult {
            term_l2: cached.term_l2.clone(),
            meaning_l1: cached.meaning_l1.clone(),
            phrase_sense_l1: None,
            words: if words.is_empty() {
                vec![WordHelp {
                    term_l2: cached.term_l2,
                    meaning_l1: cached.meaning_l1.clone(),
                }]
            } else {
                words
            },
            kind: "term".into(),
            verified: cached.verified,
            badge: badge_for(cached.verified),
            confidence: cached.confidence,
            sources,
            cached: true,
            context_snippet: cached.context_snippet,
        });
    }

    let snippet = nearest_context(context, term, 400);
    let draft = match draft_meaning(llm, term, &snippet, l1, l2).await {
        Ok(d) => d,
        Err(e) => {
            let m = e.to_string();
            if m.to_lowercase().contains("429") || m.to_lowercase().contains("limite") {
                return Err(e);
            }
            return Err(anyhow!(
                "Sens non disponible pour « {term} » — resserrez la sélection ou continuez la lecture ({m})"
            ));
        }
    };
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
        term_l2: row.term_l2.clone(),
        meaning_l1: row.meaning_l1.clone(),
        phrase_sense_l1: None,
        words: vec![WordHelp {
            term_l2: row.term_l2,
            meaning_l1: row.meaning_l1,
        }],
        kind: "term".into(),
        verified: row.verified,
        badge: badge_for(row.verified),
        confidence: row.confidence,
        sources: reconciled.sources,
        cached: false,
        context_snippet: row.context_snippet,
    })
}

async fn define_phrase(
    db: &Db,
    llm: &LlmClient,
    phrase: &str,
    context: &str,
    l1: &str,
    l2: &str,
) -> Result<GlossaryResult> {
    if let Some(cached) = db.get_glossary(phrase).await? {
        let (sources, words, is_phrase) = parse_cached_payload(&cached.sources_json);
        if is_phrase || !words.is_empty() {
            return Ok(GlossaryResult {
                term_l2: cached.term_l2,
                meaning_l1: cached.meaning_l1.clone(),
                phrase_sense_l1: Some(cached.meaning_l1),
                words,
                kind: "phrase".into(),
                verified: cached.verified,
                badge: "Sens de la phrase — contexte cours".into(),
                confidence: cached.confidence,
                sources,
                cached: true,
                context_snippet: cached.context_snippet,
            });
        }
        // Legacy cache row without word helps — fall through to refresh.
    }

    let snippet = nearest_context(context, phrase, 500)
        .or_else(|| {
            let t = context.trim();
            if t.is_empty() {
                None
            } else {
                Some(t.chars().take(900).collect())
            }
        });

    let drafted = draft_phrase_sense(llm, phrase, &snippet, l1, l2).await?;
    let payload = json!({
        "kind": "phrase",
        "web": [],
        "words": drafted.words,
    });
    let sources_json = payload.to_string();
    let row = db
        .put_glossary(
            phrase,
            &drafted.phrase_sense_l1,
            snippet.as_deref(),
            &sources_json,
            false,
            drafted.confidence,
        )
        .await?;

    Ok(GlossaryResult {
        term_l2: row.term_l2,
        meaning_l1: row.meaning_l1.clone(),
        phrase_sense_l1: Some(row.meaning_l1),
        words: drafted.words,
        kind: "phrase".into(),
        verified: false,
        badge: "Sens de la phrase — contexte cours".into(),
        confidence: drafted.confidence,
        sources: vec![],
        cached: false,
        context_snippet: row.context_snippet,
    })
}

#[derive(Debug)]
struct PhraseDraft {
    phrase_sense_l1: String,
    words: Vec<WordHelp>,
    confidence: f64,
}

async fn draft_phrase_sense(
    llm: &LlmClient,
    phrase: &str,
    snippet: &Option<String>,
    l1: &str,
    l2: &str,
) -> Result<PhraseDraft> {
    let system = format!(
        "You are a bilingual study assistant for learners whose mother tongue is L1 ({l1}). \
         The course / PDF is written in L2 ({l2}). The learner selected a multi-word phrase or sentence. \
         Reply with JSON only: {{\
           \"phrase_l2\":\"...\",\
           \"phrase_sense_l1\":\"...\",\
           \"words\":[{{\"term_l2\":\"...\",\"meaning_l1\":\"...\"}}],\
           \"confidence\":0.0-1.0\
         }}. \
         Rules: \
         (1) phrase_sense_l1 = the meaning of the WHOLE selection in L1 (not a word-by-word gloss), \
             using course_context and the subject of the course when helpful (2–4 short sentences max). \
         (2) words = short L1 glosses (a few words of explanation, not a bare cognate) for important \
             content words / technical terms (skip articles, pronouns, simple auxiliaries). \
         (3) Do not invent facts that contradict the course context."
    );
    let user = json!({
        "phrase_l2": phrase,
        "course_context": snippet,
        "l1": l1,
        "l2": l2,
    })
    .to_string();

    let v = llm
        .chat_json(ModelRole::Interactive, &system, &user)
        .await?;
    let phrase_sense = v
        .get("phrase_sense_l1")
        .and_then(|m| m.as_str())
        .or_else(|| v.get("meaning_l1").and_then(|m| m.as_str()))
        .ok_or_else(|| anyhow!("LLM phrase draft missing phrase_sense_l1"))?
        .trim()
        .to_string();
    if phrase_sense.is_empty() {
        return Err(anyhow!("Sens de la phrase non disponible"));
    }
    let words = parse_words_array(v.get("words")).unwrap_or_else(|| fallback_word_list(phrase));
    let confidence = v
        .get("confidence")
        .and_then(|c| c.as_f64())
        .unwrap_or(0.7)
        .clamp(0.0, 1.0);
    Ok(PhraseDraft {
        phrase_sense_l1: phrase_sense,
        words,
        confidence,
    })
}

fn parse_words_array(v: Option<&Value>) -> Option<Vec<WordHelp>> {
    let arr = v?.as_array()?;
    let mut out = Vec::new();
    for item in arr {
        let term = item
            .get("term_l2")
            .or_else(|| item.get("term"))
            .and_then(|t| t.as_str())
            .unwrap_or("")
            .trim();
        let meaning = item
            .get("meaning_l1")
            .or_else(|| item.get("meaning"))
            .and_then(|t| t.as_str())
            .unwrap_or("")
            .trim();
        if term.is_empty() || meaning.is_empty() {
            continue;
        }
        out.push(WordHelp {
            term_l2: term.to_string(),
            meaning_l1: meaning.to_string(),
        });
    }
    if out.is_empty() {
        None
    } else {
        Some(out)
    }
}

fn fallback_word_list(phrase: &str) -> Vec<WordHelp> {
    let stop: [&str; 40] = [
        "the", "a", "an", "and", "or", "of", "to", "in", "on", "for", "is", "are", "was", "were",
        "be", "by", "as", "at", "it", "its", "this", "that", "with", "from", "le", "la", "les",
        "un", "une", "des", "de", "du", "et", "en", "est", "que", "qui", "dans", "pour", "par",
    ];
    phrase
        .split_whitespace()
        .map(|w| {
            w.trim_matches(|c: char| !c.is_alphanumeric() && c != '-')
                .to_string()
        })
        .filter(|w| w.len() > 2 && !stop.contains(&w.to_lowercase().as_str()))
        .take(8)
        .map(|term_l2| WordHelp {
            meaning_l1: format!("(voir contexte) {term_l2}"),
            term_l2,
        })
        .collect()
}

/// Parse cache payload: legacy `[{web…}]` or `{kind,web,words}` envelope.
fn parse_cached_payload(raw: &str) -> (Vec<WebSource>, Vec<WordHelp>, bool) {
    if let Ok(v) = serde_json::from_str::<Value>(raw) {
        if v.get("kind").and_then(|k| k.as_str()) == Some("phrase")
            || v.get("words").is_some()
        {
            let web = v
                .get("web")
                .and_then(|w| serde_json::from_value::<Vec<WebSource>>(w.clone()).ok())
                .unwrap_or_default();
            let words = parse_words_array(v.get("words")).unwrap_or_default();
            return (web, words, true);
        }
        if let Ok(web) = serde_json::from_value::<Vec<WebSource>>(v.clone()) {
            return (web, vec![], false);
        }
    }
    if let Ok(web) = serde_json::from_str::<Vec<WebSource>>(raw) {
        return (web, vec![], false);
    }
    (vec![], vec![], false)
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
            phrase_sense_l1: None,
            words: vec![WordHelp {
                term_l2: term.into(),
                meaning_l1: draft_meaning.into(),
            }],
            kind: "term".into(),
            verified: false,
            badge: badge_for(false),
            confidence: 0.4,
            sources: vec![],
            cached: false,
            context_snippet: None,
        });
    }

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
            phrase_sense_l1: None,
            words: vec![WordHelp {
                term_l2: term.into(),
                meaning_l1: draft_meaning.into(),
            }],
            kind: "term".into(),
            verified: true,
            badge: badge_for(true),
            confidence: 0.85,
            sources: sources.to_vec(),
            cached: false,
            context_snippet: None,
        });
    }

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
                meaning_l1: meaning.clone(),
                phrase_sense_l1: None,
                words: vec![WordHelp {
                    term_l2: term.into(),
                    meaning_l1: meaning,
                }],
                kind: "term".into(),
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
            phrase_sense_l1: None,
            words: vec![WordHelp {
                term_l2: term.into(),
                meaning_l1: draft_meaning.into(),
            }],
            kind: "term".into(),
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
    hits * 2 >= tokens.len()
}

fn nearest_context(text: &str, term: &str, radius: usize) -> Option<String> {
    let lower = text.to_lowercase();
    let needle = term.to_lowercase();
    let pos = lower.find(&needle)?;
    let start = pos.saturating_sub(radius);
    let end = (pos + needle.len() + radius).min(text.len());
    let slice = text.get(start..end).unwrap_or(term);
    Some(slice.trim().to_string())
}
