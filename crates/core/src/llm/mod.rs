//! The LLM call (Phase 0): one blocking chat-completion against an
//! OpenAI-compatible endpoint. Streaming (SSE) and the provider booth
//! split arrive with Phase 1 — Phase 0 needs exactly one round trip to
//! prove the loop.
//!
//! Model identity stays gravity-side (ADR-0007): the endpoint, model,
//! and key live in gravity's config/env, never in krystallizer's data.
//!
//! Submodules:
//! - [`sse`] — the SSE wire decoder (the one home for the framing and
//!   chunk shape; the provider guest and any streaming consumer ride it).

pub mod sse;

use anyhow::Context;

/// One chat message on the wire (the OpenAI wire shape — an interface
/// currency; krystallizer's storage never sees this form).
#[derive(serde::Serialize, Clone, Debug)]
pub struct WireMessage {
    pub role: String,
    pub content: String,
}

/// The endpoint handle: endpoint URL + model + resolved key.
#[derive(Clone, Debug)]
pub struct LlmClient {
    endpoint: String,
    model: String,
    api_key: String,
    /// The env var NAME the key was resolved from (kept so the
    /// provider booth can re-read per call — rotation without reload).
    api_key_env_name: String,
    http: reqwest::Client,
}

impl LlmClient {
    /// Resolve once per turn (not per request): the key comes from the
    /// env var named in the config and is held only inside this handle.
    pub fn from_config(config: &gravity_config::GravityConfig) -> anyhow::Result<Self> {
        Ok(Self {
            endpoint: config.endpoint.trim_end_matches('/').to_string(),
            model: config.model.clone(),
            api_key: config.api_key()?,
            api_key_env_name: config.api_key_env.clone(),
            http: reqwest::Client::new(),
        })
    }

    /// The endpoint URL (the provider booth's arg — the key itself
    /// never crosses back out of this handle).
    pub fn endpoint(&self) -> &str {
        &self.endpoint
    }

    /// The model id (the provider booth's arg).
    pub fn model(&self) -> &str {
        &self.model
    }

    /// The env var NAME the key was resolved from (the provider booth
    /// re-reads it per call — rotation without a reload).
    pub fn api_key_env(&self) -> &str {
        &self.api_key_env_name
    }

    /// One chat completion: send the full message list, return the
    /// assistant's reply text. No retry yet — transport-level
    /// retry/backoff is Phase 1's provider-booth responsibility.
    pub async fn complete(&self, messages: &[WireMessage]) -> anyhow::Result<String> {
        let url = format!("{}/chat/completions", self.endpoint);
        let body = serde_json::json!({
            "model": self.model,
            "messages": messages,
        });
        let reply = self
            .http
            .post(&url)
            .bearer_auth(&self.api_key)
            .json(&body)
            .send()
            .await
            .context("LLM request failed (transport)")?
            .error_for_status()
            .context("LLM request rejected (HTTP status)")?
            .json::<serde_json::Value>()
            .await
            .context("LLM response was not JSON")?;
        reply["choices"][0]["message"]["content"]
            .as_str()
            .map(str::to_string)
            .context("LLM response missing choices[0].message.content")
    }
}
