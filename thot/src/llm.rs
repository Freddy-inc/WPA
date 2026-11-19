//! OpenAI-compatible LLM client: Groq (primary) with Ollama fallback.

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
    pub groq_interactive_model: String,
    pub groq_batch_model: String,
    pub ollama_base: String,
    pub ollama_model: String,
}

impl Default for LlmConfig {
    fn default() -> Self {
        Self {
            groq_api_key: std::env::var("GROQ_API_KEY").ok().filter(|k| !k.is_empty()),
            groq_interactive_model: std::env::var("THOT_GROQ_INTERACTIVE_MODEL")
                .unwrap_or_else(|_| "openai/gpt-oss-20b".into()),
            groq_batch_model: std::env::var("THOT_GROQ_BATCH_MODEL")
                .unwrap_or_else(|_| "openai/gpt-oss-120b".into()),
            ollama_base: std::env::var("THOT_OLLAMA_BASE")
                .unwrap_or_else(|_| OLLAMA_BASE.into()),
            ollama_model: std::env::var("THOT_OLLAMA_MODEL")
                .unwrap_or_else(|_| "qwen2.5".into()),
        }
    }
}

#[derive(Debug, Clone)]
pub struct LlmClient {
    http: Client,
    config: LlmConfig,
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
        Self { http, config }
    }

    pub fn provider_label(&self) -> &'static str {
        if self.config.groq_api_key.is_some() {
            "groq"
        } else {
            "ollama"
        }
    }

    pub fn has_groq_key(&self) -> bool {
        self.config.groq_api_key.is_some()
    }

    /// Chat completion returning raw assistant text.
    pub async fn chat(
        &self,
        role: ModelRole,
        system: &str,
        user: &str,
        json_mode: bool,
    ) -> Result<String> {
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
            info!("No GROQ_API_KEY; using Ollama");
        }

        self.call_openai_compatible(
            &self.config.ollama_base,
            None,
            self.model_for(role, false),
            &messages,
            json_mode,
        )
        .await
        .context("Ollama LLM call failed (is Ollama running with the configured model?)")
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
