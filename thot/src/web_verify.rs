//! Official/public web sources for glossary cross-check (Wiktionary + Wikipedia).

use anyhow::Result;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use tracing::warn;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WebSource {
    pub kind: String,
    pub title: String,
    pub url: String,
    pub excerpt: String,
}

#[derive(Clone)]
pub struct WebVerifier {
    http: Client,
}

impl WebVerifier {
    pub fn new() -> Self {
        let http = Client::builder()
            .user_agent("ThotMVP/0.1 (educational; contact: thot@local)")
            .timeout(std::time::Duration::from_secs(15))
            .build()
            .expect("reqwest");
        Self { http }
    }

    /// Fetch Wiktionary + Wikipedia excerpts for a term in L2 language code (e.g. en, fr).
    pub async fn lookup(&self, term: &str, l2_lang: &str) -> Vec<WebSource> {
        let lang = normalize_lang(l2_lang);
        let mut sources = Vec::new();
        match self.wiktionary(term, &lang).await {
            Ok(Some(s)) => sources.push(s),
            Ok(None) => {}
            Err(e) => warn!(error = %e, "Wiktionary lookup failed"),
        }
        match self.wikipedia(term, &lang).await {
            Ok(Some(s)) => sources.push(s),
            Ok(None) => {}
            Err(e) => warn!(error = %e, "Wikipedia lookup failed"),
        }
        sources
    }

    async fn wiktionary(&self, term: &str, lang: &str) -> Result<Option<WebSource>> {
        // REST definition API returns structured glosses (extracts API is often empty on Wiktionary).
        let url = format!(
            "https://{lang}.wiktionary.org/api/rest_v1/page/definition/{}",
            urlencoding_lite(term)
        );
        let resp = self.http.get(&url).send().await?;
        if resp.status().as_u16() == 404 {
            return Ok(None);
        }
        if !resp.status().is_success() {
            return Ok(None);
        }
        let v: serde_json::Value = resp.json().await?;
        // Keys are language names or codes depending on edition; take first non-empty definition.
        let mut excerpt = String::new();
        if let Some(obj) = v.as_object() {
            for (_lang_key, entries) in obj {
                let Some(arr) = entries.as_array() else { continue };
                for entry in arr {
                    let defs = entry
                        .get("definitions")
                        .and_then(|d| d.as_array())
                        .cloned()
                        .unwrap_or_default();
                    for def in defs {
                        if let Some(raw) = def.get("definition").and_then(|d| d.as_str()) {
                            let plain = strip_html(raw);
                            if !plain.is_empty() {
                                excerpt = plain;
                                break;
                            }
                        }
                    }
                    if !excerpt.is_empty() {
                        break;
                    }
                }
                if !excerpt.is_empty() {
                    break;
                }
            }
        }
        if excerpt.is_empty() {
            return Ok(None);
        }
        let page_url = format!(
            "https://{lang}.wiktionary.org/wiki/{}",
            urlencoding_lite(term)
        );
        Ok(Some(WebSource {
            kind: "wiktionary".into(),
            title: term.into(),
            url: page_url,
            excerpt: truncate(&excerpt, 500),
        }))
    }

    async fn wikipedia(&self, term: &str, lang: &str) -> Result<Option<WebSource>> {
        let url = format!(
            "https://{lang}.wikipedia.org/api/rest_v1/page/summary/{}",
            urlencoding_lite(term)
        );
        let resp = self.http.get(&url).send().await?;
        if resp.status().as_u16() == 404 {
            return Ok(None);
        }
        if !resp.status().is_success() {
            return Ok(None);
        }
        let v: serde_json::Value = resp.json().await?;
        let extract = v
            .get("extract")
            .and_then(|e| e.as_str())
            .unwrap_or("")
            .trim();
        if extract.is_empty() {
            return Ok(None);
        }
        let title = v
            .get("title")
            .and_then(|t| t.as_str())
            .unwrap_or(term);
        let page_url = v
            .pointer("/content_urls/desktop/page")
            .and_then(|u| u.as_str())
            .map(|s| s.to_string())
            .unwrap_or_else(|| {
                format!(
                    "https://{lang}.wikipedia.org/wiki/{}",
                    urlencoding_lite(title)
                )
            });
        Ok(Some(WebSource {
            kind: "wikipedia".into(),
            title: title.into(),
            url: page_url,
            excerpt: truncate(extract, 500),
        }))
    }
}

fn normalize_lang(lang: &str) -> String {
    let l = lang.trim().to_lowercase();
    match l.as_str() {
        "english" | "en" | "eng" => "en".into(),
        "french" | "fr" | "fra" | "français" | "francais" => "fr".into(),
        "spanish" | "es" | "spa" | "español" | "espanol" => "es".into(),
        "german" | "de" | "deu" | "deutsch" => "de".into(),
        "italian" | "it" | "ita" => "it".into(),
        "portuguese" | "pt" | "por" => "pt".into(),
        s if s.len() == 2 => s.to_string(),
        _ => "en".into(),
    }
}

fn strip_html(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut in_tag = false;
    for c in s.chars() {
        match c {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if !in_tag => out.push(c),
            _ => {}
        }
    }
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn truncate(s: &str, max: usize) -> String {
    let mut out: String = s.chars().take(max).collect();
    if s.chars().count() > max {
        out.push('…');
    }
    out
}

fn urlencoding_lite(s: &str) -> String {
    let mut out = String::new();
    for c in s.chars() {
        match c {
            'A'..='Z' | 'a'..='z' | '0'..='9' | '-' | '_' | '.' | '~' => out.push(c),
            ' ' => out.push('_'),
            _ => {
                for b in c.to_string().as_bytes() {
                    out.push_str(&format!("%{b:02X}"));
                }
            }
        }
    }
    out
}
