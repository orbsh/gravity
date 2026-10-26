//! The fixed tool list (ADR-0004): hardcoded, grouped, and carrying the names
//! k10r's tool face already has (`docs/SEAM.md` §2).
//!
//! Two entries today, because two doors answer: `recall_memory` and
//! `commit_memory`. `resolve_skills` has no door (k10r withdrew the bring-up
//! pool read with zero clients; the skill node is Phase 5+) and `invoke_skill`
//! needs the skill identity `resolve_skills` produces — the ADR fixed their
//! NAMES so nothing is renamed later, and the list grows by door, never by
//! enthusiasm. Listing a tool the substrate cannot serve would teach the model
//! that failing tools are normal.
//!
//! The list is static on purpose: the `tools` array is part of the request's
//! stable prefix, so the same bytes ride every turn.

use serde_json::{json, Value};

/// How a tool's execution behaves (ADR-0004 §6) — the field Phase 2.5's cold
/// path reads and the one Phase 5's `tool_invoke_count` hangs on.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Nature {
    /// In-process, idempotent: repeated for free, no outside effects.
    Fast,
    /// Touches the outside world or changes state.
    Human,
}

pub struct ToolSpec {
    pub name: &'static str,
    pub description: &'static str,
    /// The JSON-schema builder for the arguments. A function rather than a
    /// literal because `serde_json::Value` cannot be built in a `static`.
    pub parameters: fn() -> Value,
    pub nature: Nature,
}

/// The list. Order is the order the model sees.
static SPECS: &[ToolSpec] = &[
    ToolSpec {
        name: "recall_memory",
        description: "Recall what is known over the long term. k10r lands the query in the \
                      fact graph and answers with the facts around it, rendered as text. Use it \
                      when the answer depends on something you were not told in this conversation; \
                      the turn-start projection already carries the composed head.",
        parameters: || {
            json!({
                "type": "object",
                "properties": {
                    "query": {
                        "type": "string",
                        "description": "What to recall, in natural language."
                    }
                },
                "required": ["query"]
            })
        },
        nature: Nature::Fast,
    },
    ToolSpec {
        name: "commit_memory",
        description: "Write facts into the long-term graph. Each fact is an 8-slot tuple: \
                      [head value, head type, head props, relation name, relation props, \
                      tail value, tail type, tail props]. Props are JSON objects (`{}` when \
                      empty). A functional relation at the same source is adjudicated by k10r, \
                      not overwritten blindly — the answer says which of landed / rewritten / \
                      dropped happened.",
        parameters: || {
            json!({
                "type": "object",
                "properties": {
                    "facts": {
                        "type": "array",
                        "description": "The 8-slot fact tuples to ingest.",
                        "items": {
                            "type": "array",
                            "minItems": 8,
                            "maxItems": 8
                        }
                    }
                },
                "required": ["facts"]
            })
        },
        nature: Nature::Human,
    },
];

/// The entries, in the order the model sees them.
pub fn specs() -> &'static [ToolSpec] {
    SPECS
}

/// The names this deployment serves — the ones an unknown call is answered
/// with.
pub fn names() -> Vec<&'static str> {
    SPECS.iter().map(|spec| spec.name).collect()
}

/// The `tools` array for the provider request (OpenAI's function shape).
pub fn tools_array() -> Value {
    Value::Array(
        SPECS
            .iter()
            .map(|spec| {
                json!({
                    "type": "function",
                    "function": {
                        "name": spec.name,
                        "description": spec.description,
                        "parameters": (spec.parameters)(),
                    }
                })
            })
            .collect(),
    )
}

/// Execute one call against its door.
///
/// Every failure is a VALUE the model reads (`{"error": …}`), never a turn
/// failure: a wrong argument or a missing deployment fact must not cost the
/// user their answer. `memory_scope` is the deployment fact (ADR-0004 §5) —
/// absent, the memory tools say so by name instead of guessing a namespace.
pub fn call(
    name: &str,
    args: &Value,
    store_url: &str,
    memory_scope: Option<u64>,
) -> Result<Value, String> {
    let store_url = store_url.trim_end_matches('/');
    match name {
        "recall_memory" => {
            let query = args
                .get("query")
                .and_then(Value::as_str)
                .ok_or("recall_memory: `query` is required")?;
            let scope = scope(memory_scope, "recall_memory")?;
            post(
                &format!("{store_url}/memories/{scope}/recall"),
                &json!([query]),
            )
        }
        "commit_memory" => {
            let facts = args
                .get("facts")
                .ok_or("commit_memory: `facts` is required")?;
            if !facts.is_array() {
                return Err("commit_memory: `facts` must be an array of 8-slot tuples".into());
            }
            let scope = scope(memory_scope, "commit_memory")?;
            post(&format!("{store_url}/memories/{scope}"), facts)
        }
        other => Err(format!(
            "unknown tool `{other}`; this deployment serves: {}",
            names().join(", ")
        )),
    }
}

fn scope(memory_scope: Option<u64>, tool: &str) -> Result<u64, String> {
    memory_scope.ok_or_else(|| {
        format!("{tool}: `memory_scope` is not set on this deployment (the turn args carry it)")
    })
}

/// One POST, with the door's answer as the tool result and its refusal as the
/// error text (a 4xx is the caller's mistake to read, not a turn failure).
fn post(url: &str, body: &Value) -> Result<Value, String> {
    match ureq::post(url).send_json(body.clone()) {
        Ok(resp) => resp
            .into_json::<Value>()
            .map_err(|e| format!("{url}: unreadable answer: {e}")),
        Err(ureq::Error::Status(code, resp)) => {
            let text = resp.into_string().unwrap_or_default();
            Err(format!("{url}: HTTP {code}: {text}"))
        }
        Err(e) => Err(format!("{url}: {e}")),
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    #[test]
    fn the_list_carries_the_tool_faces_names() {
        assert_eq!(names(), vec!["recall_memory", "commit_memory"]);
    }

    #[test]
    fn every_entry_is_an_openai_function_shape() {
        let array = tools_array();
        let entries = array.as_array().expect("an array");
        assert_eq!(entries.len(), specs().len());
        for (entry, spec) in entries.iter().zip(specs()) {
            assert_eq!(entry["type"], "function");
            assert_eq!(entry["function"]["name"], spec.name);
            assert!(
                entry["function"]["description"].as_str().unwrap_or("").len() > 40,
                "a tool without a usable description is a tool the model cannot choose"
            );
            assert_eq!(entry["function"]["parameters"]["type"], "object");
            assert!(entry["function"]["parameters"]["required"].is_array());
        }
    }

    #[test]
    fn an_unlisted_tool_is_refused_by_name() {
        let Err(error) = call("invoke_skill", &json!({}), "http://127.0.0.1:1", Some(37)) else {
            panic!("an unlisted tool must be refused");
        };
        assert!(error.contains("unknown tool `invoke_skill`"), "{error}");
        assert!(error.contains("recall_memory"), "{error}");
    }

    #[test]
    fn a_missing_scope_names_the_fact_instead_of_guessing() {
        let Err(error) = call(
            "recall_memory",
            &json!({ "query": "x" }),
            "http://127.0.0.1:1",
            None,
        ) else {
            panic!("a tool with no scope must be refused");
        };
        assert!(error.contains("memory_scope"), "{error}");
    }
}