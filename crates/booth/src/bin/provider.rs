//! The LLM provider guest — an induction process: SSE from an
//! OpenAI-compatible endpoint parsed into an iterate producer. One
//! envelope round per pull (`{"delta": "..."}`), `done: true` closes.
//!
//! Args (per Start): `{"endpoint", "model", "key_env", "messages":
//! [{"role", "content"}, ...]}`. The key is read from the environment
//! PER CALL — the value never crosses the module or the args.
//!
//! Stream state lives in THIS process: the SSE connection is opened on
//! Start (round 0) and consumed one decoded event per pull round. The
//! guard's `total` is unknown for a live network stream, so the Start
//! reply carries none (the guard defaults to u64::MAX and exhaustion
//! rides the producer's own `done: true`).
//!
//! The SSE framing and chunk shape come from gravity-core's `llm::sse`
//! (the one home for the wire decoding — the CLI's LLM path and this
//! guest share it).

use anyhow::Context;
use gravity_core::llm::sse::{SseDecoder, SseEvent};
use induction_guest::{Ctx, Guest};
use serde_json::{json, Value};
use std::io::Read;
use std::sync::Mutex;

fn main() -> anyhow::Result<()> {
    Guest::new()
        .named("gravity-provider")
        // The Start handler: `stream` is the name the consumer's
        // iterate_start names. Round 0 opens the SSE connection and
        // replies the first envelope (`{done:false, item:{delta}}`).
        .on("stream", |_ctx: &mut Ctx, args: Value| {
            let conn = open_stream(&args)?;
            let mut slot = STREAM.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
            let mut stream = conn;
            let reply = next_envelope(&mut stream)?;
            // Done already dropped the stream (next_envelope signals it
            // by leaving None here); otherwise the connection persists.
            if reply.get("done").and_then(Value::as_bool) != Some(true) {
                *slot = Some(stream);
            }
            Ok(reply)
        })
        // The pull handler: `__next__` is the reserved binding the
        // guest guard table routes every pull round to.
        .producer(|_ctx: &mut Ctx, _args: Value| {
            let mut slot = STREAM.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
            let Some(mut stream) = slot.take() else {
                anyhow::bail!("provider: pull with no live stream");
            };
            let reply = next_envelope(&mut stream)?;
            if reply.get("done").and_then(Value::as_bool) != Some(true) {
                *slot = Some(stream);
            }
            Ok(reply)
        })
        .run()
}

/// The connection this guest process holds across pull rounds — the
/// process IS the stream state. One stream per guest instance (the
/// realm keys instances by session; a second turn cold-starts a new
/// child), so a single slot suffices.
static STREAM: Mutex<Option<Stream>> = Mutex::new(None);

struct Stream {
    /// The SSE decoder (buffer + framing), gravity-core's shared wire
    /// module.
    sse: SseDecoder,
    reader: Box<dyn std::io::Read + Send>,
}

impl Stream {
    /// Pull more bytes into the decoder (blocking read — the pull IS
    /// the backpressure; a slow upstream stalls its own pull).
    fn fill(&mut self) -> anyhow::Result<()> {
        let mut chunk = [0u8; 8192];
        let n = self
            .reader
            .read(&mut chunk)
            .context("provider: stream read failed")?;
        if n == 0 {
            anyhow::bail!("provider: upstream closed before [DONE]");
        }
        self.sse.feed(&chunk[..n]);
        Ok(())
    }
}

/// Decode the next envelope out of the stream, refilling as needed.
/// The caller owns the stream (taken from the slot); a non-terminal
/// reply means the caller puts it back (the connection persists).
fn next_envelope(stream: &mut Stream) -> anyhow::Result<Value> {
    loop {
        if let Some(ev) = stream.sse.decode()? {
            return Ok(match ev {
                SseEvent::Delta(delta) => json!({ "done": false, "item": { "delta": delta } }),
                // Tool calls ride the envelope verbatim — the TURN
                // parses and dispatches them (the provider is transport,
                // not policy).
                SseEvent::ToolCalls(calls) => {
                    json!({ "done": false, "item": { "tool_calls": calls } })
                }
                SseEvent::Done => json!({ "done": true }),
            });
        }
        stream.fill()?;
    }
}

/// Connection retry: constant backoff, a bounded attempt count. The
/// transport-level resilience PLAN Phase 1 assigns to the provider —
/// transient endpoint failures (connection refused, 502/503) retry;
/// a 4xx (bad key, bad model) is NOT transient and fails through
/// immediately (retrying a 401 is just delay). `done` is reported by
/// the host's pull — the guest never sees it as an error to retry.
const CONNECT_ATTEMPTS: u32 = 3;
const CONNECT_BACKOFF_MS: u64 = 500;

fn open_stream(args: &Value) -> anyhow::Result<Stream> {
    let endpoint = args
        .get("endpoint")
        .and_then(Value::as_str)
        .context("provider: missing `endpoint`")?
        .trim_end_matches('/')
        .to_string();
    let model = args
        .get("model")
        .and_then(Value::as_str)
        .context("provider: missing `model`")?;
    let key_env = args
        .get("key_env")
        .and_then(Value::as_str)
        .context("provider: missing `key_env`")?;
    let key = std::env::var(key_env).context("provider: key env var unset")?;
    let messages = args
        .get("messages")
        .cloned()
        .context("provider: missing `messages`")?;

    let url = format!("{endpoint}/chat/completions");
    // The affordances ride through untouched (the turn owns the list — ADR-0004;
    // this booth is a transport for whatever the caller's request shape is).
    let mut body = json!({ "model": model, "messages": messages, "stream": true });
    if let Some(tools) = args.get("tools") {
        body["tools"] = tools.clone();
    }
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
                return Ok(Stream {
                    sse: SseDecoder::new(),
                    reader: resp.into_reader(),
                });
            }
            Err(ureq::Error::Status(code, _)) if (400..500).contains(&code) => {
                // Client error: not transient — the pull fails through
                // now (the consumer decides what a 401/404 means).
                anyhow::bail!("provider: endpoint rejected the request ({code})");
            }
            Err(e) => {
                last_err = e.to_string();
                eprintln!("[gravity-provider] connect attempt {attempt}: {last_err}");
            }
        }
    }
    anyhow::bail!("provider: endpoint unreachable after {CONNECT_ATTEMPTS} attempts: {last_err}")
}
