//! The SSE wire decoder — the ONE home for OpenAI-compatible
//! chunked-response parsing. Both readers of the LLM wire ride this:
//! the provider guest (induction producer, one envelope per pull) and
//! any future streaming consumer. The framing (`data: <json>\n\n`
//! lines, the `[DONE]` sentinel) and the chunk shape
//! (`choices[0].delta.content`) live here; the transport (blocking
//! read vs buffered text) stays with the caller.

use anyhow::Context;
use serde_json::Value;

/// One decoded SSE event.
#[derive(Debug)]
pub enum SseEvent {
    /// A token delta (`choices[0].delta.content`).
    Delta(String),
    /// A tool call the LLM emitted (`choices[0].delta.tool_calls` —
    /// Phase 2's loop face; the arguments string rides verbatim, the
    /// caller's tool module parses it once at the boundary).
    ToolCalls(Value),
    /// The `[DONE]` sentinel — the stream's terminal.
    Done,
}

/// A buffered SSE byte stream: raw reads accumulate in a text buffer,
/// decoded events pop out one per [`SseDecoder::next`].
pub struct SseDecoder {
    /// Buffered, undecoded SSE text left over from the last read.
    buf: String,
}

impl Default for SseDecoder {
    fn default() -> Self {
        Self::new()
    }
}

impl SseDecoder {
    pub fn new() -> Self {
        Self { buf: String::new() }
    }

    /// Feed raw bytes from the transport (a read round).
    pub fn feed(&mut self, bytes: &[u8]) {
        self.buf.push_str(&String::from_utf8_lossy(bytes));
    }

    /// Decode the next complete event out of the buffer, if one is
    /// present. `Ok(None)` = buffer exhausted, feed more. (Deliberately
    /// NOT the `Iterator::next` trait — a feed/decode protocol, not an
    /// iterator: refills come from the caller's transport.)
    pub fn decode(&mut self) -> anyhow::Result<Option<SseEvent>> {
        while let Some(pos) = self.buf.find("\n\n") {
            let frame: String = self.buf.drain(..pos + 2).collect();
            for line in frame.lines() {
                let Some(data) = line.strip_prefix("data: ") else {
                    continue;
                };
                if data.trim() == "[DONE]" {
                    return Ok(Some(SseEvent::Done));
                }
                let chunk: Value = serde_json::from_str(data)
                    .with_context(|| format!("llm: bad SSE payload: {data}"))?;
                // Tool calls ride the chunk before content (a chunk
                // carries either; some providers emit both in one —
                // content first, calls second).
                if let Some(calls) = chunk
                    .get("choices")
                    .and_then(|c| c.get(0))
                    .and_then(|c| c.get("delta"))
                    .and_then(|d| d.get("tool_calls"))
                    .and_then(Value::as_array)
                    .filter(|a| !a.is_empty())
                {
                    return Ok(Some(SseEvent::ToolCalls(Value::Array(calls.clone()))));
                }
                if let Some(delta) = chunk
                    .get("choices")
                    .and_then(|c| c.get(0))
                    .and_then(|c| c.get("delta"))
                    .and_then(|d| d.get("content"))
                    .and_then(Value::as_str)
                {
                    if !delta.is_empty() {
                        return Ok(Some(SseEvent::Delta(delta.to_string())));
                    }
                }
            }
        }
        Ok(None)
    }
}
