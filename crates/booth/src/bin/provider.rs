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

use anyhow::Context;
use induction_guest::{Ctx, Guest};
use serde_json::{json, Value};
use std::sync::Mutex;

fn main() -> anyhow::Result<()> {
    Guest::new()
        .named("gravity-provider")
        // The Start handler: `stream` is the name the consumer's
        // iterate_start names. Round 0 opens the SSE connection and
        // replies the first envelope (`{done:false, item:{delta}}`).
        .on("stream", |_ctx: &mut Ctx, args: Value| {
            let conn = open_stream(&args)?;
            *STREAM.lock().unwrap() = Some(conn);
            let mut slot = STREAM.lock().unwrap();
            let stream = slot.as_mut().unwrap();
            loop {
                if let Some(ev) = next_event(stream)? {
                    return Ok(match ev {
                        Event::Delta(delta) => {
                            json!({ "done": false, "item": { "delta": delta } })
                        }
                        Event::Done => {
                            *slot = None;
                            json!({ "done": true })
                        }
                    });
                }
                stream.fill()?;
            }
        })
        // The pull handler: `__next__` is the reserved binding the
        // guest guard table routes every pull round to.
        .producer(|_ctx: &mut Ctx, args: Value| stream_round(args))
        .run()
}

/// The connection this guest process holds across pull rounds — the
/// process IS the stream state. One stream per guest instance (the
/// realm keys instances by session; a second turn cold-starts a new
/// child), so a single slot suffices.
static STREAM: Mutex<Option<Stream>> = Mutex::new(None);

struct Stream {
    /// Buffered, undecoded SSE text left over from the last read.
    buf: String,
    reader: Box<dyn std::io::Read + Send>,
}

/// One pull round: yield one decoded event per round until the
/// terminal `[DONE]` sentinel (round 0 never arrives here — the Start
/// handler opened the stream and consumed the first event).
fn stream_round(_args: Value) -> anyhow::Result<Value> {
    let mut slot = STREAM.lock().unwrap();
    let stream = slot
        .as_mut()
        .ok_or_else(|| anyhow::anyhow!("provider: pull with no live stream"))?;
    loop {
        if let Some(ev) = next_event(stream)? {
            return Ok(match ev {
                Event::Delta(delta) => json!({ "done": false, "item": { "delta": delta } }),
                Event::Done => {
                    *slot = None; // the connection closes with the stream
                    json!({ "done": true })
                }
            });
        }
        // Buffer exhausted: refill from the socket (blocking read — the
        // pull IS the backpressure; a slow upstream stalls its own pull).
        stream.fill()?;
    }
}

enum Event {
    Delta(String),
    Done,
}

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
    let body = json!({ "model": model, "messages": messages, "stream": true });
    let resp = ureq::post(&url)
        .set("Authorization", &format!("Bearer {key}"))
        .send_string(&body.to_string())
        .context("provider: LLM endpoint request failed")?;
    Ok(Stream {
        buf: String::new(),
        reader: resp.into_reader(),
    })
}

/// Pull more bytes into the buffer.
impl Stream {
    fn fill(&mut self) -> anyhow::Result<()> {
        let mut chunk = [0u8; 8192];
        let n = self
            .reader
            .read(&mut chunk)
            .context("provider: stream read failed")?;
        if n == 0 {
            anyhow::bail!("provider: upstream closed before [DONE]");
        }
        self.buf.push_str(&String::from_utf8_lossy(&chunk[..n]));
        Ok(())
    }
}

use std::io::Read;

/// Decode the next complete SSE event out of the buffer, if one is
/// present. SSE frames are `data: <json>\n\n`; the terminal sentinel is
/// `data: [DONE]`.
fn next_event(stream: &mut Stream) -> anyhow::Result<Option<Event>> {
    while let Some(pos) = stream.buf.find("\n\n") {
        let frame = stream.buf.drain(..pos + 2).collect::<String>();
        for line in frame.lines() {
            let Some(data) = line.strip_prefix("data: ") else {
                continue;
            };
            if data.trim() == "[DONE]" {
                return Ok(Some(Event::Done));
            }
            let chunk: Value = serde_json::from_str(data)
                .with_context(|| format!("provider: bad SSE payload: {data}"))?;
            if let Some(delta) = chunk
                .get("choices")
                .and_then(|c| c.get(0))
                .and_then(|c| c.get("delta"))
                .and_then(|d| d.get("content"))
                .and_then(Value::as_str)
            {
                if !delta.is_empty() {
                    return Ok(Some(Event::Delta(delta.to_string())));
                }
            }
        }
    }
    Ok(None)
}
