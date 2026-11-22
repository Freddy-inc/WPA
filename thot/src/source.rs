//! Locate a missed notion / answer inside the active course PDF corpus.

use anyhow::{anyhow, Result};
use serde::Serialize;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use tokio::process::Command;

use crate::db::{Db, DocumentRow};
use crate::generate::QuizMiss;

#[derive(Debug, Clone, Serialize)]
pub struct SourceHit {
    pub notion: String,
    pub query: String,
    pub doc_id: i64,
    pub filename: String,
    pub page: i64,
    pub total_pages: i64,
    pub snippet: String,
    pub score: f64,
}

/// Best PDF page for each quiz miss (course docs only — never a random file).
pub async fn locate_misses(
    db: &Db,
    data_dir: &Path,
    misses: &[QuizMiss],
) -> Result<Vec<SourceHit>> {
    let course_id = db.active_course_id().await?;
    let docs = db.list_documents(course_id).await?;
    if docs.is_empty() {
        return Err(anyhow!("Aucun document dans ce cours"));
    }
    let mut out = Vec::new();
    for m in misses {
        let notion = if m.notion.trim().is_empty() {
            m.question.clone()
        } else {
            m.notion.trim().to_string()
        };
        let query = best_query(m);
        if let Some(hit) = locate_query(db, data_dir, &docs, &notion, &query).await? {
            out.push(hit);
        }
    }
    Ok(out)
}

pub async fn locate_query_str(
    db: &Db,
    data_dir: &Path,
    notion: &str,
    query: &str,
) -> Result<Option<SourceHit>> {
    let course_id = db.active_course_id().await?;
    let docs = db.list_documents(course_id).await?;
    if docs.is_empty() {
        return Ok(None);
    }
    locate_query(db, data_dir, &docs, notion, query).await
}

fn best_query(m: &QuizMiss) -> String {
    for cand in [&m.notion, &m.correct_answer, &m.question] {
        let t = cand.trim();
        if t.chars().count() >= 3 {
            return t.to_string();
        }
    }
    "cours".into()
}

async fn locate_query(
    db: &Db,
    data_dir: &Path,
    docs: &[DocumentRow],
    notion: &str,
    query: &str,
) -> Result<Option<SourceHit>> {
    let mut best: Option<SourceHit> = None;
    for doc in docs {
        let pages = page_texts_for_doc(db, data_dir, doc).await?;
        if pages.is_empty() {
            continue;
        }
        let total = pages.len() as i64;
        for (i, page_text) in pages.iter().enumerate() {
            let score = score_match(page_text, query, notion);
            if score <= 0.0 {
                continue;
            }
            let page = (i as i64) + 1;
            let snippet = snippet_around(page_text, query, 120);
            let hit = SourceHit {
                notion: notion.to_string(),
                query: query.to_string(),
                doc_id: doc.id,
                filename: doc.filename.clone(),
                page,
                total_pages: total.max(doc.total_pages.max(1)),
                snippet,
                score,
            };
            if best.as_ref().map(|b| b.score).unwrap_or(0.0) < score {
                best = Some(hit);
            }
        }
    }
    if best.is_none() {
        // Fallback: active / first doc page 1 with a short preview.
        if let Some(doc) = docs.iter().find(|d| d.active).or_else(|| docs.first()) {
            let preview: String = doc.text.chars().take(120).collect();
            best = Some(SourceHit {
                notion: notion.to_string(),
                query: query.to_string(),
                doc_id: doc.id,
                filename: doc.filename.clone(),
                page: 1,
                total_pages: doc.total_pages.max(1),
                snippet: preview,
                score: 0.01,
            });
        }
    }
    Ok(best)
}

async fn page_texts_for_doc(
    db: &Db,
    data_dir: &Path,
    doc: &DocumentRow,
) -> Result<Vec<String>> {
    let pdf_path = data_dir.join("docs").join(format!("{}.pdf", doc.id));
    if pdf_path.is_file() {
        if let Ok(pages) = extract_pages_python(&pdf_path).await {
            if !pages.is_empty() && pages.iter().any(|p| !p.trim().is_empty()) {
                let n = pages.len() as i64;
                if n > 0 && n != doc.total_pages {
                    let _ = db.set_document_total_pages(doc.id, n).await;
                }
                return Ok(pages);
            }
        }
    }
    Ok(estimate_pages_from_text(&doc.text, doc.total_pages.max(1)))
}

fn estimate_pages_from_text(text: &str, total_pages: i64) -> Vec<String> {
    let n = total_pages.max(1) as usize;
    if n == 1 || text.trim().is_empty() {
        return vec![text.to_string()];
    }
    // Prefer form-feed splits when the extractor left them.
    let by_ff: Vec<String> = text
        .split('\u{c}')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();
    if by_ff.len() == n {
        return by_ff;
    }
    let chars: Vec<char> = text.chars().collect();
    let len = chars.len().max(1);
    let mut pages = Vec::with_capacity(n);
    for i in 0..n {
        let start = i * len / n;
        let end = (i + 1) * len / n;
        pages.push(chars[start..end].iter().collect());
    }
    pages
}

async fn extract_pages_python(pdf_path: &Path) -> Result<Vec<String>> {
    let script = resolve_pdf_pages_script()?;
    let out = Command::new("python3")
        .arg(&script)
        .arg("--pdf")
        .arg(pdf_path)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .await?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        return Err(anyhow!("pdf_pages failed: {err}"));
    }
    let pages: Vec<String> = serde_json::from_slice(&out.stdout)?;
    Ok(pages)
}

fn resolve_pdf_pages_script() -> Result<PathBuf> {
    let candidates = [
        PathBuf::from("scripts/pdf_pages.py"),
        PathBuf::from("thot/scripts/pdf_pages.py"),
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("scripts/pdf_pages.py"),
    ];
    for c in candidates {
        if c.is_file() {
            return Ok(c);
        }
    }
    Err(anyhow!("scripts/pdf_pages.py introuvable"))
}

fn score_match(page_text: &str, query: &str, notion: &str) -> f64 {
    let hay = normalize(page_text);
    if hay.is_empty() {
        return 0.0;
    }
    let mut score = 0.0;
    let q = normalize(query);
    let n = normalize(notion);
    if !q.is_empty() && hay.contains(&q) {
        score += 5.0 + (q.chars().count() as f64) * 0.05;
    }
    if !n.is_empty() && n != q && hay.contains(&n) {
        score += 3.5;
    }
    for token in q.split_whitespace().chain(n.split_whitespace()) {
        if token.chars().count() < 4 {
            continue;
        }
        if hay.contains(token) {
            score += 1.0;
        }
    }
    score
}

fn normalize(s: &str) -> String {
    s.chars()
        .map(|c| c.to_lowercase().next().unwrap_or(c))
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn snippet_around(text: &str, query: &str, radius: usize) -> String {
    let lower = text.to_lowercase();
    let q = query.trim().to_lowercase();
    let idx = if q.is_empty() {
        0
    } else {
        lower.find(&q).unwrap_or(0)
    };
    let start = idx.saturating_sub(radius);
    let end = (idx + q.len() + radius).min(text.len());
    let mut snip: String = text
        .chars()
        .skip(start)
        .take(end.saturating_sub(start))
        .collect();
    if start > 0 {
        snip = format!("…{snip}");
    }
    if end < text.len() {
        snip = format!("{snip}…");
    }
    snip
}
