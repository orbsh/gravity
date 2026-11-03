//! A live chat-completion stream, held by the process that asked for it.
//!
//! There is no provider booth: the turn opens its own streams. A stream is a
//! blocking reader over the endpoint's SSE body, decoded by [`super::sse`] and
//! reassembled by [`super::tool_stream`], and it lives on the thread that pulls
//! it. Two turns in flight, or a turn's answer and its compression branch, are
//! two of these on two threads — no shared slot, no instance to key, nothing to
//! evict. induction's request/reply ids are what let one turn process serve
//! several of them at once.
//!
//! The envelope shape is the one the turn already spoke over the pull face, so a
//! caller's loop is unchanged: `{"done": false, "item": {"delta": "…"}}` per token,
//! `{"done": false, "item": {"tool_calls": […]}}` once a call's fragments stop
//! arriving (the call is COMPLETE — a real endpoint splits the argument document,
//! and dispatching the first piece dispatches an empty call), and `{"done": true}`
//! at the end.

use anyhow::Context;
use serde_json::{json, Value};
use std::io::Read;

use super::sse::{SseDecoder, SseEvent};
use super::tool_stream::ToolCallAccumulator;

/// Connection retry: constant backoff, a bounded attempt count. Transient
/// endpoint failures (connection refused, 502/503) retry; a 4xx (bad key, bad
/// model) is NOT transient and fails through immediately — retrying a 401 is
/// just delay.
const CONNECT_ATTEMPTS: u32 = 3;
const CONNECT_BACKOFF_MS: u64 = 500;

/// The request one stream is opened with. The key is an env var NAME: the value
/// is read here, per call, and never crosses the caller's wire (rotation without
/// a reload).
#[derive(Clone, Debug)]
pub struct Request<'a> {
    pub endpoint: &'a str,
    pub model: &'a str,
    pub key_env: &'a str,
    pub messages: &'a Value,
    /// The tool list, verbatim (the caller owns it — ADR-0004; this is transport
    /// for whatever the caller's request shape is).
    pub tools: Option<&'a Value>,
}

/// One open stream: the decoder, its reader, and the call fragments gathered
/// since the last non-call event.
pub struct Live {
    sse: SseDecoder,
    reader: Box<dyn Read + Send>,
    calls: ToolCallAccumulator,
    /// At most one envelope waiting for its turn: a stream that ends a call and
    /// sends content in the same round must not lose either.
    queued: Option<Value>,
}

impl Live {
    /// Open the stream. Blocks only for the connection and the response headers.
    pub fn open(request: &Request<'_>) -> anyhow::Result<Self> {
        let endpoint = request.endpoint.trim_end_matches('/');
        let key = std::env::var(request.key_env).context("llm: key env var unset")?;
        let mut body = json!({
            "model": request.model,
            "messages": request.messages,
            "stream": true,
        });
        if let Some(tools) = request.tools {
            body["tools"] = tools.clone();
        }
        let url = format!("{endpoint}/chat/completions");
        let mut last_err = String::new();
        for attempt in 0..CONNECT_ATTEMPTS {
            if attempt > 0 {
                std::thread::sleep(std::time::Duration::from_millis(CONNECT_BACKOFF_MS));
            }
            let resp = ureq::post(&url)
                .set("Authorization", &format!("Bearer {key}"))
                .set("Content-Type", "application/json")
                .send_string(&body.to_string());
            match resp {
                Ok(resp) => {
                    return Ok(Self {
                        sse: SseDecoder::new(),
                        reader: resp.into_reader(),
                        calls: ToolCallAccumulator::new(),
                        queued: None,
                    });
                }
                Err(ureq::Error::Status(code, _)) if (400..500).contains(&code) => {
                    anyhow::bail!("llm: endpoint rejected the request ({code})");
                }
                Err(e) => {
                    last_err = e.to_string();
                    eprintln!("[gravity] connect attempt {attempt}: {last_err}");
                }
            }
        }
        anyhow::bail!("llm: endpoint unreachable after {CONNECT_ATTEMPTS} attempts: {last_err}")
    }

    /// The next envelope, refilling as needed. Blocking: the pull IS the
    /// backpressure, so a slow upstream stalls its own thread and nobody else's.
    /// Named `pull`, not `next`: this is a stream's round, not an iterator's step.
    pub fn pull(&mut self) -> anyhow::Result<Value> {
        if let Some(queued) = self.queued.take() {
            return Ok(queued);
        }
        loop {
            if let Some(event) = self.sse.decode()? {
                match event {
                    // Fragments accumulate; the envelope is emitted only once the
                    // stream stops talking about calls (a delta, or the end).
                    SseEvent::ToolCalls(fragments) => {
                        if let Some(fragments) = fragments.as_array() {
                            self.calls.push(fragments);
                        }
                        continue;
                    }
                    SseEvent::Delta(delta) => {
                        let envelope = json!({ "done": false, "item": { "delta": delta } });
                        if let Some(calls) = self.calls.take() {
                            self.queued = Some(envelope);
                            return Ok(json!({ "done": false, "item": { "tool_calls": calls } }));
                        }
                        return Ok(envelope);
                    }
                    SseEvent::Done => {
                        // The calls are complete even when the provider sends no
                        // trailing content — and they must arrive BEFORE the done,
                        // or the turn ends without ever dispatching them.
                        if let Some(calls) = self.calls.take() {
                            self.queued = Some(json!({ "done": true }));
                            return Ok(json!({ "done": false, "item": { "tool_calls": calls } }));
                        }
                        return Ok(json!({ "done": true }));
                    }
                }
            }
            let mut chunk = [0u8; 8192];
            let n = self
                .reader
                .read(&mut chunk)
                .context("llm: stream read failed")?;
            if n == 0 {
                anyhow::bail!("llm: upstream closed before [DONE]");
            }
            self.sse.feed(&chunk[..n]);
        }
    }
}