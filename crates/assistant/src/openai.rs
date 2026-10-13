//! The OpenAI-compatible chat-completions provider (Ollama, llama.cpp, LM Studio, OpenRouter,
//! OpenAI and other endpoints that speak `POST {base}/chat/completions`).

use std::time::Duration;

use serde_json::Value;

use crate::provider::Provider;

/// A model reply larger than this is refused.
const MAX_RESPONSE_BYTES: u64 = 8 * 1024 * 1024;
/// How long one model call may take (local models on a CPU can be slow).
const TIMEOUT: Duration = Duration::from_secs(300);

pub struct OpenAiCompatible {
    base_url: String,
    api_key: Option<String>,
    agent: ureq::Agent,
}

impl OpenAiCompatible {
    /// `base_url` is the API root, e.g. `http://localhost:11434/v1` (Ollama) or
    /// `https://openrouter.ai/api/v1`. The key, when given, is sent as a bearer token.
    pub fn new(base_url: &str, api_key: Option<String>) -> Self {
        let config = ureq::Agent::config_builder().timeout_global(Some(TIMEOUT)).http_status_as_error(false).build();
        Self { base_url: base_url.trim().trim_end_matches('/').to_string(), api_key: api_key.filter(|k| !k.trim().is_empty()), agent: config.into() }
    }
}

impl Provider for OpenAiCompatible {
    fn complete(&mut self, request: &Value) -> Result<Value, String> {
        let url = format!("{}/chat/completions", self.base_url);
        let mut req = self.agent.post(&url).header("Content-Type", "application/json");
        if let Some(key) = &self.api_key {
            req = req.header("Authorization", &format!("Bearer {key}"));
        }
        let mut resp = req.send(request.to_string()).map_err(|e| format!("{url}: {e}"))?;
        let status = resp.status().as_u16();
        let body = resp.body_mut().with_config().limit(MAX_RESPONSE_BYTES).read_to_string().map_err(|e| format!("{url}: {e}"))?;
        let v: Value = serde_json::from_str(&body).map_err(|_| format!("{url}: HTTP {status}, and the reply is not JSON"))?;
        if !(200..300).contains(&status) {
            let msg = v.pointer("/error/message").and_then(Value::as_str).map_or_else(|| v.to_string(), str::to_string);
            return Err(format!("{url}: HTTP {status}: {msg}"));
        }
        Ok(v)
    }
}
