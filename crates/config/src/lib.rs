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
    /// (e.g. `https://api.openai.com/v1`). Optional since the CLI became a
    /// VENUE CLIENT (ADR-0002): a CLI deployment names `venue` and holds no
    /// endpoint of its own — the turn runs in the booth, whose facts ride
    /// the turn args.
    pub endpoint: Option<String>,
    /// Model id the endpoint serves (`gpt-4o-mini`, ...).
    pub model: Option<String>,
    /// The env var NAME holding the API key — the key itself is never
    /// in the config (ADR-0007 §secrets).
    pub api_key_env: Option<String>,
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
    /// The venue this CLI submits turns to (ADR-0002). Its presence is what
    /// makes the file a CLIENT config; the local for-loop is retired.
    pub venue: Option<VenueConfig>,
}

/// The venue's connection facts. The password is NOT here — the field names
/// the env var that holds it (the same custody rule as the API key).
#[derive(Clone, Debug, serde::Deserialize)]
pub struct VenueConfig {
    /// The gateway's WS address, e.g. `ws://127.0.0.1:7780`.
    pub url: String,
    /// The account the CLI binds as (signup on first use, login after).
    pub username: String,
    /// The env var NAME holding the password.
    pub password_env: String,
    /// The venue's mount table (ADR-0001 §7): which mount type serves which operation language.
    /// Optional — a client that runs no code omits it, and `invoke_skill` then refuses by name
    /// ("no mount serves `python` here") instead of guessing.
    #[serde(default)]
    pub mounts: Option<Vec<MountEntry>>,
}

/// One entry of the venue's mount table, spelled as the turn carries it.
#[derive(Clone, Debug, serde::Deserialize)]
pub struct MountEntry {
    /// The operation language the mount runs (`python`, `nushell`, …).
    pub language: String,
    /// The mount's type name in the venue.
    #[serde(rename = "type")]
    pub type_name: String,
}

impl GravityConfig {
    /// Resolve the API key from the environment. The key crosses this
    /// boundary exactly once per call — it never lands in the config
    /// struct, so Debug-logging the config can never leak it.
    pub fn api_key(&self) -> anyhow::Result<String> {
        let name = self
            .api_key_env
            .as_deref()
            .ok_or_else(|| anyhow::anyhow!("config has no `api_key_env` (not an LLM-hosting config)"))?;
        std::env::var(name).map_err(|_| anyhow::anyhow!("env var {name} not set (the API key)"))
    }

    /// The endpoint, when this config hosts an LLM locally.
    pub fn endpoint(&self) -> anyhow::Result<&str> {
        self.endpoint
            .as_deref()
            .ok_or_else(|| anyhow::anyhow!("config has no `endpoint` (not an LLM-hosting config)"))
    }

    /// The model id, same rule.
    pub fn model(&self) -> anyhow::Result<&str> {
        self.model
            .as_deref()
            .ok_or_else(|| anyhow::anyhow!("config has no `model` (not an LLM-hosting config)"))
    }
}
