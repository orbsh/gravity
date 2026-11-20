//! gravity-config — the turn's configuration.
//!
//! Phase 0 shape: one OpenAI-compatible endpoint + model id, plus the
//! krystallizer store path. Secrets ride environment variables
//! (ADR-0007's rule: never in the config file); the endpoint URL and
//! model are not secrets.
//!
//! Deliberately flat: no nested subsystems yet — the config grows when
//! a second consumer (provider booth, prism transport) needs a node.

/// The turn booth's configuration (Phase 0).
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
    /// graph itself, and this booth injects k10r's rendering instead; a
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
}

/// One mount as `GET /admin/mounts` publishes it (prism ADR-0021). This is an ANSWER, not config:
/// a client keeps no copy of the table, because two copies of one venue's mounts drift (gravity
/// ADR-0001's open item, closed by reading).
#[derive(Clone, Debug, serde::Deserialize)]
pub struct MountEntry {
    /// The operation language the mount runs (`python`, `nushell`, …).
    pub language: String,
    /// The mount's type name in the venue.
    #[serde(rename = "type")]
    pub type_name: String,
    /// The venue minted the type (always true for a listed row that registered).
    #[serde(default)]
    pub registered: bool,
    /// The node behind the mount is in the venue's pool right now.
    #[serde(default)]
    pub node_connected: bool,
}

/// THE VENUE'S MOUNT TABLE, read from the venue rather than kept anywhere (prism ADR-0021,
/// gravity ADR-0001 §7). One function for both readers — the CLI before it submits a turn, and the
/// turn booth for the turns a PAGE submits, which carry no table at all — because two readers with
/// two copies is the drift this shape exists to end.
///
/// A `ws://` url is the same listener as `http://`: this reader has no TLS client, so a `wss://`
/// venue must be reached through a local one, and that is said instead of failing obscurely.
pub fn admin_base(venue_url: &str) -> anyhow::Result<String> {
    if let Some(rest) = venue_url.strip_prefix("ws://") {
        return Ok(format!("http://{rest}"));
    }
    // A venue that PUBLISHES this to its guests publishes the http origin (prism's
    // `PRISM_VENUE_URL`): the same listener, already spelled the way `/admin` is read.
    if venue_url.starts_with("http://") {
        return Ok(venue_url.trim_end_matches('/').to_string());
    }
    anyhow::bail!(
        "venue url `{venue_url}` is not plain `ws://`: the mount table is read over HTTP and this \
         reader has no TLS client"
    )
}

/// The mount table the venue publishes: `registered` rows only (a row that refused to register is
/// not callable, whatever it says), and for one language the CONNECTED node first — a job that
/// addresses the absent node spends a dispatch learning what the venue already said.
///
/// Returns the rows and the venue's own refusal lines: a caller that swallows those turns "the
/// venue has no mounts" into a mystery.
pub fn fetch_mounts(venue_url: &str) -> anyhow::Result<(Vec<MountEntry>, Vec<String>)> {
    let url = format!("{}/admin/mounts", admin_base(venue_url)?);
    let answer: serde_json::Value = ureq::get(&url)
        .call()
        .map_err(|e| anyhow::anyhow!("reading {url}: {e}"))?
        .into_json()
        .map_err(|e| anyhow::anyhow!("{url} is not JSON: {e}"))?;
    let mut entries: Vec<MountEntry> = answer
        .get("mounts")
        .cloned()
        .map(|rows| serde_json::from_value(rows).unwrap_or_default())
        .unwrap_or_default();
    let refused: Vec<String> = answer
        .get("refused")
        .and_then(serde_json::Value::as_array)
        .map(|lines| {
            lines
                .iter()
                .filter_map(|line| line.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default();
    entries.retain(|m| m.registered);
    entries.sort_by(|a, b| {
        (&a.language, !a.node_connected, &a.type_name)
            .cmp(&(&b.language, !b.node_connected, &b.type_name))
    });
    Ok((entries, refused))
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
