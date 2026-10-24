//! The tool-call loop's contract (PLAN Phase 2): parse the LLM's tool
//! calls → dispatch each through the injectable executor → feed the
//! results back as `role: "tool"` messages for the next inference
//! round. Failure is a value into the next round (a tool failure is
//! not a turn failure); the delta boundary is the loop's own — the
//! transcript lives in the loop, never a full-session write-back.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unwrap_in_result, clippy::panic_in_result_fn)]

use gravity_core::tool::{parse_tool_calls, ToolDispatch, ToolResultMessage};
use serde_json::{json, Value};

/// The injectable executor: the loop drives THIS, not a concrete
/// transport — the aura `ctx.invoke` face and the test stub are both
/// just impls.
trait ToolExecutor {
    fn dispatch(&mut self, call: &ToolDispatch) -> anyhow::Result<String>;
}

/// The loop, one round: given the raw completion chunk, produce the
/// dispatches and the fed-back result messages.
fn run_round(
    chunk: &Value,
    executor: &mut dyn ToolExecutor,
) -> anyhow::Result<Vec<ToolResultMessage>> {
    let calls = parse_tool_calls(chunk)?;
    let mut results = Vec::new();
    for call in calls {
        // Target resolution rides the tool list (Phase 4.5): the loop
        // takes it verbatim — here the tool list is the stub's.
        let dispatch = ToolDispatch {
            tool_call_id: call.id.clone(),
            name: call.function.name.clone(),
            target: format!("effector:local:{}", call.function.name),
            args: call.function.arguments.clone(),
        };
        let content = match executor.dispatch(&dispatch) {
            Ok(out) => out,
            // Failure is a value into the next inference (the tool
            // failed, not the turn — the loop continues).
            Err(e) => format!("tool error: {e:#}"),
        };
        results.push(ToolResultMessage {
            tool_call_id: call.id.clone(),
            content,
        });
    }
    Ok(results)
}

struct StubExecutor {
    outcomes: Vec<Result<String, String>>,
}

impl ToolExecutor for StubExecutor {
    fn dispatch(&mut self, call: &ToolDispatch) -> anyhow::Result<String> {
        assert_eq!(call.target, format!("effector:local:{}", call.name));
        self.outcomes.remove(0).map_err(|e| anyhow::anyhow!(e))
    }
}

#[test]
fn tool_calls_parse_from_the_wire_shape() {
    // OpenAI ships arguments as a JSON STRING — decoded once at the
    // boundary.
    let chunk = json!({
        "tool_calls": [
            {"id": "c1", "function": {"name": "read_file", "arguments": "{\"path\": \"/tmp/x\"}"}},
            {"id": "c2", "function": {"name": "list_dir", "arguments": "{}"}},
        ]
    });
    let calls = parse_tool_calls(&chunk).unwrap();
    assert_eq!(calls.len(), 2);
    assert_eq!(calls[0].function.name, "read_file");
    assert_eq!(calls[0].function.arguments["path"], "/tmp/x");
    assert_eq!(calls[1].function.arguments, json!({}));
}

#[test]
fn no_tool_calls_is_an_empty_round() {
    // A plain reply chunk (no tool_calls key) — the loop ends.
    let chunk = json!({"content": "just an answer"});
    let calls = parse_tool_calls(&chunk).unwrap();
    assert!(calls.is_empty());
}

#[test]
fn malformed_arguments_fail_the_round_not_the_process() {
    let chunk = json!({
        "tool_calls": [
            {"id": "c1", "function": {"name": "x", "arguments": "not json"}},
        ]
    });
    assert!(parse_tool_calls(&chunk).is_err());
}

#[test]
fn results_feed_back_with_call_ids() {
    let chunk = json!({
        "tool_calls": [
            {"id": "c1", "function": {"name": "read_file", "arguments": "{}"}},
            {"id": "c2", "function": {"name": "boom", "arguments": "{}"}},
        ]
    });
    let mut executor = StubExecutor {
        outcomes: vec![Ok("file contents".into()), Err("permission denied".into())],
    };
    let results = run_round(&chunk, &mut executor).unwrap();
    assert_eq!(results.len(), 2);
    assert_eq!(results[0].tool_call_id, "c1");
    assert_eq!(results[0].content, "file contents");
    // The failed tool's result carries the error TEXT (a value into the
    // next inference — the loop and the turn survive it).
    assert_eq!(results[1].tool_call_id, "c2");
    assert!(results[1].content.contains("permission denied"));
}
