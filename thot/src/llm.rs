//! OpenAI-compatible LLM client: Groq (primary), Ollama fallback, optional stub for demos.

use anyhow::{anyhow, Context, Result};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tracing::{info, warn};

const GROQ_BASE: &str = "https://api.groq.com/openai/v1";
const OLLAMA_BASE: &str = "http://127.0.0.1:11434/v1";

/// Interactive (glossary) vs batch (fiches/quiz) model roles.
#[derive(Debug, Clone, Copy)]
pub enum ModelRole {
    Interactive,
    Batch,
}

#[derive(Debug, Clone)]
pub struct LlmConfig {
    pub groq_api_key: Option<String>,
    /// Which env var supplied the Groq key: `"GROQ_API_KEY"` | `"Etude"` | none.
    pub groq_key_env: Option<&'static str>,
    pub groq_interactive_model: String,
    pub groq_batch_model: String,
    pub ollama_base: String,
    pub ollama_model: String,
    /// When true, skip remote LLMs and return deterministic demo JSON.
    pub force_stub: bool,
}

/// Resolve Groq API key: prefer `GROQ_API_KEY`, else fallback `Etude`. Never log the value.
pub fn resolve_groq_key() -> (Option<String>, Option<&'static str>) {
    let from_groq = std::env::var("GROQ_API_KEY")
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|k| !k.is_empty());
    if from_groq.is_some() {
        return (from_groq, Some("GROQ_API_KEY"));
    }
    let from_etude = std::env::var("Etude")
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|k| !k.is_empty());
    if from_etude.is_some() {
        return (from_etude, Some("Etude"));
    }
    (None, None)
}

impl Default for LlmConfig {
    fn default() -> Self {
        let force_stub = matches!(
            std::env::var("THOT_LLM_STUB").as_deref(),
            Ok("1") | Ok("true") | Ok("yes")
        );
        let (groq_api_key, groq_key_env) = resolve_groq_key();
        Self {
            groq_api_key,
            groq_key_env,
            groq_interactive_model: std::env::var("THOT_GROQ_INTERACTIVE_MODEL")
                .unwrap_or_else(|_| "openai/gpt-oss-20b".into()),
            groq_batch_model: std::env::var("THOT_GROQ_BATCH_MODEL")
                .unwrap_or_else(|_| "openai/gpt-oss-120b".into()),
            ollama_base: std::env::var("THOT_OLLAMA_BASE")
                .unwrap_or_else(|_| OLLAMA_BASE.into()),
            ollama_model: std::env::var("THOT_OLLAMA_MODEL")
                .unwrap_or_else(|_| "qwen2.5".into()),
            force_stub,
        }
    }
}

#[derive(Debug, Clone)]
pub struct LlmClient {
    http: Client,
    config: LlmConfig,
    /// Becomes true after first successful stub fallback (or when force_stub).
    used_stub: std::sync::Arc<std::sync::atomic::AtomicBool>,
}

#[derive(Debug, Serialize)]
struct ChatRequest {
    model: String,
    messages: Vec<ChatMessage>,
    temperature: f32,
    #[serde(skip_serializing_if = "Option::is_none")]
    response_format: Option<Value>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct ChatMessage {
    pub role: String,
    pub content: String,
}

#[derive(Debug, Deserialize)]
struct ChatResponse {
    choices: Vec<Choice>,
}

#[derive(Debug, Deserialize)]
struct Choice {
    message: ChatMessage,
}

impl LlmClient {
    pub fn new(config: LlmConfig) -> Self {
        let http = Client::builder()
            .user_agent("ThotMVP/0.1 (+https://github.com/Freddy-inc/WPA)")
            .timeout(std::time::Duration::from_secs(120))
            .build()
            .expect("reqwest client");
        let used_stub = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(
            config.force_stub,
        ));
        Self {
            http,
            config,
            used_stub,
        }
    }

    pub fn provider_label(&self) -> &'static str {
        if self.config.force_stub
            || self
                .used_stub
                .load(std::sync::atomic::Ordering::Relaxed)
        {
            return "stub";
        }
        if self.config.groq_api_key.is_some() {
            "groq"
        } else {
            "ollama"
        }
    }

    pub fn has_groq_key(&self) -> bool {
        self.config.groq_api_key.is_some()
    }

    /// Env var name that provided the Groq key (`GROQ_API_KEY` or `Etude`), if any.
    pub fn groq_key_env_name(&self) -> Option<&'static str> {
        self.config.groq_key_env
    }

    pub fn is_stub(&self) -> bool {
        self.config.force_stub
            || self
                .used_stub
                .load(std::sync::atomic::Ordering::Relaxed)
    }

    /// Chat completion returning raw assistant text.
    pub async fn chat(
        &self,
        role: ModelRole,
        system: &str,
        user: &str,
        json_mode: bool,
    ) -> Result<String> {
        if self.config.force_stub {
            info!(provider = "stub", "Using forced LLM stub");
            return Ok(stub_response(system, user));
        }

        let messages = vec![
            ChatMessage {
                role: "system".into(),
                content: system.into(),
            },
            ChatMessage {
                role: "user".into(),
                content: user.into(),
            },
        ];

        if let Some(key) = &self.config.groq_api_key {
            match self
                .call_openai_compatible(
                    GROQ_BASE,
                    Some(key),
                    self.model_for(role, true),
                    &messages,
                    json_mode,
                )
                .await
            {
                Ok(text) => {
                    info!(provider = "groq", "LLM call succeeded");
                    return Ok(text);
                }
                Err(err) => {
                    warn!(error = %err, "Groq failed; trying Ollama fallback");
                }
            }
        } else {
            info!("No GROQ_API_KEY; trying Ollama");
        }

        match self
            .call_openai_compatible(
                &self.config.ollama_base,
                None,
                self.model_for(role, false),
                &messages,
                json_mode,
            )
            .await
        {
            Ok(text) => {
                info!(provider = "ollama", "LLM call succeeded");
                Ok(text)
            }
            Err(err) => {
                warn!(error = %err, "Ollama failed; using stub responses");
                self.used_stub
                    .store(true, std::sync::atomic::Ordering::Relaxed);
                Ok(stub_response(system, user))
            }
        }
    }

    /// Chat and parse the assistant content as JSON.
    pub async fn chat_json(
        &self,
        role: ModelRole,
        system: &str,
        user: &str,
    ) -> Result<Value> {
        let text = self.chat(role, system, user, true).await?;
        parse_json_loose(&text)
    }

    fn model_for(&self, role: ModelRole, groq: bool) -> &str {
        if !groq {
            return &self.config.ollama_model;
        }
        match role {
            ModelRole::Interactive => &self.config.groq_interactive_model,
            ModelRole::Batch => &self.config.groq_batch_model,
        }
    }

    async fn call_openai_compatible(
        &self,
        base: &str,
        api_key: Option<&str>,
        model: &str,
        messages: &[ChatMessage],
        json_mode: bool,
    ) -> Result<String> {
        let url = format!("{}/chat/completions", base.trim_end_matches('/'));
        let body = ChatRequest {
            model: model.into(),
            messages: messages.to_vec(),
            temperature: 0.2,
            response_format: if json_mode {
                Some(json!({ "type": "json_object" }))
            } else {
                None
            },
        };

        let mut req = self.http.post(&url).json(&body);
        if let Some(key) = api_key {
            req = req.bearer_auth(key);
        }

        let resp = req.send().await.context("HTTP request to LLM")?;
        let status = resp.status();
        let body_text = resp.text().await.unwrap_or_default();
        if !status.is_success() {
            return Err(anyhow!("LLM HTTP {status}: {body_text}"));
        }

        let parsed: ChatResponse =
            serde_json::from_str(&body_text).context("parse chat completions response")?;
        let content = parsed
            .choices
            .first()
            .map(|c| c.message.content.clone())
            .ok_or_else(|| anyhow!("empty choices from LLM"))?;
        Ok(content)
    }
}

/// Deterministic demo responses when no live LLM is available.
fn stub_response(system: &str, user: &str) -> String {
    let blob = format!("{system}\n{user}").to_lowercase();
    if blob.contains("\"fiches\"") || blob.contains("flashcard") {
        return json!({
            "fiches": [
                {
                    "front": "[STUB] Photosynthesis",
                    "back": "Processus par lequel les plantes convertissent la lumière en énergie chimique (glucose)."
                },
                {
                    "front": "[STUB] Chlorophyll",
                    "back": "Pigment vert des chloroplastes qui absorbe la lumière du soleil."
                },
                {
                    "front": "[STUB] Calvin cycle",
                    "back": "Fixation du carbone dans le stroma ; produit des glucides."
                },
                {
                    "front": "[STUB] Thylakoid membrane",
                    "back": "Lieu des réactions dépendantes de la lumière."
                },
                {
                    "front": "[STUB] Stomata",
                    "back": "Ouvertures qui régulent les échanges gazeux (CO₂ / O₂)."
                }
            ]
        })
        .to_string();
    }
    if blob.contains("\"questions\"") || blob.contains("multiple-choice quiz") {
        return json!({
            "questions": [
                {
                    "question": "[STUB] What does photosynthesis convert light energy into?",
                    "choices": ["Heat only", "Chemical energy (glucose)", "Sound", "Magnetic fields"],
                    "answer_index": 1,
                    "notion": "photosynthesis"
                },
                {
                    "question": "[STUB] Where do light-dependent reactions occur?",
                    "choices": ["Nucleus", "Stroma", "Thylakoid membrane", "Cell wall"],
                    "answer_index": 2,
                    "notion": "thylakoid"
                },
                {
                    "question": "[STUB] Which gas is released as a byproduct?",
                    "choices": ["Nitrogen", "Oxygen", "Methane", "Helium"],
                    "answer_index": 1,
                    "notion": "oxygen"
                },
                {
                    "question": "[STUB] What pigment absorbs sunlight?",
                    "choices": ["Hemoglobin", "Melanin", "Chlorophyll", "Keratin"],
                    "answer_index": 2,
                    "notion": "chlorophyll"
                },
                {
                    "question": "[STUB] Where does the Calvin cycle fix carbon?",
                    "choices": ["Stroma", "Mitochondria", "Vacuole", "Cytoplasm only"],
                    "answer_index": 0,
                    "notion": "Calvin cycle"
                },
                {
                    "question": "[STUB] What are the main inputs of photosynthesis?",
                    "choices": ["CO₂ and water", "Nitrogen only", "Salt and sand", "Helium"],
                    "answer_index": 0,
                    "notion": "inputs"
                },
                {
                    "question": "[STUB] Where is chemical energy stored after photosynthesis?",
                    "choices": ["As glucose", "As pure heat", "As sound", "As magnetic fields"],
                    "answer_index": 0,
                    "notion": "glucose"
                }
            ]
        })
        .to_string();
    }

    // Glossary draft / reconcile
    let term = extract_term_hint(user).unwrap_or_else(|| "term".into());
    json!({
        "term_l2": term,
        "meaning_l1": format!(
            "[STUB] Sens pédagogique de « {term} » dans le contexte du cours (démo sans LLM distant)."
        ),
        "verified": false,
        "confidence": 0.55
    })
    .to_string()
}

fn extract_term_hint(user: &str) -> Option<String> {
    if let Ok(v) = serde_json::from_str::<Value>(user) {
        if let Some(t) = v.get("term_l2").and_then(|x| x.as_str()) {
            return Some(t.to_string());
        }
    }
    None
}

/// Strip markdown fences and parse JSON.
pub fn parse_json_loose(text: &str) -> Result<Value> {
    let trimmed = text.trim();
    let candidate = if let Some(start) = trimmed.find('{') {
        let end = trimmed.rfind('}').unwrap_or(trimmed.len() - 1);
        &trimmed[start..=end]
    } else if let Some(start) = trimmed.find('[') {
        let end = trimmed.rfind(']').unwrap_or(trimmed.len() - 1);
        &trimmed[start..=end]
    } else {
        trimmed
    };
    serde_json::from_str(candidate).with_context(|| format!("invalid JSON from LLM: {text}"))
}
