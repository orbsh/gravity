//! The tool-call loop's wire types (PLAN Phase 2): what the LLM emits
//! (a tool call in its completion chunk), what the turn loop feeds
//! back (the tool result as a message). The OpenAI wire shapes — the
//! same interface currency as `WireMessage`; krystallizer's storage
//! never sees these forms.

use serde::{Deserialize, Serialize};

/// A tool call the LLM emitted (`choices[0].delta.tool_calls[0]` in a
/// streamed chunk, `choices[0].message.tool_calls[0]` in a blocking
/// reply). `id` correlates the result message back for the next
/// inference round.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct ToolCall {
    pub id: String,
    /// `{"name": "...", "arguments": {...}}` — arguments as a JSON
    /// object (OpenAI ships them as a string; the loop parses once,
    /// at the boundary).
    pub function: ToolFunction,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct ToolFunction {
    pub name: String,
    /// Parsed arguments (the wire's JSON string decoded once here).
    pub arguments: Value,
}

/// A tool result fed back into the next inference round (`role: "tool"`
/// with `tool_call_id` — the feed-back half of the loop).
#[derive(Clone, Debug, Serialize)]
pub struct ToolResultMessage {
    pub tool_call_id: String,
    /// The result content: a string on success, the error text on
    /// failure (failure is a value into the NEXT inference — a tool
    /// failure is not a turn failure, ADR-0012's stance at the loop).
    pub content: String,
}

use serde_json::Value;

/// A tool dispatch the loop drives: the LLM-selected operation plus
/// where it executes. Target shape per the unified call model (Phase
/// 4.5's `user:node:capability` — Phase 2's first cut takes the target
/// verbatim from the tool list the caller supplies; gravity never
/// resolves locations itself).
#[derive(Clone, Debug)]
pub struct ToolDispatch {
    pub tool_call_id: String,
    pub name: String,
    pub target: String,
    pub args: Value,
}

/// Parse the tool calls out of a completion chunk's `tool_calls` array
/// (the wire ships arguments as a JSON string — decoded once here;
/// malformed arguments surface as a loop error value, never a panic).
pub fn parse_tool_calls(chunk: &Value) -> anyhow::Result<Vec<ToolCall>> {
    let Some(calls) = chunk.get("tool_calls").and_then(Value::as_array) else {
        return Ok(Vec::new());
    };
    calls
        .iter()
        .map(|c| {
            let id = c
                .get("id")
                .and_then(Value::as_str)
                .context("tool call missing `id`")?
                .to_string();
            let function = c.get("function").context("tool call missing `function`")?;
            let name = function
                .get("name")
                .and_then(Value::as_str)
                .context("tool call missing `function.name`")?
                .to_string();
            let raw_args = function
                .get("arguments")
                .and_then(Value::as_str)
                .unwrap_or("{}");
            let arguments: Value =
                serde_json::from_str(raw_args).context("tool call arguments were not JSON")?;
            Ok(ToolCall {
                id,
                function: ToolFunction { name, arguments },
            })
        })
        .collect()
}

use anyhow::Context;
