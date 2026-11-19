//! PDF text extraction via `pdf-extract`.

use anyhow::{anyhow, Result};

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

    let text = pdf_extract::extract_text_from_mem(bytes)
        .map_err(|e| anyhow!("Import impossible : texte non extractible ({e})"))?;

    let cleaned = normalize_whitespace(&text);
    if cleaned.chars().filter(|c| c.is_alphanumeric()).count() < 40 {
        return Err(anyhow!(
            "Import impossible : texte non extractible (PDF scanné ou couche texte absente)"
        ));
    }

    Ok(ExtractedPdf {
        filename: filename.to_string(),
        char_count: cleaned.chars().count(),
        text: cleaned,
    })
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
}
