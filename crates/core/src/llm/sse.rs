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
    /// The last `usage` object any chunk carried. Kept on the DECODER rather than emitted as
    /// an event: a chunk may carry content AND usage (providers differ), and an event would
    /// have to be queued behind the delta that was already ready — a queue that loses the
    /// number the moment a caller stops pulling. The caller asks once, after `[DONE]`.
    usage: Option<Value>,
}

impl Default for SseDecoder {
    fn default() -> Self {
        Self::new()
    }
}

impl SseDecoder {
    pub fn new() -> Self {
        Self {
            buf: String::new(),
            usage: None,
        }
    }

    /// What the stream said the request cost, once it is over. `None` = the endpoint was not
    /// asked for usage, or does not report it (an older server, a non-OpenAI shape).
    pub fn take_usage(&mut self) -> Option<Value> {
        self.usage.take()
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
                // `usage` is read on every chunk, before any early return below: the frame that
                // carries it may also carry content, and a `return` here would drop the number.
                if let Some(usage) = chunk.get("usage").filter(|u| u.is_object()) {
                    self.usage = Some(usage.clone());
                }
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

#[cfg(test)]
mod tests {

    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    use super::*;

    /// Drain a whole stream, returning what came out in order and the usage left behind.
    fn drain(frames: &str) -> (Vec<String>, Option<Value>) {
        let mut decoder = SseDecoder::new();
        decoder.feed(frames.as_bytes());
        let mut seen = Vec::new();
        while let Some(event) = decoder.decode().expect("decode") {
            match event {
                SseEvent::Delta(text) => seen.push(format!("delta:{text}")),
                SseEvent::ToolCalls(_) => seen.push("calls".into()),
                SseEvent::Done => {
                    seen.push("done".into());
                    break;
                }
            }
        }
        (seen, decoder.take_usage())
    }

    fn prompt_tokens(usage: &Option<Value>) -> Option<u64> {
        usage
            .as_ref()?
            .get("prompt_tokens")
            .and_then(Value::as_u64)
    }

    #[test]
    fn the_usage_chunk_is_kept_and_is_not_an_event() {
        // The shape an OpenAI-compatible server sends when the request asked for it: empty
        // `choices`, the cost beside them. It must not become a delta, and it must not be dropped.
        let (seen, usage) = drain(concat!(
            "data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"hi\"}}]}\n\n",
            "data: {\"choices\":[],\"usage\":{\"prompt_tokens\":1234,\"completion_tokens\":7}}\n\n",
            "data: [DONE]\n\n",
        ));
        assert_eq!(seen, vec!["delta:hi".to_string(), "done".to_string()]);
        assert_eq!(prompt_tokens(&usage), Some(1234));
        // …and taking it clears it: a second reader gets nothing rather than a stale number.
        assert!(usage.is_some());
    }

    #[test]
    fn a_chunk_carrying_content_and_usage_loses_neither() {
        // Some servers put the tally on the same chunk as the last token. Reading usage only on
        // `[DONE]`, or emitting an event for it, would drop it: the early return for the delta
        // happens first.
        let (seen, usage) = drain(concat!(
            "data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"hi\"}}],\"usage\":{\"prompt_tokens\":99}}\n\n",
            "data: [DONE]\n\n",
        ));
        assert_eq!(seen, vec!["delta:hi".to_string(), "done".to_string()]);
        assert_eq!(prompt_tokens(&usage), Some(99));
    }

    #[test]
    fn a_stream_that_reports_nothing_says_so() {
        let (seen, usage) = drain("data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"x\"}}]}\n\n");
        assert_eq!(seen, vec!["delta:x".to_string()]);
        assert!(usage.is_none(), "no usage chunk, no number — not a zero");
    }
}
