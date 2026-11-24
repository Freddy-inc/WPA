//! PDF text extraction via `pdf-extract` + per-page locate via `lopdf` (no Python).

use std::path::Path;

use anyhow::{anyhow, Result};
use lopdf::Document;

#[derive(Debug, Clone)]
pub struct ExtractedPdf {
    pub filename: String,
    pub text: String,
    pub char_count: usize,
}

/// Extract plain text from PDF bytes. Fails clearly for scans / empty text layers.
pub fn extract_text(filename: &str, bytes: &[u8]) -> Result<ExtractedPdf> {
    if bytes.is_empty() {
        return Err(anyhow!("PDF vide"));
    }
    if !bytes.starts_with(b"%PDF") {
        return Err(anyhow!("Fichier non PDF (signature manquante)"));
    }

    let text = pdf_extract::extract_text_from_mem(bytes).map_err(|e| {
        anyhow!("Texte non extractible — choisissez un autre fichier ({e})")
    })?;

    let cleaned = normalize_whitespace(&text);
    if cleaned.chars().filter(|c| c.is_alphanumeric()).count() < 40 {
        return Err(anyhow!(
            "Texte non extractible (PDF scanné ou sans couche texte) — essayez un autre fichier"
        ));
    }

    Ok(ExtractedPdf {
        filename: filename.to_string(),
        char_count: cleaned.chars().count(),
        text: cleaned,
    })
}

/// Per-page plain text (Rust) — used for Error→PDF page locate.
/// Splits the PDF with `lopdf`, then runs `pdf-extract` on each single-page doc.
pub fn extract_pages_from_path(path: &Path) -> Result<Vec<String>> {
    let bytes = std::fs::read(path)
        .map_err(|e| anyhow!("lecture PDF {}: {e}", path.display()))?;
    extract_pages_from_bytes(&bytes)
}

pub fn extract_pages_from_bytes(bytes: &[u8]) -> Result<Vec<String>> {
    if bytes.is_empty() || !bytes.starts_with(b"%PDF") {
        return Err(anyhow!("PDF invalide pour extraction par page"));
    }
    let doc = Document::load_mem(bytes).map_err(|e| anyhow!("lopdf load: {e}"))?;
    let mut nums: Vec<u32> = doc.get_pages().keys().copied().collect();
    nums.sort_unstable();
    if nums.is_empty() {
        return Err(anyhow!("PDF sans pages"));
    }
    let mut pages = Vec::with_capacity(nums.len());
    for num in nums {
        let mut one = Document::load_mem(bytes).map_err(|e| anyhow!("lopdf page {num}: {e}"))?;
        let others: Vec<u32> = one
            .get_pages()
            .keys()
            .copied()
            .filter(|&n| n != num)
            .collect();
        if !others.is_empty() {
            one.delete_pages(&others);
        }
        let mut buf = Vec::new();
        one.save_to(&mut buf)
            .map_err(|e| anyhow!("lopdf save page {num}: {e}"))?;
        let text = pdf_extract::extract_text_from_mem(&buf).unwrap_or_default();
        pages.push(normalize_whitespace(&text));
    }
    if pages.iter().all(|p| p.trim().is_empty()) {
        return Err(anyhow!("aucune couche texte par page"));
    }
    Ok(pages)
}

fn normalize_whitespace(s: &str) -> String {
    s.lines()
        .map(|l| l.trim())
        .filter(|l| !l.is_empty())
        .collect::<Vec<_>>()
        .join("\n")
}

/// Split course text into overlapping-ish chunks for batch LLM calls.
pub fn chunk_text(text: &str, max_chars: usize) -> Vec<String> {
    if text.chars().count() <= max_chars {
        return vec![text.to_string()];
    }
    let mut chunks = Vec::new();
    let mut current = String::new();
    for para in text.split('\n') {
        if current.chars().count() + para.chars().count() + 1 > max_chars && !current.is_empty() {
            chunks.push(current.trim().to_string());
            current.clear();
        }
        if !current.is_empty() {
            current.push('\n');
        }
        current.push_str(para);
    }
    if !current.trim().is_empty() {
        chunks.push(current.trim().to_string());
    }
    chunks
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_non_pdf() {
        let err = extract_text("x.bin", b"hello").unwrap_err();
        assert!(err.to_string().contains("non PDF"));
    }

    #[test]
    fn chunks_respect_size() {
        let text = "a\n".repeat(100);
        let chunks = chunk_text(&text, 50);
        assert!(chunks.len() > 1);
        assert!(chunks.iter().all(|c| c.chars().count() <= 60));
    }

    #[test]
    fn extract_pages_rejects_garbage() {
        let err = extract_pages_from_bytes(b"not-a-pdf").unwrap_err();
        assert!(err.to_string().contains("invalide"));
    }
}
