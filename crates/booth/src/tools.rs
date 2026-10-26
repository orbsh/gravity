//! The fixed tool list (ADR-0004): hardcoded, grouped, and carrying the names
//! k10r's tool face already has (`docs/SEAM.md` §2).
//!
//! Three entries today, because three doors answer: `recall_memory`,
//! `commit_memory` and `resolve_skills` (k10r ADR-0015 landed the skill node and
//! its resolve door, which is what this name was waiting for). `invoke_skill`
//! stays off until the walk over a skill exists (gravity Phase 4.6: resolve a
//! step's `code_id` to a code row, then call the language's mount) — the ADR
//! fixed the NAME so nothing is renamed later, and the list grows by door, never
//! by enthusiasm. Listing a tool the substrate cannot serve would teach the model
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
    ToolSpec {
        name: "resolve_skills",
        description: "List the skills this deployment can run. Use it when the answer needs a \
                      capability the conversation did not hand you: k10r answers candidates by \
                      intent — each with a name, what it does and a score. A candidate is not a \
                      tool by itself; the call that walks one comes from `invoke_skill`, which \
                      takes the id you pick here.",
        parameters: || {
            json!({
                "type": "object",
                "properties": {
                    "query": {
                        "type": "string",
                        "description": "What you are trying to do, in natural language."
                    },
                    "limit": {
                        "type": "integer",
                        "description": "How many candidates at most (default 5, at most 20)."
                    }
                },
                "required": ["query"]
            })
        },
        nature: Nature::Fast,
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
        "resolve_skills" => {
            let query = args
                .get("query")
                .and_then(Value::as_str)
                .ok_or("resolve_skills: `query` is required")?;
            // A skill is a node in the same graph as the facts (k10r ADR-0015),
            // so the scope that addresses the graph addresses its skills too.
            let scope = scope(memory_scope, "resolve_skills")?;
            let mut body = json!({ "query": query });
            if let Some(limit) = args.get("limit") {
                body["limit"] = limit.clone();
            }
            post(&format!("{store_url}/skills/{scope}/resolve"), &body)
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
        assert_eq!(
            names(),
            vec!["recall_memory", "commit_memory", "resolve_skills"]
        );
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

    /// A one-shot HTTP server on an OS-picked port: it records the request line
    /// and body, then answers `answer`. Enough to prove WHERE a tool call goes
    /// and WHAT it carries — the door's own behaviour is k10r's test.
    fn one_shot(answer: &'static str) -> (String, std::sync::mpsc::Receiver<String>) {
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("a free port");
        let url = format!("http://{}", listener.local_addr().expect("an address"));
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let (mut socket, _) = listener.accept().expect("one connection");
            // Read until the body is there: one `read()` may stop at the end of
            // the headers (TCP segmentation), and a test that assumes otherwise
            // asserts against an empty body.
            let mut request = Vec::new();
            let mut chunk = [0u8; 4096];
            loop {
                let read = socket.read(&mut chunk).expect("a request");
                if read == 0 {
                    break;
                }
                request.extend_from_slice(&chunk[..read]);
                let complete = String::from_utf8_lossy(&request)
                    .split_once("\r\n\r\n")
                    .map(|(head, body)| {
                        let length = head
                            .lines()
                            .find_map(|line| {
                                let (name, value) = line.split_once(':')?;
                                name.eq_ignore_ascii_case("content-length")
                                    .then(|| value.trim().parse::<usize>().ok())?
                            })
                            .unwrap_or(0);
                        body.len() >= length
                    })
                    .unwrap_or(false);
                if complete {
                    break;
                }
            }
            tx.send(String::from_utf8_lossy(&request).to_string())
                .expect("the test is listening");
            let response = format!(
                "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\n\r\n{}",
                answer.len(),
                answer
            );
            socket.write_all(response.as_bytes()).expect("a reply");
        });
        (url, rx)
    }

    #[test]
    fn resolve_skills_posts_to_the_skill_door_with_the_scope_in_the_path() {
        let (url, seen) = one_shot(r#"[{"node_id":4,"name":"github-issues","title":"issues","content":"lists them","score":0.5}]"#);
        let answer = call(
            "resolve_skills",
            &json!({ "query": "list github issues", "limit": 3 }),
            &url,
            Some(37),
        )
        .expect("the door answered");
        assert_eq!(answer[0]["name"], "github-issues");

        let request = seen.recv().expect("the server saw the call");
        assert!(
            request.starts_with("POST /skills/37/resolve "),
            "a skill is a node in the graph, so the graph's scope addresses it: {request}"
        );
        let body = request.split("\r\n\r\n").nth(1).unwrap_or_default();
        assert!(body.contains("\"query\":\"list github issues\""), "{body}");
        assert!(body.contains("\"limit\":3"), "the limit rides the call: {body}");
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