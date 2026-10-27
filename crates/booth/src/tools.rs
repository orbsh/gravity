//! The fixed tool list (ADR-0004): hardcoded, grouped, and carrying the names
//! k10r's tool face already has (`docs/SEAM.md` §2).
//!
//! Four entries today, because four doors answer: `recall_memory`,
//! `commit_memory` and `resolve_skills` (k10r's HTTP surface — data access) and
//! `invoke_skill` (a node mount — code execution, ADR-0001). The list is fixed
//! because gravity is the top of the stack and a pure glue: its memory and
//! skills ARE k10r's and its execution surface IS the node's, so there is
//! nothing per-deployment to discover here. Names are k10r's; the ADR fixed
//! them so nothing gets renamed later, and the list grows by door, never by
//! enthusiasm. Listing a tool the substrate cannot serve would teach the model
//! that failing tools are normal.
//!
//! The list is static on purpose: the `tools` array is part of the request's
//! stable prefix, so the same bytes ride every turn.
//!
//! Two kinds of door, one dispatch: [`Doors`] is the seam. The turn owns the
//! implementation (HTTP for k10r, `ctx.invoke` for a mount); this module owns
//! the list, the argument reading and the error vocabulary, which is what the
//! tests pin.

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
                      intent — each with a name, what it does, its steps and a score. A \
                      candidate is not a tool by itself; the call that runs one is \
                      `invoke_skill`, which takes the id you pick here.",
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
    ToolSpec {
        name: "invoke_skill",
        description: "Run ONE step of a skill you got from `resolve_skills`. It reads the skill, \
                      takes the step you name (0 when you name none), fetches that step's script \
                      and runs it on this deployment's node in the script's own language, \
                      returning what the script returned. A skill with several steps is a \
                      sequence YOU drive: call again with the next `step`, passing along whatever \
                      it needs in `arguments`. Nothing else is kept between calls except what the \
                      script itself remembers, so pass what it needs.",
        parameters: || {
            json!({
                "type": "object",
                "properties": {
                    "skill_id": {
                        "type": "integer",
                        "description": "The `node_id` that `resolve_skills` answered."
                    },
                    "step": {
                        "type": "integer",
                        "description": "Which step to run (0-based; default 0)."
                    },
                    "arguments": {
                        "type": "object",
                        "description": "What the step's script receives as its arguments."
                    }
                },
                "required": ["skill_id"]
            })
        },
        // It runs code: outside the process, on a node, with whatever effects
        // that code has (ADR-0004 §6).
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

/// The two kinds of door a call reaches. k10r's HTTP surface is data access
/// (ADR-0007 as revised: HTTP is the single cross-process form); a mount is
/// code execution on a node (ADR-0001) and only the turn can reach it, because
/// only the turn holds the ctx.
pub trait Doors {
    fn post(&mut self, url: &str, body: &Value) -> Result<Value, String>;
    fn get(&mut self, url: &str) -> Result<Value, String>;
    /// `ctx.invoke(type_name, key, "execute", args)` — `execute` is the entry
    /// CONVENTION every runner serves (ADR-0001 §1), never a lookup.
    fn mount(&mut self, type_name: &str, key: &str, args: Value) -> Result<Value, String>;
}

/// The deployment facts a call reads. All of them come from the turn args and
/// none has a default: a namespace, a mount table and a residency key are the
/// deployment's declarations, and a guess would silently point at someone
/// else's graph or instance (ADR-0004 §5).
pub struct Facts<'a> {
    pub store_url: &'a str,
    /// k10r's namespace for this deployment's graph (facts AND skills).
    pub memory_scope: Option<u64>,
    /// OPERATION language → the mount type that serves it (prism's manifest).
    pub mounts: &'a [(String, String)],
    /// The residency key a mount call runs under (ADR-0001 §6: the caller
    /// chooses it; gravity uses the conversation, so a script's state survives
    /// the turns of one conversation and nothing else).
    pub session: &'a str,
}

impl Facts<'_> {
    fn scope(&self, tool: &str) -> Result<u64, String> {
        self.memory_scope.ok_or_else(|| {
            format!("{tool}: `memory_scope` is not set on this deployment (the turn args carry it)")
        })
    }

    fn store(&self) -> &str {
        self.store_url.trim_end_matches('/')
    }

    fn mount_for(&self, language: &str) -> Option<&str> {
        self.mounts
            .iter()
            .find(|(served, _)| served == language)
            .map(|(_, type_name)| type_name.as_str())
    }
}

/// Execute one call against its door.
///
/// Every failure is a VALUE the model reads (`{"error": …}`), never a turn
/// failure: a wrong argument or a missing deployment fact must not cost the
/// user their answer.
pub fn call(doors: &mut impl Doors, name: &str, args: &Value, facts: &Facts<'_>) -> Result<Value, String> {
    let store_url = facts.store();
    match name {
        "recall_memory" => {
            let query = args
                .get("query")
                .and_then(Value::as_str)
                .ok_or("recall_memory: `query` is required")?;
            let scope = facts.scope("recall_memory")?;
            doors.post(
                &format!("{store_url}/memories/{scope}/recall"),
                &json!([query]),
            )
        }
        "commit_memory" => {
            let facts_arg = args
                .get("facts")
                .ok_or("commit_memory: `facts` is required")?;
            if !facts_arg.is_array() {
                return Err("commit_memory: `facts` must be an array of 8-slot tuples".into());
            }
            let scope = facts.scope("commit_memory")?;
            doors.post(&format!("{store_url}/memories/{scope}"), facts_arg)
        }
        "resolve_skills" => {
            let query = args
                .get("query")
                .and_then(Value::as_str)
                .ok_or("resolve_skills: `query` is required")?;
            // A skill is a node in the same graph as the facts (k10r ADR-0015),
            // so the scope that addresses the graph addresses its skills too.
            let scope = facts.scope("resolve_skills")?;
            let mut body = json!({ "query": query });
            if let Some(limit) = args.get("limit") {
                body["limit"] = limit.clone();
            }
            doors.post(&format!("{store_url}/skills/{scope}/resolve"), &body)
        }
        "invoke_skill" => invoke(doors, args, facts, store_url),
        other => Err(format!(
            "unknown tool `{other}`; this deployment serves: {}",
            names().join(", ")
        )),
    }
}

/// `invoke_skill`: one step, resolved to a script, run on the mount that
/// serves its language.
///
/// The walk is deliberately ONE step (ADR-0004 §3, Phase 4.6): ordering,
/// retry and feeding one step's result into the next are Phase 5's, and a
/// second meaning of "invoke" inside the same tool would make the model's
/// control over the sequence invisible. A multi-step skill is a loop the MODEL
/// drives — it saw the steps in the resolve answer.
fn invoke(
    doors: &mut impl Doors,
    args: &Value,
    facts: &Facts<'_>,
    store_url: &str,
) -> Result<Value, String> {
    let skill_id = args
        .get("skill_id")
        .and_then(Value::as_u64)
        .ok_or("invoke_skill: `skill_id` is required (the `node_id` resolve_skills answered)")?;
    let scope = facts.scope("invoke_skill")?;
    let skill = doors.get(&format!("{store_url}/skills/{scope}/{skill_id}"))?;

    // The steps are TEXT in the row (the store does not parse what the producer
    // owns) — parsing here is the walk's first act, not a second schema.
    let steps_text = skill
        .get("steps")
        .and_then(Value::as_str)
        .ok_or("invoke_skill: the skill answered no `steps`")?;
    let steps: Vec<Value> = if steps_text.trim().is_empty() {
        Vec::new()
    } else {
        serde_json::from_str(steps_text)
            .map_err(|e| format!("invoke_skill: the skill's `steps` is not a JSON array: {e}"))?
    };
    let index = args.get("step").and_then(Value::as_u64).unwrap_or(0) as usize;
    let step = steps.get(index).ok_or_else(|| {
        format!(
            "invoke_skill: step {index} does not exist — this skill declares {} step(s)",
            steps.len()
        )
    })?;
    let code_id = step.get("code_id").and_then(Value::as_u64).ok_or_else(|| {
        format!("invoke_skill: step {index} names no `code_id`")
    })?;

    let code = doors.get(&format!("{store_url}/code/{code_id}"))?;
    let language = code
        .get("language")
        .and_then(Value::as_str)
        .ok_or_else(|| format!("invoke_skill: code row {code_id} answered no `language`"))?;
    let source = code
        .get("source")
        .and_then(Value::as_str)
        .ok_or_else(|| format!("invoke_skill: code row {code_id} answered no `source`"))?;

    // Language → mount is a MATCH against the deployment's declaration, not a
    // default: a language no mount serves cannot be called (ADR-0001 §7), and
    // the error names what this deployment does declare.
    let mount = facts.mount_for(language).ok_or_else(|| {
        let served: Vec<&str> = facts.mounts.iter().map(|(l, _)| l.as_str()).collect();
        format!(
            "invoke_skill: no mount serves `{language}` here (this deployment declares: {})",
            if served.is_empty() {
                "no mounts".to_string()
            } else {
                served.join(", ")
            }
        )
    })?;

    // The script rides the call as an ARGUMENT: the mount's own source is a
    // fixed runner, and nothing about it varies per skill or version (ADR-0001
    // §2 — skills are data, never types).
    let arguments = args.get("arguments").cloned().unwrap_or_else(|| json!({}));

    // The code row states what the script takes. Checking here — before a node
    // is asked to run anything — is the difference between a model that learns
    // the contract from one error and a node that fails on a missing key three
    // layers down.
    let parameters = code
        .get("parameters")
        .and_then(Value::as_str)
        .unwrap_or_default();
    check_arguments(parameters, &arguments).map_err(|e| format!("invoke_skill: {e}"))?;

    doors.mount(
        mount,
        facts.session,
        json!({ "code": source, "args": arguments }),
    )
}

/// Does `arguments` honour the contract a code row declares in `parameters`?
///
/// The field is TEXT the store never parses (k10r ADR-0012 §2: the producer owns
/// the shape), so the only shape this can check is one the producer DECLARED. A
/// document that declares nothing — `{}`, an empty string, or prose somebody wrote
/// for a human — constrains nothing, and passing it means exactly that: nothing
/// was checked, because nothing was claimed. Inventing a rejection for it would
/// break rows whose author chose description over schema, and the model could do
/// nothing about that either.
///
/// The declared form is a JSON Schema SUBSET, because that is what people already
/// write and a custom format would be one more thing to learn:
///   { "type": "object",
///     "required": ["repo"],
///     "properties": { "repo": {"type": "string"}, "n": {"type": "integer"} },
///     "additionalProperties": false }
fn check_arguments(declared: &str, given: &Value) -> Result<(), String> {
    let text = declared.trim();
    if text.is_empty() {
        return Ok(());
    }
    let Ok(schema) = serde_json::from_str::<Value>(text) else {
        // Prose, not a contract. See the doc comment: silence is the honest
        // answer, and the review belongs on the producer's side.
        return Ok(());
    };
    let Some(object) = schema.as_object() else {
        return Ok(());
    };
    let required = object.get("required").and_then(Value::as_array);
    let properties = object.get("properties").and_then(Value::as_object);
    if required.is_none() && properties.is_none() {
        return Ok(());
    }

    let Some(given) = given.as_object() else {
        return Err(format!(
            "the code row declares parameters, so `arguments` must be an object; the contract is: {text}"
        ));
    };

    if let Some(required) = required {
        let missing: Vec<&str> = required
            .iter()
            .filter_map(Value::as_str)
            .filter(|name| !given.contains_key(*name))
            .collect();
        if !missing.is_empty() {
            return Err(format!(
                "`arguments` is missing required {}: {}; the contract is: {text}",
                if missing.len() == 1 { "key" } else { "keys" },
                missing.join(", ")
            ));
        }
    }

    if let Some(properties) = properties {
        for (name, rule) in properties {
            let Some(value) = given.get(name) else {
                continue;
            };
            let wanted = rule.get("type").and_then(type_names).unwrap_or_default();
            if wanted.is_empty() || wanted.iter().any(|kind| is_kind(value, kind)) {
                continue;
            }
            return Err(format!(
                "`arguments.{name}` is {}, and the code row wants {}; the contract is: {text}",
                kind_of(value),
                wanted.join(" or ")
            ));
        }
    }

    if object.get("additionalProperties") == Some(&Value::Bool(false)) {
        let unknown: Vec<&str> = given
            .keys()
            .filter(|name| {
                properties
                    .map(|declared| !declared.contains_key(*name))
                    .unwrap_or(true)
            })
            .map(String::as_str)
            .collect();
        if !unknown.is_empty() {
            return Err(format!(
                "the code row takes no `{}`; the contract is: {text}",
                unknown.join("`, no `")
            ));
        }
    }

    Ok(())
}

/// `"type": "string"` and `"type": ["string", "null"]` are both idiomatic.
fn type_names(value: &Value) -> Option<Vec<&str>> {
    if let Some(name) = value.as_str() {
        return Some(vec![name]);
    }
    value
        .as_array()
        .map(|names| names.iter().filter_map(Value::as_str).collect())
}

fn kind_of(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "a boolean",
        Value::Number(_) => "a number",
        Value::String(_) => "a string",
        Value::Array(_) => "an array",
        Value::Object(_) => "an object",
    }
}

fn is_kind(value: &Value, kind: &str) -> bool {
    match kind {
        "null" => value.is_null(),
        "boolean" => value.is_boolean(),
        "string" => value.is_string(),
        "number" => value.is_number(),
        // JSON has no integer, so this is a number that survived the trip
        // whole; a float that arrived as 4.0 is still the integer 4.
        "integer" => value
            .as_f64()
            .is_some_and(|n| n.is_finite() && n.fract() == 0.0),
        "array" => value.is_array(),
        "object" => value.is_object(),
        // An unknown keyword is not a constraint we can honour; it must not
        // become one that rejects valid arguments.
        _ => true,
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    /// Every door, recorded: what the call asked of each, and canned answers
    /// keyed by a substring of the URL. Enough to pin WHERE a call goes and
    /// WHAT it carries — the doors' own behaviour is k10r's and the realm's.
    #[derive(Default)]
    struct Recorder {
        posts: Vec<(String, Value)>,
        gets: Vec<String>,
        mounts: Vec<(String, String, Value)>,
        answers: Vec<(String, Value)>,
    }

    impl Recorder {
        fn answering(mut self, needle: &str, answer: Value) -> Self {
            self.answers.push((needle.to_string(), answer));
            self
        }
    }

    impl Doors for Recorder {
        fn post(&mut self, url: &str, body: &Value) -> Result<Value, String> {
            self.posts.push((url.to_string(), body.clone()));
            Ok(json!({ "posted": true }))
        }

        fn get(&mut self, url: &str) -> Result<Value, String> {
            self.gets.push(url.to_string());
            self.answers
                .iter()
                .find(|(needle, _)| url.contains(needle.as_str()))
                .map(|(_, answer)| answer.clone())
                .ok_or_else(|| format!("{url}: no canned answer"))
        }

        fn mount(&mut self, type_name: &str, key: &str, args: Value) -> Result<Value, String> {
            self.mounts
                .push((type_name.to_string(), key.to_string(), args));
            Ok(json!({ "ran": true }))
        }
    }

    fn facts<'a>(mounts: &'a [(String, String)], session: &'a str) -> Facts<'a> {
        Facts {
            store_url: "http://mem.test/",
            memory_scope: Some(37),
            mounts,
            session,
        }
    }

    fn python_mount() -> Vec<(String, String)> {
        vec![("python".to_string(), "runner_py".to_string())]
    }

    #[test]
    fn the_list_carries_the_tool_faces_names() {
        assert_eq!(
            names(),
            vec![
                "recall_memory",
                "commit_memory",
                "resolve_skills",
                "invoke_skill"
            ]
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
        let mounts = python_mount();
        let mut doors = Recorder::default();
        let Err(error) = call(
            &mut doors,
            "task_open",
            &json!({}),
            &facts(&mounts, "s-1"),
        ) else {
            panic!("an unlisted tool must be refused");
        };
        assert!(error.contains("unknown tool `task_open`"), "{error}");
        assert!(error.contains("recall_memory"), "{error}");
    }

    #[test]
    fn resolve_skills_posts_to_the_skill_door_with_the_scope_in_the_path() {
        let mounts = python_mount();
        let mut doors = Recorder::default();
        call(
            &mut doors,
            "resolve_skills",
            &json!({ "query": "list github issues", "limit": 3 }),
            &facts(&mounts, "s-1"),
        )
        .expect("the door answered");

        let (url, body) = &doors.posts[0];
        assert_eq!(
            url, "http://mem.test/skills/37/resolve",
            "a skill is a node in the graph, so the graph's scope addresses it"
        );
        assert_eq!(body["query"], "list github issues");
        assert_eq!(body["limit"], 3);
    }

    #[test]
    fn a_missing_scope_names_the_fact_instead_of_guessing() {
        let mounts = python_mount();
        let mut doors = Recorder::default();
        let facts = Facts {
            store_url: "http://mem.test",
            memory_scope: None,
            mounts: &mounts,
            session: "s-1",
        };
        let Err(error) = call(&mut doors, "recall_memory", &json!({ "query": "x" }), &facts) else {
            panic!("a tool with no scope must be refused");
        };
        assert!(error.contains("memory_scope"), "{error}");
    }

    #[test]
    fn invoke_skill_walks_one_step_to_the_mount_that_serves_its_language() {
        let mounts = python_mount();
        let mut doors = Recorder::default()
            .answering(
                "/skills/37/9",
                json!({
                    "node_id": 9,
                    "name": "github-issues",
                    "steps": r#"[{"code_id":4,"note":"fetch"},{"code_id":5,"note":"format"}]"#,
                }),
            )
            .answering(
                "/code/5",
                json!({
                    "language": "python",
                    "description": "formats them",
                    "parameters": "{}",
                    "source": "print('second step')",
                }),
            );

        call(
            &mut doors,
            "invoke_skill",
            &json!({ "skill_id": 9, "step": 1, "arguments": { "repo": "orbsh/okra" } }),
            &facts(&mounts, "channel-7"),
        )
        .expect("the mount answered");

        assert_eq!(
            doors.gets,
            vec![
                "http://mem.test/skills/37/9".to_string(),
                "http://mem.test/code/5".to_string()
            ],
            "the skill, then the code row the CHOSEN step names — never the first step by \
             accident"
        );
        let (type_name, key, args) = &doors.mounts[0];
        assert_eq!(type_name, "runner_py", "the language chose the mount");
        assert_eq!(key, "channel-7", "residency is the caller's (ADR-0001 §6)");
        assert_eq!(args["code"], "print('second step')");
        assert_eq!(args["args"]["repo"], "orbsh/okra");
    }

    #[test]
    fn invoke_skill_refuses_a_language_no_mount_serves() {
        let mounts = python_mount();
        let mut doors = Recorder::default()
            .answering("/skills/37/9", json!({ "steps": r#"[{"code_id":4}]"# }))
            .answering("/code/4", json!({ "language": "nu", "source": "1 + 1" }));

        let Err(error) = call(
            &mut doors,
            "invoke_skill",
            &json!({ "skill_id": 9 }),
            &facts(&mounts, "channel-7"),
        ) else {
            panic!("a language with no mount cannot be called");
        };
        assert!(error.contains("no mount serves `nu`"), "{error}");
        assert!(error.contains("python"), "it names what IS declared: {error}");
        assert!(doors.mounts.is_empty(), "nothing ran");
    }

    #[test]
    fn invoke_skill_refuses_a_step_the_skill_does_not_have() {
        let mounts = python_mount();
        let mut doors = Recorder::default()
            .answering("/skills/37/9", json!({ "steps": r#"[{"code_id":4}]"# }));

        let Err(error) = call(
            &mut doors,
            "invoke_skill",
            &json!({ "skill_id": 9, "step": 3 }),
            &facts(&mounts, "channel-7"),
        ) else {
            panic!("step 3 of a one-step skill must be refused");
        };
        assert!(error.contains("declares 1 step(s)"), "{error}");
        assert!(doors.mounts.is_empty(), "nothing ran");
    }

    #[test]
    fn the_declared_parameters_are_enforced_before_the_node_runs() {
        let schema = r#"{"type":"object","required":["repo"],"properties":{"repo":{"type":"string"},"n":{"type":"integer"}}}"#;
        let mounts = python_mount();

        // Honoured: the walk proceeds with the arguments untouched.
        let mut doors = Recorder::default()
            .answering("/skills/37/9", json!({ "steps": r#"[{"code_id":4}]"# }))
            .answering(
                "/code/4",
                json!({ "language": "python", "parameters": schema, "source": "print(1)" }),
            );
        call(
            &mut doors,
            "invoke_skill",
            &json!({ "skill_id": 9, "arguments": { "repo": "orbsh/okra", "n": 4 } }),
            &facts(&mounts, "channel-7"),
        )
        .expect("a call that honours the contract runs");
        assert_eq!(doors.mounts.len(), 1);

        // A missing required key is refused BY NAME, and nothing runs.
        let mut doors = Recorder::default()
            .answering("/skills/37/9", json!({ "steps": r#"[{"code_id":4}]"# }))
            .answering(
                "/code/4",
                json!({ "language": "python", "parameters": schema, "source": "print(1)" }),
            );
        let Err(error) = call(
            &mut doors,
            "invoke_skill",
            &json!({ "skill_id": 9, "arguments": { "n": 4 } }),
            &facts(&mounts, "channel-7"),
        ) else {
            panic!("a missing required key must not reach the node");
        };
        assert!(error.contains("missing required key: repo"), "{error}");
        assert!(error.contains("the contract is:"), "it hands back the contract: {error}");
        assert!(doors.mounts.is_empty(), "nothing ran");

        // A wrong type is refused, and says what it got and what was wanted.
        let mut doors = Recorder::default()
            .answering("/skills/37/9", json!({ "steps": r#"[{"code_id":4}]"# }))
            .answering(
                "/code/4",
                json!({ "language": "python", "parameters": schema, "source": "print(1)" }),
            );
        let Err(error) = call(
            &mut doors,
            "invoke_skill",
            &json!({ "skill_id": 9, "arguments": { "repo": "orbsh/okra", "n": "four" } }),
            &facts(&mounts, "channel-7"),
        ) else {
            panic!("a string where the row wants an integer must not reach the node");
        };
        assert!(error.contains("`arguments.n` is a string"), "{error}");
        assert!(error.contains("wants integer"), "{error}");
        assert!(doors.mounts.is_empty());
    }

    #[test]
    fn a_row_that_declares_nothing_checks_nothing() {
        let mounts = python_mount();

        // `{}` — the shape every row in the tree has today.
        for parameters in ["{}", "", "the repo, as owner/name"] {
            let mut doors = Recorder::default()
                .answering("/skills/37/9", json!({ "steps": r#"[{"code_id":4}]"# }))
                .answering(
                    "/code/4",
                    json!({ "language": "python", "parameters": parameters, "source": "print(1)" }),
                );
            call(
                &mut doors,
                "invoke_skill",
                &json!({ "skill_id": 9, "arguments": { "anything": [1, 2] } }),
                &facts(&mounts, "channel-7"),
            )
            .unwrap_or_else(|e| panic!("`{parameters}` declares no schema, so it cannot refuse: {e}"));
            assert_eq!(doors.mounts.len(), 1);
        }
    }

    #[test]
    fn additional_properties_false_is_the_rows_choice_to_make() {
        let schema = r#"{"properties":{"repo":{"type":"string"}},"additionalProperties":false}"#;
        let mounts = python_mount();
        let mut doors = Recorder::default()
            .answering("/skills/37/9", json!({ "steps": r#"[{"code_id":4}]"# }))
            .answering(
                "/code/4",
                json!({ "language": "python", "parameters": schema, "source": "print(1)" }),
            );
        let Err(error) = call(
            &mut doors,
            "invoke_skill",
            &json!({ "skill_id": 9, "arguments": { "repo": "orbsh/okra", "verb": "run" } }),
            &facts(&mounts, "channel-7"),
        ) else {
            panic!("a closed contract takes no extra key");
        };
        assert!(error.contains("takes no `verb`"), "{error}");
        assert!(doors.mounts.is_empty());
    }
}