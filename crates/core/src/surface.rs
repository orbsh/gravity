//! The memory surface seam (ADR-0007 as revised 2026-10-10): the turn
//! function speaks ONE surface; how it reaches krystallizer — in-process
//! mem-core or the mem-server HTTP API — is a deployment choice behind
//! this trait, invisible to the turn loop. In-process linking is a
//! local same-machine implementation detail, NOT a second access mode;
//! HTTP is the only cross-process form.

use anyhow::Context;
/// The projection type rides the seam too (prompt assembly reads it).
pub use mem_core::channel::SessionView;
/// mem-core's single-member default, re-exported so drivers need no
/// mem-core dependency for one constant.
pub use mem_core::channel::DEFAULT_MEMBER;
use mem_core::channel::{ChannelStore, MessageKey};

/// One trait object per surface kind; the turn function takes
/// `&dyn MemorySurface`. Methods mirror `ChannelStore`'s primitives
/// exactly — the surface adds no policy. `async_trait` boxes the
/// futures (object safety); the turn loop's latency is dominated by
/// the LLM call, the boxing is noise at this seam.
#[async_trait::async_trait]
pub trait MemorySurface: Send + Sync {
    /// The projection read (checkpoint + increment).
    async fn fetch_session(&self, channel_id: u64, member_id: u64) -> anyhow::Result<SessionView>;

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

/// In-process surface: wraps a tokio-mutex'd `ChannelStore` — the same
/// single-writer discipline the mem-server shell uses (mem-core's
/// append/checkpoint take `&mut self`; the channel log is single-writer
/// by design).
pub struct InProcessSurface {
    store: tokio::sync::Mutex<mem_core::channel::ChannelStore>,
}

impl InProcessSurface {
    /// Open (or create) the channel-log keyspace at `path`.
    pub fn open(path: &std::path::Path) -> Result<Self, fjall::Error> {
        Ok(Self::wrap(ChannelStore::open(path)?))
    }

    /// Wrap an already-opened store — the booth binding (Milestone B)
    /// takes over a store its host opened; Fjall locks per directory,
    /// so a second `open` of the same path is an error, not a handle.
    pub fn wrap(store: ChannelStore) -> Self {
        Self {
            store: tokio::sync::Mutex::new(store),
        }
    }
}

#[async_trait::async_trait]
impl MemorySurface for InProcessSurface {
    async fn fetch_session(&self, channel_id: u64, member_id: u64) -> anyhow::Result<SessionView> {
        Ok(self.store.lock().await.fetch_full(channel_id, member_id))
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
        let mut store = self.store.lock().await;
        Ok(store.append(channel_id, ts_ms, sub, role, kind, content))
    }
}

/// HTTP surface: the mem-server wire contract (the caller owns the
/// clock — ts_ms/sub ride the request; the server never assigns).
pub struct HttpSurface {
    base: String, // e.g. http://127.0.0.1:8100 — no trailing slash
    http: reqwest::Client,
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

/// The response of the append call (mem-server's `MessageKeyDto`).
/// The key is reconstructible locally (the caller sent ts/sub), so the
/// body is not parsed — the status check is the contract.
impl HttpSurface {
    pub fn new(base: &str) -> Self {
        Self {
            base: base.trim_end_matches('/').to_string(),
            http: reqwest::Client::new(),
        }
    }
}

#[async_trait::async_trait]
impl MemorySurface for HttpSurface {
    async fn fetch_session(&self, channel_id: u64, member_id: u64) -> anyhow::Result<SessionView> {
        // Wire shape mirrors mem-server's SessionDto; keys are not
        // needed by the prompt assembly, so the response is decoded
        // loosely (increment rows only).
        #[derive(serde::Deserialize)]
        struct KeyDto {
            channel_id: u64,
            ts_ms: u64,
            sub: u16,
        }
        #[derive(serde::Deserialize)]
        struct RowDto {
            key: KeyDto,
            row: RowInner,
        }
        #[derive(serde::Deserialize)]
        struct RowInner {
            role: u8,
            kind: u8,
            content: String,
        }
        #[derive(serde::Deserialize)]
        struct SessionDto {
            coverage_ts: u64,
            summary: Option<String>,
            increment: Vec<RowDto>,
        }
        let url = format!(
            "{base}/channels/{channel_id}/members/{member_id}/session",
            base = self.base
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
        Ok(SessionView {
            coverage_ts: dto.coverage_ts,
            summary: dto.summary,
            increment: dto
                .increment
                .into_iter()
                // Keys ride the wire (mem-server echoes them in every
                // entry) — delta records (Phase 2.5) need them, so no
                // key-dropping here.
                .map(|r| {
                    (
                        MessageKey {
                            channel_id: r.key.channel_id,
                            ts_ms: r.key.ts_ms,
                            sub: r.key.sub,
                        },
                        mem_core::channel::MessageRow {
                            role: r.row.role,
                            kind: r.row.kind,
                            content: r.row.content,
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
        let status = self
            .http
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
            .context("mem-server append rejected (HTTP status)")?
            .status();
        let _ = status;
        Ok(MessageKey {
            channel_id,
            ts_ms,
            sub,
        })
    }
}
