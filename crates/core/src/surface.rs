//! The memory surface seam (ADR-0007 as revised 2026-10-10): the turn
//! function speaks ONE surface; how it reaches krystallizer — in-process
//! mem-core or the mem-server HTTP API — is a deployment choice behind
//! this trait, invisible to the turn loop. In-process linking is a
//! local same-machine implementation detail, NOT a second access mode;
//! HTTP is the only cross-process form.
//!
//! **The seam's currency is the PROJECTION k10r hands out, not storage
//! rows.** k10r renders (ADR-0010 §5): the head arrives as text in the
//! format the deployment asked for, and an increment entry arrives with the
//! LLM role NAME already resolved — the mapping from the stored code is
//! k10r's side of the wire (ADR-0008 §2). Gravity renders nothing about the
//! graph: it never assembles a head fact list, a slice or a community.

use anyhow::Context;
use mem_core::channel::MessageKey;
/// mem-core's single-member default, re-exported so drivers need no
/// mem-core dependency for one constant.
pub use mem_core::channel::{DEFAULT_MEMBER_NAME, default_member};

/// How k10r represents a memory answer (ADR-0010 §5).
///
/// `struct` is deliberately NOT a variant: it hands back the fact DATA for a
/// consumer that assembles the graph itself, and the turn executor does no
/// such assembly — the renderer is k10r's. A deployment that configures
/// `struct` is a configuration error, surfaced at once ([`Self::from_config`])
/// rather than discovered as a decode failure on the first turn.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum MemoryFormat {
    /// The line form — the default, and what a prompt wants.
    #[default]
    Plaintext,
    /// Headings and bullets.
    Markdown,
    /// A `graph LR` diagram; readable by a human, not a prompt.
    Mermaid,
}

impl MemoryFormat {
    /// Parse a configured value. Absent means the default; `struct` is
    /// refused BY NAME with the reason, because the executor cannot use it.
    pub fn from_config(raw: Option<&str>) -> anyhow::Result<Self> {
        match raw {
            None => Ok(Self::default()),
            Some("plaintext") => Ok(Self::Plaintext),
            Some("markdown") => Ok(Self::Markdown),
            Some("mermaid") => Ok(Self::Mermaid),
            Some("struct") => anyhow::bail!(
                "memory format `struct` is not usable by the turn executor: it returns \
                 the fact data for a caller that assembles the graph itself, and gravity \
                 injects k10r's rendering"
            ),
            Some(other) => anyhow::bail!(
                "unknown memory format `{other}` — expected plaintext / markdown / mermaid"
            ),
        }
    }

    /// The wire value, as k10r spells it.
    pub fn name(self) -> &'static str {
        match self {
            Self::Plaintext => "plaintext",
            Self::Markdown => "markdown",
            Self::Mermaid => "mermaid",
        }
    }

    /// mem-core's own format token — the in-process surface renders through
    /// the same functions the HTTP answer came from.
    fn render(self) -> mem_core::RenderFormat {
        match self {
            Self::Plaintext => mem_core::RenderFormat::Plaintext,
            Self::Markdown => mem_core::RenderFormat::Markdown,
            Self::Mermaid => mem_core::RenderFormat::Mermaid,
        }
    }
}

/// Who a message row is from, as k10r declares it (`mem_core::channel::role`).
///
/// The STORE keeps a plain `u8` — a protocol code, not a vocabulary — and this
/// enum is the writer's shape at the edge: no magic numbers in the turn, and a
/// reordered variant cannot renumber the wire because every discriminant is
/// spelled out. The numeric table itself stays k10r's (`role::USER` … are the
/// discriminants below); this type adds the shape, not a second table.
///
/// Names are NOT here on purpose: `user`/`assistant`/… is k10r's `role_name`,
/// the one place the code becomes a label, and every reader — the projection a
/// UI consumes and the prompt a model consumes — takes it from there. A second
/// label table in this crate would be a second source of truth.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[repr(u8)]
pub enum Role {
    User = mem_core::channel::role::USER,
    Assistant = mem_core::channel::role::ASSISTANT,
    Tool = mem_core::channel::role::TOOL,
    System = mem_core::channel::role::SYSTEM,
}

impl Role {
    /// The byte this row is stored under.
    pub const fn code(self) -> u8 {
        self as u8
    }
}

/// Reading a stored code back. Total: an unknown code is the caller's bug and
/// comes back as `Err`, never as a default — a fallback here would silently
/// relabel a row nobody can name.
impl TryFrom<u8> for Role {
    type Error = u8;

    fn try_from(code: u8) -> Result<Self, u8> {
        Ok(match code {
            mem_core::channel::role::USER => Role::User,
            mem_core::channel::role::ASSISTANT => Role::Assistant,
            mem_core::channel::role::TOOL => Role::Tool,
            mem_core::channel::role::SYSTEM => Role::System,
            other => return Err(other),
        })
    }
}

/// What a row IS, as k10r declares it (`mem_core::channel::kind`). Same
/// reasoning as [`Role`]: a protocol code the store keeps as a byte, and a
/// writer's shape at the edge.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[repr(u8)]
pub enum Kind {
    Text = mem_core::channel::kind::TEXT,
    ToolCall = mem_core::channel::kind::TOOL_CALL,
    ToolResult = mem_core::channel::kind::TOOL_RESULT,
}

impl Kind {
    pub const fn code(self) -> u8 {
        self as u8
    }
}

impl TryFrom<u8> for Kind {
    type Error = u8;

    fn try_from(code: u8) -> Result<Self, u8> {
        Ok(match code {
            mem_core::channel::kind::TEXT => Kind::Text,
            mem_core::channel::kind::TOOL_CALL => Kind::ToolCall,
            mem_core::channel::kind::TOOL_RESULT => Kind::ToolResult,
            other => return Err(other),
        })
    }
}

/// One increment entry as the projection carries it: the key the message
/// lives under, the LLM role NAME (k10r's mapping), the kind byte and the
/// content. `role` is `None` for a stored code k10r does not police — a
/// writer's bug, which the turn loop renders as `system` (the record happened,
/// but its speaker cannot be named, and calling it `user` would put words in
/// the user's mouth).
#[derive(Clone, Debug, PartialEq)]
pub struct ViewRow {
    pub role: Option<String>,
    pub kind: u8,
    pub content: String,
}

/// The assembled projection k10r serves (ADR-0008 §2): the head
/// (initialization memory, RENDERED), the compression point, and the log
/// increment strictly after it.
///
/// The head is text because a memory consumer need not know how the graph is
/// assembled (ADR-0010 §5); the increment stays structural because it is the
/// CONVERSATION — its role turns must stay separable on the LLM wire.
/// The compression directive k10r serves beside the projection (ADR-0008 §4).
///
/// It is a CONTROL value, not a record: the span to summarize up to, and the
/// tail prompt to append to it. Gravity carries it so the directive is not
/// dropped between the seam and the turn loop; running the branch turn is the
/// turn loop's half, and k10r derives the coverage point when the summary comes
/// back — gravity never states one.
#[derive(Clone, Debug, PartialEq)]
pub struct Compression {
    /// The record the summary should end at (k10r ADR-0024): a pointer in the
    /// log's key space, so the turn is part of the address.
    pub checkpoint: u64,
    pub checkpoint_turn: u64,
    pub prompt: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct SessionProjection {
    /// Initialization memory, rendered by k10r in the requested format;
    /// empty when the user node has no facts yet.
    pub head: String,
    /// The turn the coverage point falls in (ADR-0024).
    pub coverage_turn: u64,
    pub coverage_ts: u64,
    pub summary: Option<String>,
    pub increment: Vec<(MessageKey, ViewRow)>,
    /// The policy's verdict when one fired; `None` = build the context from
    /// the records alone.
    pub compress: Option<Compression>,
}

/// One trait object per surface kind; the turn function takes
/// `&dyn MemorySurface`. Methods mirror the mem-server doors exactly — the
/// surface adds no policy. `async_trait` boxes the futures (object safety);
/// the turn loop's latency is dominated by the LLM call, the boxing is
/// noise at this seam.
#[async_trait::async_trait]
pub trait MemorySurface: Send + Sync {
    /// The projection read (head + checkpoint + increment).
    async fn fetch_session(
        &self,
        channel_id: u64,
        // k10r's member id is the user's uuid (k10r ADR-0025): an identity, not a counter.
        member_id: mem_core::Uuid7,
    ) -> anyhow::Result<SessionProjection>;

    /// Append one log entry; returns the full key (the delta record).
    ///
    /// This surface writes a FINISHED record: it opens a turn, writes the row and
    /// closes the turn keeping it (k10r ADR-0024). The live path — a turn that
    /// runs for many rounds and is resumed after a cold call — is the booth's
    /// (`TurnLog` in `booth/src/bin/turn.rs`); this one exists for the two-line
    /// exchange and for tests, where a row IS the whole turn.
    async fn append_message(
        &self,
        channel_id: u64,
        ts_ms: u64,
        sub: u16,
        role: u8,
        kind: u8,
        content: &str,
    ) -> anyhow::Result<MessageKey>;
}

/// In-process surface: wraps a tokio-mutex'd `ChannelStore` + `GraphStore` —
/// the same single-writer discipline the mem-server shell uses. It links the
/// SAME primitives and renders through the SAME function the HTTP answer
/// came from, so the two surfaces agree on the shape.
pub struct InProcessSurface {
    channels: tokio::sync::Mutex<mem_core::ChannelStore>,
    graph: tokio::sync::Mutex<mem_core::GraphStore>,
    format: MemoryFormat,
}

impl InProcessSurface {
    /// Open (or create) the service database at `path` — ONE database, both
    /// keyspaces (`open_service`), because Fjall locks the directory.
    /// The seam's error is mem-core's `String` (the storage crate does not
    /// hand out its own error type).
    pub fn open(path: &std::path::Path) -> Result<Self, String> {
        Ok(Self::wrap(mem_core::open_service(path)?))
    }

    /// Wrap stores a host already opened — the booth binding (Milestone B)
    /// takes over stores its host opened; Fjall locks per directory, so a
    /// second `open` of the same path is an error, not a handle.
    pub fn wrap(stores: mem_core::ServiceStores) -> Self {
        Self {
            channels: tokio::sync::Mutex::new(stores.channels),
            graph: tokio::sync::Mutex::new(stores.graph),
            format: MemoryFormat::default(),
        }
    }

    /// Ask for another representation (the in-process landing point for the
    /// format knob; an HTTP deployment passes it in the URL instead).
    pub fn with_format(mut self, format: MemoryFormat) -> Self {
        self.format = format;
        self
    }
}

#[async_trait::async_trait]
impl MemorySurface for InProcessSurface {
    async fn fetch_session(
        &self,
        channel_id: u64,
        member_id: mem_core::Uuid7,
    ) -> anyhow::Result<SessionProjection> {
        // ONE lock order (channel, then graph) — the same order the mem-server
        // door takes, so an in-process deployment cannot deadlock where HTTP
        // would not.
        let mut channels = self.channels.lock().await;
        let mut graph = self.graph.lock().await;
        let projection = mem_core::view::assemble(
            &mut channels,
            &mut graph,
            channel_id,
            member_id,
            mem_core::view::DEFAULT_SCOPE,
        )?;
        // The policy evaluates HERE, as it does at the door, so the two
        // surfaces answer the same directive for the same state. No clock rides
        // this call (the caller's clock belongs to its own turn), so the cache
        // clock stays silent and only the budget can fire.
        let assembled = mem_core::render_head(&projection.head, mem_core::RenderFormat::Plaintext)
            .len()
            + projection.view.summary.as_deref().map_or(0, str::len)
            + projection
                .view
                .increment
                .iter()
                .map(|(_, row)| row.content.len())
                .sum::<usize>();
        let verdict = mem_core::compress::evaluate(
            &projection.view,
            assembled,
            None,
            &mem_core::CompressionParams::default(),
        );
        if let Some(v) = &verdict {
            // The split this verdict chose is FROZEN here, so the summary that
            // comes back lands on it (ADR-0008 §2: the checkpoint carries the
            // head, not a re-computation of it). k10r retired the separate
            // verdict record — the checkpoint IS the record, and the head ids are
            // what it carries; a decision that is not written down is not one.
            let head_ids: Vec<u64> = projection.head.iter().map(|fact| fact.edge_id).collect();
            channels
                .open_checkpoint(channel_id, v.checkpoint, &head_ids)
                .map_err(anyhow::Error::msg)?;
        }
        Ok(SessionProjection {
            head: mem_core::render_head(&projection.head, self.format.render()),
            coverage_ts: projection.view.coverage.ts_ms,
            coverage_turn: projection.view.coverage.turn_id,
            summary: projection.view.summary,
            increment: projection
                .view
                .increment
                .into_iter()
                .map(|(key, row)| {
                    (
                        key,
                        ViewRow {
                            role: mem_core::view::role_name(row.role).map(str::to_string),
                            kind: row.kind,
                            content: row.content,
                        },
                    )
                })
                .collect(),
            compress: verdict.map(|v| Compression {
                checkpoint: v.checkpoint.ts_ms,
                checkpoint_turn: v.checkpoint.turn_id,
                prompt: mem_core::TAIL_PROMPT.to_string(),
            }),
        })
    }

    async fn append_message(
        &self,
        channel_id: u64,
        ts_ms: u64,
        sub: u16,
        role: u8,
        kind: u8,
        content: &str,
    ) -> anyhow::Result<MessageKey> {
        let mut store = self.channels.lock().await;
        // One row, one turn: the append OPENS it and the store mints the id (k10r
        // ADR-0024 §2) — there is no separate open for a turn that writes one row.
        let key = store
            .append_opening(channel_id, ts_ms, sub, role, kind, content)
            .map_err(anyhow::Error::msg)?;
        let turn = key.turn_id;
        // A row written here is a finished exchange, so the turn closes at once
        // and keeps exactly that row. The closing record must sort after it.
        store
            .close_turn(
                channel_id,
                turn,
                ts_ms,
                sub + 1,
                role,
                "",
                &[(ts_ms, sub)],
            )
            .map_err(anyhow::Error::msg)?;
        Ok(key)
    }
}

/// HTTP surface: the mem-server wire contract (the caller owns the
/// clock — ts_ms/sub ride the request; the server never assigns).
pub struct HttpSurface {
    base: String, // e.g. http://127.0.0.1:8100 — no trailing slash
    http: reqwest::Client,
    format: MemoryFormat,
}

impl HttpSurface {
    pub fn new(base: &str) -> Self {
        Self {
            base: base.trim_end_matches('/').to_string(),
            http: reqwest::Client::new(),
            format: MemoryFormat::default(),
        }
    }

    /// Ask k10r for another representation of the head (ADR-0010 §5). The
    /// format rides the query string, so it shows in a log line and is
    /// reproducible with curl.
    pub fn with_format(mut self, format: MemoryFormat) -> Self {
        self.format = format;
        self
    }
}

#[async_trait::async_trait]
impl MemorySurface for HttpSurface {
    async fn fetch_session(
        &self,
        channel_id: u64,
        member_id: mem_core::Uuid7,
    ) -> anyhow::Result<SessionProjection> {
        #[derive(serde::Deserialize)]
        struct KeyDto {
            channel_id: u64,
            /// The turn the record belongs to (k10r ADR-0024).
            turn_id: u64,
            ts_ms: u64,
            sub: u16,
        }
        /// The increment entry is FLAT: the role NAME rides beside the key
        /// (k10r resolved the stored code — there is no nested storage row).
        #[derive(serde::Deserialize)]
        struct EntryDto {
            key: KeyDto,
            role: Option<String>,
            kind: u8,
            content: String,
        }
        /// The compression directive (ADR-0008 §4) — absent when no verdict
        /// fired, which is the ordinary case.
        #[derive(serde::Deserialize)]
        struct CompressDto {
            checkpoint: u64,
            /// The turn the checkpoint's pointer falls in (ADR-0024). Defaulted:
            /// an older server sent `(ts, sub)` alone.
            #[serde(default)]
            checkpoint_turn: u64,
            prompt: String,
        }
        #[derive(serde::Deserialize)]
        struct SessionDto {
            /// Rendered text under every text format. A `struct` answer would
            /// put an ARRAY here and fail this decode — which is why the
            /// format is validated where it is configured.
            #[serde(default)]
            head: String,
            /// The turn the coverage point falls in (k10r ADR-0024). Defaulted:
            /// a server that predates the turn axis sends no such field, and a
            /// pointer without a turn is the older `(0, ts, sub)` reading.
            #[serde(default)]
            coverage_turn: u64,
            coverage_ts: u64,
            summary: Option<String>,
            increment: Vec<EntryDto>,
            #[serde(default)]
            compress: Option<CompressDto>,
        }
        let format = self.format.name();
        let url = format!(
            "{base}/channels/{channel_id}/members/{member_id}/session?format={format}",
            base = self.base,
        );
        let dto: SessionDto = self
            .http
            .get(&url)
            .send()
            .await
            .and_then(|r| r.error_for_status())
            .context("mem-server session request failed")?
            .json()
            .await
            .context("mem-server session response was not the expected shape")?;
        Ok(SessionProjection {
            head: dto.head,
            coverage_turn: dto.coverage_turn,
            coverage_ts: dto.coverage_ts,
            summary: dto.summary,
            compress: dto.compress.map(|c| Compression {
                checkpoint: c.checkpoint,
                checkpoint_turn: c.checkpoint_turn,
                prompt: c.prompt,
            }),
            increment: dto
                .increment
                .into_iter()
                // Keys ride the wire (mem-server echoes them in every entry)
                // — delta records (Phase 2.5) are written from them.
                .map(|entry| {
                    (
                        MessageKey {
                            channel_id: entry.key.channel_id,
                            turn_id: entry.key.turn_id,
                            ts_ms: entry.key.ts_ms,
                            sub: entry.key.sub,
                        },
                        ViewRow {
                            role: entry.role,
                            kind: entry.kind,
                            content: entry.content,
                        },
                    )
                })
                .collect(),
        })
    }

    async fn append_message(
        &self,
        channel_id: u64,
        ts_ms: u64,
        sub: u16,
        role: u8,
        kind: u8,
        content: &str,
    ) -> anyhow::Result<MessageKey> {
        // One call = one FINISHED turn (see the trait's note): write the row that
        // OPENS it (the store mints the id there, k10r ADR-0024 §2), close keeping
        // that row. The live multi-round turn is the booth's own path.
        let url = format!("{base}/channels/{channel_id}/messages", base = self.base);
        let key = self
            .http
            .post(&url)
            .json(&serde_json::json!({
                // No `turn_id`: the omission asks the store to mint one.
                "ts_ms": ts_ms,
                "sub": sub,
                "role": role,
                "kind": kind,
                "content": content,
            }))
            .send()
            .await
            .context("mem-server append request failed (transport)")?
            .error_for_status()
            .context("mem-server append rejected (HTTP status)")?
            .json::<serde_json::Value>()
            .await
            .context("mem-server append answered the wrong shape")?;
        let turn_id = key["turn_id"]
            .as_u64()
            .context("mem-server append answered no turn_id")?;
        self.http
            .post(format!(
                "{base}/channels/{channel_id}/turns/{turn_id}/close",
                base = self.base
            ))
            .json(&serde_json::json!({
                "ts_ms": ts_ms,
                "sub": sub + 1,
                "role": role,
                "content": "",
                "keep": [{ "ts_ms": ts_ms, "sub": sub }],
            }))
            .send()
            .await
            .context("mem-server turn-close request failed (transport)")?
            .error_for_status()
            .context("mem-server turn-close rejected (HTTP status)")?;
        Ok(MessageKey {
            channel_id,
            turn_id,
            ts_ms,
            sub,
        })
    }
}

#[cfg(test)]
mod vocabulary_tests {
    use super::{Kind, Role};

    /// The codes are k10r's, not this crate's invention: a mismatch here means the
    /// store and the writer disagree about what byte 3 means.
    #[test]
    fn the_codes_are_the_stores() {
        assert_eq!(Role::User.code(), mem_core::channel::role::USER);
        assert_eq!(Role::Assistant.code(), mem_core::channel::role::ASSISTANT);
        assert_eq!(Role::Tool.code(), mem_core::channel::role::TOOL);
        assert_eq!(Role::System.code(), mem_core::channel::role::SYSTEM);
        assert_eq!(Kind::Text.code(), mem_core::channel::kind::TEXT);
        assert_eq!(Kind::ToolCall.code(), mem_core::channel::kind::TOOL_CALL);
        assert_eq!(Kind::ToolResult.code(), mem_core::channel::kind::TOOL_RESULT);
    }

    /// Every stored code either decodes or is refused — never a silent default.
    /// Counting the decodable codes catches an arm that was never written: the
    /// domain is four values wide, and a code the enum forgot would come back as
    /// an error for a row the store legitimately holds.
    #[test]
    fn the_domains_are_exactly_as_wide_as_declared() {
        let roles: Vec<u8> = (0..=255u8).filter(|c| Role::try_from(*c).is_ok()).collect();
        assert_eq!(roles, vec![0, 1, 2, 3], "four roles, contiguous from 0");
        let kinds: Vec<u8> = (0..=255u8).filter(|c| Kind::try_from(*c).is_ok()).collect();
        assert_eq!(kinds, vec![0, 1, 2], "three kinds, contiguous from 0");
        assert_eq!(Role::try_from(4u8), Err(4));
        assert_eq!(Kind::try_from(9u8), Err(9));
    }
}
