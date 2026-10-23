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
pub use mem_core::channel::DEFAULT_MEMBER;

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

/// One increment entry as the projection carries it: the key the message
/// lives under, the LLM role NAME (k10r's mapping), the kind byte and the
/// content. `role` is `None` for a stored code k10r does not police — a
/// writer's bug, which the turn loop surfaces by falling back to `user`.
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
    pub checkpoint: u64,
    pub prompt: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct SessionProjection {
    /// Initialization memory, rendered by k10r in the requested format;
    /// empty when the user node has no facts yet.
    pub head: String,
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
        member_id: u64,
    ) -> anyhow::Result<SessionProjection>;

    /// Append one log entry; returns the full key (the delta record).
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
    pub fn open(path: &std::path::Path) -> Result<Self, fjall::Error> {
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
        member_id: u64,
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
        );
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
            channels.open_checkpoint(channel_id, (v.checkpoint_ts, v.checkpoint_sub), &head_ids);
        }
        Ok(SessionProjection {
            head: mem_core::render_head(&projection.head, self.format.render()),
            coverage_ts: projection.view.coverage_ts,
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
                checkpoint: v.checkpoint_ts,
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
        Ok(store.append(channel_id, ts_ms, sub, role, kind, content))
    }
}

/// HTTP surface: the mem-server wire contract (the caller owns the
/// clock — ts_ms/sub ride the request; the server never assigns).
pub struct HttpSurface {
    base: String, // e.g. http://127.0.0.1:8100 — no trailing slash
    http: reqwest::Client,
    format: MemoryFormat,
}

/// Request body of `POST /channels/{id}/messages` (mem-server's wire
/// shape — kept in step with mem-server's `AppendMessage`).
#[derive(serde::Serialize)]
struct AppendMessageDto {
    ts_ms: u64,
    sub: u16,
    role: u8,
    kind: u8,
    content: String,
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
        member_id: u64,
    ) -> anyhow::Result<SessionProjection> {
        #[derive(serde::Deserialize)]
        struct KeyDto {
            channel_id: u64,
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
            prompt: String,
        }
        #[derive(serde::Deserialize)]
        struct SessionDto {
            /// Rendered text under every text format. A `struct` answer would
            /// put an ARRAY here and fail this decode — which is why the
            /// format is validated where it is configured.
            #[serde(default)]
            head: String,
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
            coverage_ts: dto.coverage_ts,
            summary: dto.summary,
            compress: dto.compress.map(|c| Compression {
                checkpoint: c.checkpoint,
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
        let url = format!("{base}/channels/{channel_id}/messages", base = self.base);
        self.http
            .post(&url)
            .json(&AppendMessageDto {
                ts_ms,
                sub,
                role,
                kind,
                content: content.to_string(),
            })
            .send()
            .await
            .context("mem-server append request failed (transport)")?
            .error_for_status()
            .context("mem-server append rejected (HTTP status)")?;
        Ok(MessageKey {
            channel_id,
            ts_ms,
            sub,
        })
    }
}
