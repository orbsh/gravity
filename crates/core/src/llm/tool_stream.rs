//! Assembling a streamed tool call (ADR-0012's envelope, provider side).
//!
//! OpenAI-shaped providers do not send a tool call, they send its FRAGMENTS: the
//! first delta carries the id and the function's name with an empty argument
//! string, and every later delta carries the next slice of that string under the
//! same `index`. A consumer that dispatches on the first envelope therefore
//! dispatches an empty argument document — which is what a real endpoint did to
//! this stack the first time it ran outside a test: every stub streams one
//! complete call, so nothing in the test suite could have caught it.
//!
//! The accumulator is deliberately separate from the SSE decoder: the decoder
//! answers "what did the next frame say", this answers "what has the stream said so
//! far". Only the second one can concatenate.
use serde_json::{json, Value};
use std::collections::BTreeMap;

/// Tool-call fragments gathered between two non-call events.
#[derive(Debug, Default)]
pub struct ToolCallAccumulator {
    /// `index` → the call being built. A map, not a vector: fragments of two
    /// calls interleave, and the index is the only thing that says which is which.
    calls: BTreeMap<u64, Partial>,
}

#[derive(Debug, Default)]
struct Partial {
    id: String,
    kind: String,
    name: String,
    arguments: String,
}

impl ToolCallAccumulator {
    pub fn new() -> Self {
        Self::default()
    }

    /// Merge one delta's `tool_calls` array. Idempotent parts (id, type, name) are
    /// taken once — a provider that repeats the name in every fragment must not
    /// end up with the name twice — and `arguments` is CONCATENATED, because that
    /// is the only part the model splits.
    pub fn push(&mut self, fragments: &[Value]) {
        for fragment in fragments {
            let index = fragment
                .get("index")
                .and_then(Value::as_u64)
                .unwrap_or_default();
            let entry = self.calls.entry(index).or_default();
            if let Some(id) = fragment.get("id").and_then(Value::as_str) {
                if !id.is_empty() && entry.id.is_empty() {
                    entry.id = id.to_string();
                }
            }
            if let Some(kind) = fragment.get("type").and_then(Value::as_str) {
                if !kind.is_empty() && entry.kind.is_empty() {
                    entry.kind = kind.to_string();
                }
            }
            if let Some(function) = fragment.get("function") {
                if let Some(name) = function.get("name").and_then(Value::as_str) {
                    if !name.is_empty() && entry.name.is_empty() {
                        entry.name = name.to_string();
                    }
                }
                if let Some(piece) = function.get("arguments").and_then(Value::as_str) {
                    entry.arguments.push_str(piece);
                }
            }
        }
    }

    /// Is anything being built?
    pub fn is_empty(&self) -> bool {
        self.calls.is_empty()
    }

    /// The assembled array, in `index` order, and the accumulator empties.
    ///
    /// A fragment stream that never named its function is still returned: dropping
    /// it silently would turn "the provider said something we do not understand"
    /// into "the model asked for nothing", and those are different failures.
    pub fn take(&mut self) -> Option<Value> {
        if self.calls.is_empty() {
            return None;
        }
        let calls: Vec<Value> = std::mem::take(&mut self.calls)
            .into_values()
            .map(|call| {
                json!({
                    "id": call.id,
                    "type": if call.kind.is_empty() { "function".to_string() } else { call.kind },
                    "function": {
                        "name": call.name,
                        "arguments": if call.arguments.trim().is_empty() {
                            "{}".to_string()
                        } else {
                            call.arguments
                        },
                    },
                })
            })
            .collect();
        Some(Value::Array(calls))
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::ToolCallAccumulator;
    use serde_json::json;

    #[test]
    fn a_complete_call_in_one_delta_survives_untouched() {
        // The shape every stub uses — this is the case the tests already covered,
        // kept here so the fragment path cannot regress it.
        let mut acc = ToolCallAccumulator::new();
        acc.push(
            json!([{
                "index": 0,
                "id": "call_1",
                "type": "function",
                "function": { "name": "open_task", "arguments": "{\"name\":\"x\"}" }
            }])
            .as_array()
            .unwrap(),
        );
        let calls = acc.take().expect("one call");
        assert_eq!(calls[0]["id"], "call_1");
        assert_eq!(calls[0]["function"]["name"], "open_task");
        assert_eq!(calls[0]["function"]["arguments"], "{\"name\":\"x\"}");
    }

    #[test]
    fn fragments_are_concatenated_into_one_call() {
        let mut acc = ToolCallAccumulator::new();
        for fragment in [
            json!([{ "index": 0, "id": "call_7", "type": "function",
                     "function": { "name": "open_task", "arguments": "" } }]),
            json!([{ "index": 0, "function": { "arguments": "{\"na" } }]),
            json!([{ "index": 0, "function": { "arguments": "me\":\"veri" } }]),
            json!([{ "index": 0, "function": { "arguments": "fy-the-stack\"}" } }]),
        ] {
            acc.push(fragment.as_array().unwrap());
        }
        let calls = acc.take().expect("one call");
        assert_eq!(calls.as_array().unwrap().len(), 1);
        assert_eq!(calls[0]["id"], "call_7", "the id came from the first fragment");
        assert_eq!(calls[0]["function"]["name"], "open_task");
        assert_eq!(
            calls[0]["function"]["arguments"],
            "{\"name\":\"verify-the-stack\"}"
        );
    }

    #[test]
    fn two_interleaved_calls_stay_two() {
        let mut acc = ToolCallAccumulator::new();
        for fragment in [
            json!([{ "index": 0, "id": "a", "function": { "name": "one", "arguments": "{" } },
                   { "index": 1, "id": "b", "function": { "name": "two", "arguments": "{" } }]),
            json!([{ "index": 1, "function": { "arguments": "\"k\":2}" } },
                   { "index": 0, "function": { "arguments": "\"k\":1}" } }]),
        ] {
            acc.push(fragment.as_array().unwrap());
        }
        let calls = acc.take().expect("two calls");
        assert_eq!(calls[0]["id"], "a");
        assert_eq!(calls[0]["function"]["arguments"], "{\"k\":1}");
        assert_eq!(calls[1]["id"], "b");
        assert_eq!(calls[1]["function"]["arguments"], "{\"k\":2}");
    }

    #[test]
    fn an_empty_argument_string_becomes_an_empty_document() {
        let mut acc = ToolCallAccumulator::new();
        acc.push(
            json!([{ "index": 0, "id": "c", "function": { "name": "list_tasks" } }])
                .as_array()
                .unwrap(),
        );
        let calls = acc.take().expect("one call");
        assert_eq!(calls[0]["function"]["arguments"], "{}");
    }

    #[test]
    fn nothing_pushed_is_nothing_to_take() {
        let mut acc = ToolCallAccumulator::new();
        assert!(acc.is_empty());
        assert_eq!(acc.take(), None);
    }
}