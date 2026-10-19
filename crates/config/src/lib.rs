//! gravity-config — turn-executor configuration.
//!
//! Phase 0 shape: one OpenAI-compatible endpoint + model id, plus the
//! krystallizer store path. Secrets ride environment variables
//! (ADR-0007's rule: never in the config file); the endpoint URL and
//! model are not secrets.
//!
//! Deliberately flat: no nested subsystems yet — the config grows when
//! a second consumer (provider booth, prism transport) needs a node.

/// The turn executor's configuration (Phase 0).
#[derive(Clone, Debug, serde::Deserialize)]
pub struct GravityConfig {
    /// OpenAI-compatible chat-completions endpoint
    /// (e.g. `https://api.openai.com/v1`).
    pub endpoint: String,
    /// Model id the endpoint serves (`gpt-4o-mini`, ...).
    pub model: String,
    /// The env var NAME holding the API key — the key itself is never
    /// in the config (ADR-0007 §secrets).
    pub api_key_env: String,
    /// Krystallizer connection — exactly one of:
    /// `store_path`: a local fjall directory (in-process mem-core, the
    /// same-machine detail), or
    /// `store_url`: the mem-server base URL (the single cross-process
    /// form, ADR-0007 as revised 2026-10-10).
    pub store_path: Option<String>,
    pub store_url: Option<String>,
    /// How k10r should render the memory it hands back (ADR-0010 §5):
    /// `plaintext` (the default), `markdown`, `mermaid`. `struct` is NOT
    /// accepted — it returns the fact data for a caller that assembles the
    /// graph itself, and this executor injects k10r's rendering instead; a
    /// config carrying it is refused when the turn surface is built
    /// (`MemoryFormat::from_config`, gravity-core).
    pub memory_format: Option<String>,
}

impl GravityConfig {
    /// Resolve the API key from the environment. The key crosses this
    /// boundary exactly once per call — it never lands in the config
    /// struct, so Debug-logging the config can never leak it.
    pub fn api_key(&self) -> anyhow::Result<String> {
        std::env::var(&self.api_key_env)
            .map_err(|_| anyhow::anyhow!("env var {} not set (the API key)", self.api_key_env))
    }
}
